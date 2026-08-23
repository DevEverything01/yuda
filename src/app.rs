use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Idle,
    Recording,
    Transcribing,
    Refining,
    Ready,
    Injecting,
    Error,
}

impl SessionState {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Idle => "准备就绪",
            Self::Recording => "正在聆听",
            Self::Transcribing => "正在整理语句",
            Self::Refining => "正在优化表达",
            Self::Ready => "已准备上屏",
            Self::Injecting => "正在上屏",
            Self::Error => "需要检查设置",
        }
    }

    pub const fn is_busy(self) -> bool {
        matches!(
            self,
            Self::Recording | Self::Transcribing | Self::Refining | Self::Injecting
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transcript {
    pub text: String,
    pub confidence: u8,
}

impl Transcript {
    pub fn new(text: impl Into<String>, confidence: u8) -> Self {
        Self {
            text: text.into(),
            confidence: confidence.min(100),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub state: SessionState,
    pub transcript: Option<Transcript>,
    pub engine_label: String,
    pub hotkey_label: String,
    pub elapsed_ms: u64,
    pub level: u8,
    pub error: Option<String>,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            state: SessionState::Idle,
            transcript: None,
            engine_label: "云端优先".to_owned(),
            hotkey_label: "右 Ctrl".to_owned(),
            elapsed_ms: 0,
            level: 0,
            error: None,
        }
    }
}

impl Session {
    pub fn start_recording(&mut self) -> bool {
        if self.state.is_busy() {
            return false;
        }
        self.state = SessionState::Recording;
        self.transcript = None;
        self.error = None;
        self.elapsed_ms = 0;
        self.level = 0;
        true
    }

    pub fn update_meter(&mut self, level: u8, elapsed_ms: u64) {
        if self.state == SessionState::Recording {
            self.level = level.min(100);
            self.elapsed_ms = elapsed_ms;
        }
    }

    pub fn stop_recording(&mut self, transcript: Transcript) -> bool {
        if self.state != SessionState::Recording || transcript.is_empty() {
            return false;
        }
        self.state = SessionState::Transcribing;
        self.transcript = Some(transcript);
        self.level = 0;
        true
    }

    pub fn accept_keyboard_text(&mut self, text: impl Into<String>) -> bool {
        let text = text.into();
        if text.trim().is_empty() || self.state.is_busy() {
            return false;
        }
        self.transcript = Some(Transcript::new(text, 100));
        self.error = None;
        self.state = SessionState::Ready;
        true
    }

    pub fn edit_transcript(&mut self, text: impl Into<String>) -> bool {
        let text = text.into();
        if text.trim().is_empty() || self.state.is_busy() {
            return false;
        }
        self.transcript = Some(Transcript::new(text, 100));
        self.error = None;
        self.state = SessionState::Ready;
        true
    }

    pub fn begin_refining(&mut self) -> bool {
        if self.state != SessionState::Transcribing {
            return false;
        }
        self.state = SessionState::Refining;
        true
    }

    pub fn mark_ready(&mut self) -> bool {
        if !matches!(
            self.state,
            SessionState::Transcribing | SessionState::Refining
        ) {
            return false;
        }
        self.state = SessionState::Ready;
        true
    }

    pub fn begin_injection(&mut self) -> bool {
        if self.state != SessionState::Ready {
            return false;
        }
        self.state = SessionState::Injecting;
        true
    }

    pub fn complete_injection(&mut self) -> bool {
        if self.state != SessionState::Injecting {
            return false;
        }
        self.state = SessionState::Idle;
        self.level = 0;
        true
    }

    pub fn fail(&mut self, message: impl Into<String>) {
        self.state = SessionState::Error;
        self.error = Some(message.into());
        self.level = 0;
    }

    pub fn reset(&mut self) {
        self.state = SessionState::Idle;
        self.transcript = None;
        self.error = None;
        self.elapsed_ms = 0;
        self.level = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::{Session, SessionState, Transcript};

    #[test]
    fn recording_session_reaches_ready_and_injects_once() {
        let mut session = Session::default();
        assert!(session.start_recording());
        session.update_meter(86, 1_240);
        assert!(session.stop_recording(Transcript::new("今天下午三点开会", 96)));
        assert_eq!(session.state, SessionState::Transcribing);
        assert!(session.begin_refining());
        assert!(session.mark_ready());
        assert!(session.begin_injection());
        assert!(session.complete_injection());
        assert_eq!(session.state, SessionState::Idle);
        assert!(!session.complete_injection());
    }

    #[test]
    fn keyboard_text_enters_ready_state() {
        let mut session = Session::default();
        assert!(session.accept_keyboard_text("整理今天的会议 agenda"));
        assert_eq!(session.state, SessionState::Ready);
        assert_eq!(
            session.transcript.as_ref().map(|item| item.confidence),
            Some(100)
        );
    }

    #[test]
    fn empty_transcript_cannot_be_injected() {
        let mut session = Session::default();
        assert!(session.start_recording());
        assert!(!session.stop_recording(Transcript::new("   ", 100)));
        assert_eq!(session.state, SessionState::Recording);
    }

    #[test]
    fn error_can_be_reset() {
        let mut session = Session::default();
        session.fail("未找到可用的语音引擎");
        assert_eq!(session.state, SessionState::Error);
        session.reset();
        assert_eq!(session.state, SessionState::Idle);
        assert!(session.error.is_none());
    }
}
