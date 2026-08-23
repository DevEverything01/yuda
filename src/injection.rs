use anyhow::{bail, Context, Result};
use std::{
    io::Write,
    process::{Command, Stdio},
    thread,
    time::Duration,
};

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

#[derive(Debug)]
struct ClipboardSnapshot {
    mime: String,
    data: Vec<u8>,
}

pub fn inject_text(text: &str, delay: Duration) -> Result<PasteBackend> {
    if text.trim().is_empty() {
        bail!("拒绝上屏空文本");
    }
    let snapshot = snapshot_clipboard().context("保存当前剪贴板失败")?;
    let injection_result = (|| {
        copy_text(text).context("写入识别结果到剪贴板失败")?;
        let backend = paste_with_available_backend()?;
        thread::sleep(delay);
        Ok(backend)
    })();
    let restore_result = restore_clipboard(snapshot);
    match (injection_result, restore_result) {
        (Ok(backend), Ok(())) => Ok(backend),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(error).context("上屏完成，但恢复原剪贴板失败"),
        (Err(error), Err(restore_error)) => {
            Err(error).context(format!("上屏失败，且恢复原剪贴板也失败: {restore_error:#}"))
        }
    }
}

fn snapshot_clipboard() -> Result<Option<ClipboardSnapshot>> {
    let types = Command::new("wl-paste")
        .arg("--list-types")
        .output()
        .context("执行 wl-paste --list-types 失败；请确认 wl-clipboard 已安装")?;
    if !types.status.success() {
        return Ok(None);
    }
    let mime = String::from_utf8_lossy(&types.stdout)
        .lines()
        .map(str::trim)
        .find(|mime| {
            mime.eq_ignore_ascii_case("text/plain")
                || mime.eq_ignore_ascii_case("text/plain;charset=utf-8")
                || mime.starts_with("image/")
        })
        .map(str::to_owned);
    let Some(mime) = mime else {
        return Ok(None);
    };
    let output = Command::new("wl-paste")
        .args(["--no-newline", "--type", &mime])
        .output()
        .with_context(|| format!("读取剪贴板 MIME 类型失败: {mime}"))?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(Some(ClipboardSnapshot {
        mime,
        data: output.stdout,
    }))
}

fn copy_text(text: &str) -> Result<()> {
    let mut child = Command::new("wl-copy")
        .args(["--type", "text/plain;charset=utf-8"])
        .stdin(Stdio::piped())
        .spawn()
        .context("执行 wl-copy 失败；请确认 Wayland 会话可用")?;
    child
        .stdin
        .take()
        .context("无法打开 wl-copy stdin")?
        .write_all(text.as_bytes())?;
    let status = child.wait().context("等待 wl-copy 结束失败")?;
    if !status.success() {
        bail!("wl-copy 返回失败状态: {status}");
    }
    Ok(())
}

fn paste_with_available_backend() -> Result<PasteBackend> {
    match Command::new("wtype")
        .args(["-M", "ctrl", "-k", "v", "-m", "ctrl"])
        .status()
    {
        Ok(status) if status.success() => Ok(PasteBackend::Wtype),
        Ok(status) => {
            tracing::warn!(%status, "wtype 粘贴失败，尝试 ydotool");
            paste_with_ydotool()
        }
        Err(error) => {
            tracing::warn!(%error, "wtype 不可用，尝试 ydotool");
            paste_with_ydotool()
        }
    }
}

fn paste_with_ydotool() -> Result<PasteBackend> {
    match Command::new("ydotool")
        .args(["key", "29:1", "47:1", "47:0", "29:0"])
        .status()
    {
        Ok(status) if status.success() => Ok(PasteBackend::Ydotool),
        Ok(status) => {
            tracing::warn!(%status, "ydotool 粘贴失败，保留识别结果在剪贴板");
            Ok(PasteBackend::ClipboardOnly)
        }
        Err(error) => {
            tracing::warn!(%error, "ydotool 不可用，保留识别结果在剪贴板");
            Ok(PasteBackend::ClipboardOnly)
        }
    }
}

fn restore_clipboard(snapshot: Option<ClipboardSnapshot>) -> Result<()> {
    let Some(snapshot) = snapshot else {
        return Ok(());
    };
    let mut child = Command::new("wl-copy")
        .args(["--type", snapshot.mime.as_str()])
        .stdin(Stdio::piped())
        .spawn()
        .context("恢复剪贴板时执行 wl-copy 失败")?;
    child
        .stdin
        .take()
        .context("恢复剪贴板时无法打开 wl-copy stdin")?
        .write_all(&snapshot.data)?;
    let status = child.wait().context("等待剪贴板恢复失败")?;
    if !status.success() {
        bail!("恢复剪贴板的 wl-copy 返回失败状态: {status}");
    }
    Ok(())
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
