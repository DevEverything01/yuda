#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasteBackend {
    Wtype,
    Ydotool,
    ClipboardOnly,
}

impl PasteBackend {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Wtype => "wtype",
            Self::Ydotool => "ydotool",
            Self::ClipboardOnly => "剪贴板",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InjectionStep {
    pub name: &'static str,
    pub detail: &'static str,
}

pub fn injection_sequence(backend: PasteBackend, delay_ms: u64) -> Vec<InjectionStep> {
    let mut steps = vec![
        InjectionStep {
            name: "snapshot",
            detail: "保存当前剪贴板",
        },
        InjectionStep {
            name: "copy",
            detail: "写入识别结果",
        },
    ];
    match backend {
        PasteBackend::Wtype => steps.push(InjectionStep {
            name: "paste",
            detail: "wtype 模拟 Ctrl+V",
        }),
        PasteBackend::Ydotool => steps.push(InjectionStep {
            name: "paste",
            detail: "ydotool 模拟 Ctrl+V",
        }),
        PasteBackend::ClipboardOnly => steps.push(InjectionStep {
            name: "paste",
            detail: "保留在剪贴板",
        }),
    }
    steps.push(InjectionStep {
        name: "delay",
        detail: if delay_ms == 0 {
            "立即恢复"
        } else {
            "等待粘贴完成"
        },
    });
    steps.push(InjectionStep {
        name: "restore",
        detail: "恢复原剪贴板",
    });
    steps
}

#[cfg(test)]
mod tests {
    use super::{injection_sequence, PasteBackend};

    #[test]
    fn clipboard_sequence_restores_after_paste() {
        let steps = injection_sequence(PasteBackend::Wtype, 120);
        let names: Vec<_> = steps.iter().map(|step| step.name).collect();
        assert_eq!(names, ["snapshot", "copy", "paste", "delay", "restore"]);
    }

    #[test]
    fn fallback_is_explicit() {
        let steps = injection_sequence(PasteBackend::ClipboardOnly, 0);
        assert_eq!(steps[2].detail, "保留在剪贴板");
        assert_eq!(steps[3].detail, "立即恢复");
    }
}
