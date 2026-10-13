//! daemon 主循环：一个线程持有全部状态，其他线程（IPC、录音、识别、注入）只发消息。
//!
//! 触发源：Omarchy 模式下由 `omarchy-dictation-yuda` 通过 socket 发 start/stop/toggle；
//! evdev 模式下由本进程的热键线程发同样的命令。协议见 `docs/ipc.md`。

use anyhow::{bail, Context, Result};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    process::Command as Process,
    sync::{
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

use crate::{
    asr,
    audio::Capture,
    config::{Config, EngineMode, TriggerSource},
    history,
    injection::{self, InjectOptions, PasteBackend},
    ipc::{self, Command, Engine, Reply, Snapshot, State},
    session::{Action, Controller},
    text,
};

const ERROR_DISPLAY: Duration = Duration::from_millis(1_500);
const LEVEL_PUSH_INTERVAL: Duration = Duration::from_millis(30);

enum Msg {
    Request(Command, Sender<String>),
    Subscribe(UnixStream),
    Level(u64, f32),
    Asr(asr::Event),
    Refined(u64, String),
    Injected(u64, String, Result<PasteBackend, String>),
}

pub fn run() -> Result<()> {
    let config_path = Config::default_path()?;
    let config = Arc::new(Config::load(&config_path)?);
    let socket = ipc::socket_path()?;
    let listener = bind(&socket)?;
    let (tx, rx) = mpsc::channel();
    spawn_acceptor(listener, tx.clone())?;
    let asr_tx = tx.clone();
    let asr = asr::spawn(Arc::clone(&config), move |event| {
        let _ = asr_tx.send(Msg::Asr(event));
    })?;
    match config.trigger.source {
        TriggerSource::Evdev => spawn_evdev(&config, tx.clone())?,
        TriggerSource::Omarchy => {}
    }
    tracing::info!(
        socket = %socket.display(),
        engine = %config.engine,
        language = %config.language,
        trigger = ?config.trigger.source,
        "语打 daemon 已启动"
    );
    Daemon::new(config_path, config, asr, tx).run(&rx);
    Ok(())
}

struct Daemon {
    config_path: PathBuf,
    config: Arc<Config>,
    pending_config: Option<Config>,
    controller: Controller,
    snapshot: Snapshot,
    subscribers: Vec<UnixStream>,
    capture: Option<Capture>,
    asr: asr::Handle,
    tx: Sender<Msg>,
    record_deadline: Option<Instant>,
    error_deadline: Option<(Instant, u64)>,
    last_push: Instant,
    duration_ms: u64,
    /// 松手（结束录音）的时刻，用于记录「松手到上屏」的耗时。
    released_at: Option<Instant>,
}

impl Daemon {
    fn new(config_path: PathBuf, config: Arc<Config>, asr: asr::Handle, tx: Sender<Msg>) -> Self {
        let snapshot = Snapshot {
            language: config.language.as_str().to_owned(),
            ..Snapshot::default()
        };
        Self {
            controller: Controller::new(Duration::from_millis(config.trigger.min_hold_ms)),
            config_path,
            config,
            pending_config: None,
            snapshot,
            subscribers: Vec::new(),
            capture: None,
            asr,
            tx,
            record_deadline: None,
            error_deadline: None,
            last_push: Instant::now(),
            duration_ms: 0,
            released_at: None,
        }
    }

    fn run(&mut self, rx: &Receiver<Msg>) {
        loop {
            let deadline = [self.record_deadline, self.error_deadline.map(|(at, _)| at)]
                .into_iter()
                .flatten()
                .min();
            let timeout = deadline
                .map(|at| at.saturating_duration_since(Instant::now()))
                .unwrap_or(Duration::from_secs(3_600));
            match rx.recv_timeout(timeout) {
                Ok(msg) => self.handle(msg),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            self.check_deadlines();
        }
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Request(command, reply) => {
                let line = match command {
                    Command::Status => self.snapshot.to_line(),
                    Command::Reload => reply_line(&self.reload()),
                    Command::Subscribe => reply_line(&Reply::error("subscribe 需要单独连接")),
                    _ => reply_line(&self.control(command)),
                };
                let _ = reply.send(line);
            }
            Msg::Subscribe(stream) => self.subscribe(stream),
            Msg::Level(session, level) => self.level(session, level),
            Msg::Asr(event) => self.asr_event(event),
            Msg::Refined(session, text) => self.inject(session, text),
            Msg::Injected(session, text, result) => self.injected(session, &text, result),
        }
    }

    fn control(&mut self, command: Command) -> Reply {
        let now = Instant::now();
        match self.controller.on_command(command, now) {
            Action::Begin(session) => self.begin(session, now),
            Action::Finish(session) => {
                self.finish(session);
                Reply::ok()
            }
            Action::Discard(session) => {
                self.discard(session);
                Reply::ok()
            }
            Action::Nothing => Reply::ok(),
            Action::Reject(message) => Reply::error(message),
        }
    }

    fn begin(&mut self, session: u64, now: Instant) -> Reply {
        self.error_deadline = None;
        self.asr.begin(session, Arc::clone(&self.config));
        let asr = self.asr.clone();
        let tx = self.tx.clone();
        let capture = Capture::start(
            move |samples| asr.audio(session, samples.to_vec()),
            move |level| {
                let _ = tx.send(Msg::Level(session, level));
            },
        );
        match capture {
            Ok(capture) => {
                self.capture = Some(capture);
                self.record_deadline =
                    Some(now + Duration::from_secs(self.config.trigger.max_recording_secs));
                self.snapshot = Snapshot {
                    state: State::Recording,
                    engine: self.expected_engine(),
                    language: self.config.language.as_str().to_owned(),
                    ..Snapshot::default()
                };
                self.push();
                tracing::info!(session, "开始录音");
                Reply::ok()
            }
            Err(error) => {
                self.asr.cancel(session);
                let message = format!("麦克风不可用：{error:#}");
                self.fail(session, &message);
                Reply::error(message)
            }
        }
    }

    /// 会话开始时 UI 先显示的引擎；实际引擎由识别线程随后确认。
    fn expected_engine(&self) -> Engine {
        if self.config.engine != EngineMode::Offline && self.config.cloud.is_configured() {
            Engine::Cloud
        } else {
            Engine::Offline
        }
    }

    fn stop_capture(&mut self) {
        if let Some(capture) = self.capture.take() {
            capture.stop();
        }
        self.record_deadline = None;
    }

    fn finish(&mut self, session: u64) {
        self.duration_ms = self.controller.elapsed(Instant::now()).as_millis() as u64;
        self.released_at = Some(Instant::now());
        self.stop_capture();
        self.asr.end(session);
        self.snapshot.state = State::Transcribing;
        self.snapshot.level = 0.0;
        self.snapshot.elapsed_ms = self.duration_ms;
        self.push();
        tracing::info!(
            session,
            duration_ms = self.duration_ms,
            "录音结束，等待识别结果"
        );
    }

    fn discard(&mut self, session: u64) {
        self.stop_capture();
        self.asr.cancel(session);
        self.enter_idle();
        tracing::info!(session, "会话已丢弃");
    }

    fn level(&mut self, session: u64, level: f32) {
        if session != self.controller.session() || self.snapshot.state != State::Recording {
            return;
        }
        self.snapshot.level = level;
        self.snapshot.elapsed_ms = self.controller.elapsed(Instant::now()).as_millis() as u64;
        if self.last_push.elapsed() >= LEVEL_PUSH_INTERVAL {
            self.push();
        }
    }

    fn asr_event(&mut self, event: asr::Event) {
        match event {
            asr::Event::Engine {
                session,
                engine,
                fallback,
            } if session == self.controller.session() => {
                self.snapshot.engine = engine;
                self.snapshot.fallback = fallback;
                self.push();
            }
            asr::Event::Partial { session, text }
                if session == self.controller.session()
                    && matches!(self.snapshot.state, State::Recording | State::Transcribing) =>
            {
                self.snapshot.text = text::process(&text, &self.config.text);
                self.push();
            }
            asr::Event::Final { session, result } if session == self.controller.session() => {
                match result {
                    Err(message) => self.fail(session, &message),
                    Ok(raw) => {
                        let processed = text::process(&raw, &self.config.text);
                        tracing::info!(session, chars = processed.chars().count(), "识别完成");
                        if processed.is_empty() {
                            self.fail(session, "没有识别到语音");
                        } else {
                            self.refine_or_inject(session, processed);
                        }
                    }
                }
            }
            // 已取消或过期会话的事件。
            _ => {}
        }
    }

    fn refine_or_inject(&mut self, session: u64, text: String) {
        #[cfg(feature = "llm")]
        if self.config.llm.is_usable() && self.controller.advance(session, State::Refining) {
            self.snapshot.state = State::Refining;
            self.snapshot.text = text.clone();
            self.push();
            let llm = self.config.llm.clone();
            let tx = self.tx.clone();
            thread::spawn(move || {
                let started = Instant::now();
                let refined = match crate::refine::refine(&llm, &text) {
                    Ok(refined) => {
                        tracing::info!(ms = started.elapsed().as_millis(), "LLM 纠错完成");
                        refined
                    }
                    Err(error) => {
                        tracing::warn!(error = %format!("{error:#}"), "LLM 纠错失败，使用原文");
                        text
                    }
                };
                let _ = tx.send(Msg::Refined(session, refined));
            });
            return;
        }
        self.inject(session, text);
    }

    fn inject(&mut self, session: u64, text: String) {
        if !self.controller.advance(session, State::Injecting) {
            return;
        }
        self.snapshot.state = State::Injecting;
        self.snapshot.text = text.clone();
        self.push();
        let options = InjectOptions {
            paste_delay: Duration::from_millis(self.config.injection.paste_delay_ms),
            auto_paste: self.config.injection.auto_paste,
        };
        let tx = self.tx.clone();
        thread::spawn(move || {
            let result =
                injection::inject_text(&text, &options).map_err(|error| format!("{error:#}"));
            let _ = tx.send(Msg::Injected(session, text, result));
        });
    }

    fn injected(&mut self, session: u64, text: &str, result: Result<PasteBackend, String>) {
        match result {
            Ok(backend) => {
                if !self.controller.advance(session, State::Idle) {
                    return;
                }
                let latency_ms = self
                    .released_at
                    .take()
                    .map(|at| at.elapsed().as_millis() as u64)
                    .unwrap_or_default();
                tracing::info!(session, backend = backend.label(), latency_ms, "已上屏");
                if backend == PasteBackend::ClipboardOnly && self.config.injection.auto_paste {
                    notify("语打", "无法模拟粘贴，结果已复制到剪贴板，按 Super+V 粘贴");
                }
                self.record_history(text);
                self.enter_idle();
            }
            Err(message) => self.fail(session, &format!("上屏失败：{message}")),
        }
    }

    fn record_history(&self, text: &str) {
        if !self.config.history.enabled {
            return;
        }
        let entry = history::Entry::now(text, self.snapshot.engine.label(), self.duration_ms);
        let result = history::default_path()
            .and_then(|path| history::append(&path, &entry, self.config.history.max_entries));
        if let Err(error) = result {
            tracing::warn!(error = %format!("{error:#}"), "写入识别历史失败");
        }
    }

    fn fail(&mut self, session: u64, message: &str) {
        self.stop_capture();
        if !self.controller.advance(session, State::Error) {
            return;
        }
        tracing::warn!(session, message, "会话出错");
        self.snapshot.state = State::Error;
        self.snapshot.level = 0.0;
        self.snapshot.message = message.to_owned();
        self.error_deadline = Some((Instant::now() + ERROR_DISPLAY, session));
        self.push();
        if message != "没有识别到语音" {
            notify("语打", message);
        }
    }

    fn enter_idle(&mut self) {
        self.snapshot = Snapshot {
            engine: self.snapshot.engine,
            fallback: self.snapshot.fallback,
            language: self.config.language.as_str().to_owned(),
            ..Snapshot::default()
        };
        self.push();
        if let Some(config) = self.pending_config.take() {
            self.apply_config(config);
        }
    }

    fn check_deadlines(&mut self) {
        let now = Instant::now();
        if let Some((at, session)) = self.error_deadline {
            if now >= at {
                self.error_deadline = None;
                if self.controller.clear_error(session) {
                    self.enter_idle();
                }
            }
        }
        if self.record_deadline.is_some_and(|at| now >= at) {
            self.record_deadline = None;
            if let Action::Finish(session) = self.controller.on_timeout(self.controller.session()) {
                notify("语打", "已达到单次录音上限，自动结束录音");
                self.finish(session);
            }
        }
    }

    fn reload(&mut self) -> Reply {
        match Config::load(&self.config_path) {
            Ok(config) => {
                if self.controller.state() == State::Idle {
                    self.apply_config(config);
                } else {
                    self.pending_config = Some(config);
                }
                Reply::ok()
            }
            Err(error) => Reply::error(format!("{error:#}")),
        }
    }

    fn apply_config(&mut self, config: Config) {
        if config.trigger.source != self.config.trigger.source {
            tracing::warn!("trigger.source 的修改需要重启 yuda.service 才生效");
        }
        self.controller
            .set_min_hold(Duration::from_millis(config.trigger.min_hold_ms));
        self.config = Arc::new(config);
        self.asr.reload(Arc::clone(&self.config));
        self.snapshot.language = self.config.language.as_str().to_owned();
        self.push();
        tracing::info!(engine = %self.config.engine, language = %self.config.language, "配置已重新加载");
    }

    fn subscribe(&mut self, stream: UnixStream) {
        if stream.set_nonblocking(true).is_err() {
            return;
        }
        let mut stream = stream;
        if stream.write_all(self.snapshot.to_line().as_bytes()).is_ok() {
            self.subscribers.push(stream);
        }
    }

    fn push(&mut self) {
        self.last_push = Instant::now();
        if self.subscribers.is_empty() {
            return;
        }
        let line = self.snapshot.to_line();
        // 写不进去（客户端退出或不读取）就断开，客户端会自行重连。
        self.subscribers
            .retain_mut(|stream| stream.write_all(line.as_bytes()).is_ok());
    }
}

fn reply_line(reply: &Reply) -> String {
    let mut line = serde_json::to_string(reply).unwrap_or_else(|_| r#"{"ok":false}"#.to_owned());
    line.push('\n');
    line
}

fn bind(path: &Path) -> Result<UnixListener> {
    let directory = path.parent().context("socket 路径没有父目录")?;
    fs::create_dir_all(directory)
        .with_context(|| format!("创建运行目录失败: {}", directory.display()))?;
    fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("设置运行目录权限失败: {}", directory.display()))?;
    if path.exists() {
        if UnixStream::connect(path).is_ok() {
            bail!("语打 daemon 已在运行（{}）", path.display());
        }
        fs::remove_file(path)
            .with_context(|| format!("删除残留 socket 失败: {}", path.display()))?;
    }
    UnixListener::bind(path).with_context(|| format!("监听 socket 失败: {}", path.display()))
}

fn spawn_acceptor(listener: UnixListener, tx: Sender<Msg>) -> Result<()> {
    thread::Builder::new()
        .name("yuda-ipc".to_owned())
        .spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => {
                        if let Err(error) = serve(stream, &tx) {
                            tracing::debug!(error = %format!("{error:#}"), "处理 IPC 连接失败");
                        }
                    }
                    Err(error) => tracing::warn!(%error, "接受 IPC 连接失败"),
                }
            }
        })
        .context("启动 IPC 线程失败")?;
    Ok(())
}

