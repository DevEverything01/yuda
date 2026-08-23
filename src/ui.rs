use crate::app::{Session, SessionState, Transcript};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiSnapshot {
    pub state: SessionState,
    pub eyebrow: String,
    pub title: String,
    pub transcript: String,
    pub hint: String,
    pub engine_label: String,
    pub hotkey_label: String,
    pub level: u8,
    pub elapsed_label: String,
    pub error: Option<String>,
}

impl From<&Session> for UiSnapshot {
    fn from(session: &Session) -> Self {
        let transcript = session
            .transcript
            .as_ref()
            .map(|item| item.text.clone())
            .unwrap_or_default();
        let (eyebrow, title, hint) = match session.state {
            SessionState::Idle => ("YUDA / 语打", "按住，说出你的想法", "右 Ctrl · 松手即上屏"),
            SessionState::Recording => ("正在聆听", "我在听，请继续", "松开右 Ctrl 完成识别"),
            SessionState::Transcribing => ("正在整理", "马上就好", "正在生成可编辑文本"),
            SessionState::Refining => ("智能优化", "让表达更准确", "只修正明显的识别错误"),
            SessionState::Ready => ("准备上屏", "确认后即可使用", "识别结果已保留在剪贴板"),
            SessionState::Error => ("需要检查", "这次没有完成", "打开设置检查语音引擎"),
        };
        Self {
            state: session.state,
            eyebrow: eyebrow.to_owned(),
            title: title.to_owned(),
            transcript,
            hint: hint.to_owned(),
            engine_label: session.engine_label.clone(),
            hotkey_label: session.hotkey_label.clone(),
            level: session.level,
            elapsed_label: format_elapsed(session.elapsed_ms),
            error: session.error.clone(),
        }
    }
}

pub fn demo_session() -> Session {
    Session {
        engine_label: "云端优先 · 自动兜底".to_owned(),
        hotkey_label: "右 Ctrl".to_owned(),
        ..Session::default()
    }
}

pub fn demo_recording_session() -> Session {
    let mut session = demo_session();
    session.start_recording();
    session.update_meter(72, 4_800);
    session.transcript = Some(Transcript::new("帮我把这段会议记录整理成三个重点", 94));
    session
}

fn format_elapsed(elapsed_ms: u64) -> String {
    format!("{}.{:01}s", elapsed_ms / 1_000, (elapsed_ms % 1_000) / 100)
}

#[cfg(test)]
mod tests {
    use super::{demo_recording_session, demo_session, UiSnapshot};
    use crate::app::SessionState;

    #[test]
    fn idle_snapshot_explains_primary_action() {
        let snapshot = UiSnapshot::from(&demo_session());
        assert_eq!(snapshot.state, SessionState::Idle);
        assert!(snapshot.hint.contains("松手"));
    }

    #[test]
    fn recording_snapshot_contains_live_text_and_meter() {
        let snapshot = UiSnapshot::from(&demo_recording_session());
        assert_eq!(snapshot.state, SessionState::Recording);
        assert_eq!(snapshot.level, 72);
        assert!(snapshot.transcript.contains("会议记录"));
        assert_eq!(snapshot.elapsed_label, "4.8s");
    }
}
