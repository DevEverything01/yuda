//! 会话状态机：把控制命令翻译成 daemon 要执行的动作。纯逻辑，不碰音频与 IO。
//!
//! 每次开始录音分配一个新的会话号；取消或开始新会话后，旧会话的迟到结果按会话号丢弃，
//! 保证「一次会话最多上屏一次」。

use crate::ipc::{Command, State};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// 开始采集音频与识别。
    Begin(u64),
    /// 停止采集，等待最终识别结果。
    Finish(u64),
    /// 停止采集并丢弃本次会话。
    Discard(u64),
    /// 命令有效但无需动作（幂等）。
    Nothing,
    /// 当前状态不接受该命令。
    Reject(&'static str),
}

#[derive(Debug)]
pub struct Controller {
    state: State,
    session: u64,
    started_at: Option<Instant>,
    min_hold: Duration,
}

impl Controller {
    pub fn new(min_hold: Duration) -> Self {
        Self {
            state: State::Idle,
            session: 0,
            started_at: None,
            min_hold,
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    pub fn session(&self) -> u64 {
        self.session
    }

    pub fn set_min_hold(&mut self, min_hold: Duration) {
        self.min_hold = min_hold;
    }

    /// 录音已持续的时间。
    pub fn elapsed(&self, now: Instant) -> Duration {
        self.started_at
            .map(|started| now.saturating_duration_since(started))
            .unwrap_or_default()
    }

    pub fn on_command(&mut self, command: Command, now: Instant) -> Action {
        match (command, self.state) {
            (Command::Start | Command::Toggle, State::Idle | State::Error) => self.begin(now),
            (Command::Start, State::Recording) => Action::Nothing,
            (Command::Stop | Command::Toggle, State::Recording) => {
                if self.elapsed(now) < self.min_hold {
                    self.discard()
                } else {
                    self.state = State::Transcribing;
                    Action::Finish(self.session)
                }
            }
            (Command::Start | Command::Toggle, _) => Action::Reject("上一段语音仍在处理，请稍候"),
            (Command::Cancel, State::Recording | State::Transcribing | State::Refining) => {
                self.discard()
            }
            _ => Action::Nothing,
        }
    }

    /// 不经录音、直接处理一条文字指令（`yuda edit`）：开一个新会话并跳过录音阶段。
    /// 忙时返回 None。
    pub fn begin_direct(&mut self, now: Instant) -> Option<u64> {
        if !matches!(self.state, State::Idle | State::Error) {
            return None;
        }
        self.session += 1;
        self.state = State::Transcribing;
        self.started_at = Some(now);
        Some(self.session)
    }

    /// 最大录音时长到达时由 daemon 调用，等同于松开按键。
    pub fn on_timeout(&mut self, session: u64) -> Action {
        if session == self.session && self.state == State::Recording {
            self.state = State::Transcribing;
            Action::Finish(self.session)
        } else {
            Action::Nothing
        }
    }

    /// 识别、纠错、上屏阶段推进；会话号不匹配（已取消/过期）时返回 false，调用方应丢弃结果。
    pub fn advance(&mut self, session: u64, next: State) -> bool {
        if session != self.session || matches!(self.state, State::Idle | State::Error) {
            return false;
        }
        let allowed = matches!(
            (self.state, next),
            (State::Recording, State::Error)
                | (
                    State::Transcribing,
                    State::Refining | State::Injecting | State::Error | State::Idle
                )
                | (State::Refining, State::Injecting | State::Error)
                | (State::Injecting, State::Idle | State::Error)
        );
        if allowed {
            self.state = next;
        }
        allowed
    }

    /// 错误展示时间结束后回到空闲；期间若已开始新会话则不变。
    pub fn clear_error(&mut self, session: u64) -> bool {
        if session == self.session && self.state == State::Error {
            self.state = State::Idle;
            true
        } else {
            false
        }
    }

    fn begin(&mut self, now: Instant) -> Action {
        self.session += 1;
        self.state = State::Recording;
        self.started_at = Some(now);
        Action::Begin(self.session)
    }

    fn discard(&mut self) -> Action {
        let session = self.session;
        // 让迟到的识别结果失效。
        self.session += 1;
        self.state = State::Idle;
        Action::Discard(session)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOLD: Duration = Duration::from_millis(300);

    fn after(start: Instant, ms: u64) -> Instant {
        start + Duration::from_millis(ms)
    }

    #[test]
    fn push_to_talk_flow_reaches_idle_once() {
        let t0 = Instant::now();
        let mut controller = Controller::new(HOLD);
        let Action::Begin(id) = controller.on_command(Command::Start, t0) else {
            panic!("start must begin a session");
        };
        assert_eq!(
            controller.on_command(Command::Start, after(t0, 50)),
            Action::Nothing
        );
        assert_eq!(
            controller.on_command(Command::Stop, after(t0, 1_500)),
            Action::Finish(id)
        );
        assert!(controller.advance(id, State::Injecting));
        assert!(controller.advance(id, State::Idle));
        assert!(
            !controller.advance(id, State::Idle),
            "second injection must be refused"
        );
        assert_eq!(
            controller.on_command(Command::Stop, after(t0, 1_600)),
            Action::Nothing
        );
    }

    #[test]
    fn hold_shorter_than_minimum_is_discarded() {
        let t0 = Instant::now();
        let mut controller = Controller::new(HOLD);
        let Action::Begin(id) = controller.on_command(Command::Start, t0) else {
            panic!("start must begin");
        };
        assert_eq!(
            controller.on_command(Command::Stop, after(t0, 299)),
            Action::Discard(id)
        );
        assert_eq!(controller.state(), State::Idle);
        assert!(
            !controller.advance(id, State::Injecting),
            "late result must be dropped"
        );
    }

    #[test]
    fn exact_minimum_hold_is_accepted() {
        let t0 = Instant::now();
        let mut controller = Controller::new(HOLD);
        controller.on_command(Command::Start, t0);
        assert!(matches!(
            controller.on_command(Command::Stop, after(t0, 300)),
            Action::Finish(_)
        ));
    }

    #[test]
    fn toggle_starts_then_finishes() {
        let t0 = Instant::now();
        let mut controller = Controller::new(HOLD);
        assert!(matches!(
            controller.on_command(Command::Toggle, t0),
            Action::Begin(_)
        ));
        assert!(matches!(
            controller.on_command(Command::Toggle, after(t0, 2_000)),
            Action::Finish(_)
        ));
        assert_eq!(controller.state(), State::Transcribing);
    }

    #[test]
    fn busy_pipeline_rejects_new_recording() {
        let t0 = Instant::now();
        let mut controller = Controller::new(HOLD);
        controller.on_command(Command::Start, t0);
        controller.on_command(Command::Stop, after(t0, 1_000));
        assert!(matches!(
            controller.on_command(Command::Start, after(t0, 1_100)),
            Action::Reject(_)
        ));
    }

    #[test]
    fn cancel_during_transcription_drops_result() {
        let t0 = Instant::now();
        let mut controller = Controller::new(HOLD);
        let Action::Begin(id) = controller.on_command(Command::Start, t0) else {
            panic!("start must begin");
        };
        controller.on_command(Command::Stop, after(t0, 1_000));
        assert_eq!(
            controller.on_command(Command::Cancel, after(t0, 1_100)),
            Action::Discard(id)
        );
        assert!(!controller.advance(id, State::Injecting));
        assert!(matches!(
            controller.on_command(Command::Start, after(t0, 1_200)),
            Action::Begin(next) if next > id
        ));
    }

    #[test]
    fn error_clears_back_to_idle_and_allows_restart() {
        let t0 = Instant::now();
        let mut controller = Controller::new(HOLD);
        let Action::Begin(id) = controller.on_command(Command::Start, t0) else {
            panic!("start must begin");
        };
        assert!(controller.advance(id, State::Error));
        assert!(matches!(
            controller.on_command(Command::Start, after(t0, 100)),
            Action::Begin(_)
        ));
        assert!(
            !controller.clear_error(id),
            "new session must not be cleared"
        );
    }

    #[test]
    fn stop_and_cancel_are_harmless_when_idle() {
        let mut controller = Controller::new(HOLD);
        let now = Instant::now();
        assert_eq!(controller.on_command(Command::Stop, now), Action::Nothing);
        assert_eq!(controller.on_command(Command::Cancel, now), Action::Nothing);
        assert_eq!(controller.state(), State::Idle);
    }

    #[test]
    fn direct_instruction_skips_recording_and_respects_busy_state() {
        let t0 = Instant::now();
        let mut controller = Controller::new(HOLD);
        let id = controller
            .begin_direct(t0)
            .expect("idle accepts a direct instruction");
        assert_eq!(controller.state(), State::Transcribing);
        assert_eq!(
            controller.begin_direct(t0),
            None,
            "busy pipeline must refuse"
        );
        assert!(controller.advance(id, State::Injecting));
        assert!(controller.advance(id, State::Idle));
    }

    #[test]
    fn max_duration_timeout_finishes_only_current_recording() {
        let t0 = Instant::now();
        let mut controller = Controller::new(HOLD);
        let Action::Begin(id) = controller.on_command(Command::Start, t0) else {
            panic!("start must begin");
        };
        assert_eq!(controller.on_timeout(id + 7), Action::Nothing);
        assert_eq!(controller.on_timeout(id), Action::Finish(id));
        assert_eq!(controller.on_timeout(id), Action::Nothing);
    }
}