fn serve(stream: UnixStream, tx: &Sender<Msg>) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(1)))?;
    stream.set_write_timeout(Some(Duration::from_secs(1)))?;
    let mut line = String::new();
    BufReader::new(stream.try_clone()?).read_line(&mut line)?;
    let mut writer = &stream;
    let command: Command = match line.parse() {
        Ok(command) => command,
        Err(error) => {
            writer.write_all(reply_line(&Reply::error(error.to_string())).as_bytes())?;
            return Ok(());
        }
    };
    if command == Command::Subscribe {
        stream.set_read_timeout(None)?;
        tx.send(Msg::Subscribe(stream))
            .map_err(|_| anyhow::anyhow!("daemon 主循环已退出"))?;
        return Ok(());
    }
    let (reply_tx, reply_rx) = mpsc::channel();
    tx.send(Msg::Request(command, reply_tx))
        .map_err(|_| anyhow::anyhow!("daemon 主循环已退出"))?;
    let reply = reply_rx
        .recv_timeout(Duration::from_secs(4))
        .context("daemon 没有及时回应")?;
    writer.write_all(reply.as_bytes())?;
    Ok(())
}

#[cfg(feature = "evdev")]
fn spawn_evdev(config: &Config, tx: Sender<Msg>) -> Result<()> {
    use crate::hotkey::{HotkeyAction, HotkeyListener};
    let listener = HotkeyListener::start(&config.trigger.evdev_key)
        .context("启动 evdev 热键监听失败；Omarchy 用户请把 trigger.source 设为 omarchy")?;
    tracing::info!(key = %config.trigger.evdev_key, "evdev 热键已启用");
    thread::Builder::new()
        .name("yuda-evdev".to_owned())
        .spawn(move || loop {
            let command = match listener.recv() {
                Ok(HotkeyAction::Pressed) => Command::Start,
                Ok(HotkeyAction::Released { .. }) => Command::Stop,
                Ok(HotkeyAction::Cancelled) => Command::Cancel,
                Err(_) => return,
            };
            let (reply_tx, _reply_rx) = mpsc::channel();
            if tx.send(Msg::Request(command, reply_tx)).is_err() {
                return;
            }
        })
        .context("启动 evdev 线程失败")?;
    Ok(())
}

#[cfg(not(feature = "evdev"))]
fn spawn_evdev(_config: &Config, _tx: Sender<Msg>) -> Result<()> {
    bail!("此构建不含 evdev 热键；请把 trigger.source 设为 omarchy")
}

/// 桌面通知；Omarchy 下走 omarchy-notification-send，其他环境用 notify-send。
fn notify(title: &str, body: &str) {
    let title = title.to_owned();
    let body = body.to_owned();
    thread::spawn(move || {
        let sent = Process::new("omarchy-notification-send")
            .args([&title, &body])
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if !sent {
            let _ = Process::new("notify-send")
                .args(["-a", "语打", &title, &body])
                .status();
        }
    });
}
