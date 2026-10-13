//! 上屏：剪贴板快照 → 写入识别结果 → 模拟粘贴 → 等待 → 恢复原剪贴板。
//!
//! 粘贴后端按顺序尝试：
//! 1. Hyprland：`hyprctl eval` 调用 `send_key_state`，与 Omarchy「通用粘贴」同一路径；
//!    活动窗口带 `terminal` 标签时发送 Ctrl+Shift+V，否则 Ctrl+V。用显式修饰键，
//!    不受用户仍按着的 Super 影响。
//! 2. wtype 虚拟键盘 Ctrl+V。
//! 3. ydotool Ctrl+V。
//! 4. 都失败：保留在剪贴板，由用户手动粘贴（此时不恢复原剪贴板，避免结果丢失）。

use anyhow::{bail, Context, Result};
use std::{
    io::Write,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasteBackend {
    Hyprland,
    Wtype,
    Ydotool,
    /// 未模拟粘贴，结果留在剪贴板。
    ClipboardOnly,
}

impl PasteBackend {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Hyprland => "hyprland",
            Self::Wtype => "wtype",
            Self::Ydotool => "ydotool",
            Self::ClipboardOnly => "剪贴板",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InjectOptions {
    /// 发出粘贴后等待目标应用读取剪贴板的时间。
    pub paste_delay: Duration,
    /// 为 false 时只复制到剪贴板。
    pub auto_paste: bool,
}

impl Default for InjectOptions {
    fn default() -> Self {
        Self {
            paste_delay: Duration::from_millis(200),
            auto_paste: true,
        }
    }
}

/// Omarchy `default/hypr/bindings/clipboard.lua` 的通用粘贴逻辑：
/// 显式修饰键、按下与抬起分开发送（规避 Hyprland send_shortcut 卡键）。
const HYPRLAND_PASTE_LUA: &str = r#"local mods = "CTRL"
local window = hl.get_active_window()
if window then
  for _, tag in ipairs(window.tags or {}) do
    if tag:gsub("%*$", "") == "terminal" then
      mods = "CTRL SHIFT"
    end
  end
end
hl.dispatch(hl.dsp.send_key_state({ mods = mods, key = "V", state = "down" }))
hl.timer(function()
  hl.dispatch(hl.dsp.send_key_state({ mods = mods, key = "V", state = "up" }))
end, { timeout = 50, type = "oneshot" })"#;

/// Hyprland 当前聚焦窗口的地址；不在 Hyprland 或取不到时为 None。
pub fn active_window() -> Option<String> {
    std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?;
    let output = Command::new("hyprctl")
        .args(["activewindow", "-j"])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    value
        .get("address")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

/// 删除光标前的 `count` 个字符（语音修改先删掉上一段，再粘贴改好的版本）。
pub fn delete_chars(count: usize) -> Result<()> {
    if count == 0 {
        return Ok(());
    }
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
        wait_for_released_modifiers(Duration::from_millis(1_500));
        let lua = format!(
            r#"for _ = 1, {count} do
  hl.dispatch(hl.dsp.send_key_state({{ mods = "", key = "BackSpace", state = "down" }}))
  hl.dispatch(hl.dsp.send_key_state({{ mods = "", key = "BackSpace", state = "up" }}))
end"#
        );
        if run_quietly("hyprctl", &["eval", &lua]) {
            return Ok(());
        }
    }
    let mut args = Vec::with_capacity(count * 2);
    for _ in 0..count {
        args.extend(["-k", "BackSpace"]);
    }
    if run_quietly("wtype", &args) {
        return Ok(());
    }
    bail!("无法模拟退格键：需要 Hyprland 或 wtype")
}

/// 用户仍按着的修饰键。切换快捷键（Super+Ctrl+X）松开前就可能已经出结果；此时发送合成按键，
/// 实测会让 fcitx5 虚拟键盘的修饰键状态失去同步，之后的 Enter 被当成 Super+Ctrl+Enter。
const HELD_MODIFIERS_LUA: &str = r#"for _, key in ipairs({ "Super_L", "Super_R", "Control_L", "Control_R", "Shift_L", "Shift_R", "Alt_L", "Alt_R", "ISO_Level3_Shift" }) do
  if hl.is_key_down(key) then return "held" end
end
return "free""#;

/// 等用户松开所有修饰键再发送合成按键；最多等 `timeout`，之后照常发送。
fn wait_for_released_modifiers(timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        let held = Command::new("hyprctl")
            .args(["repl", HELD_MODIFIERS_LUA])
            .stderr(Stdio::null())
            .output()
            .map(|output| {
                output.status.success() && String::from_utf8_lossy(&output.stdout).contains("held")
            })
            .unwrap_or(false);
        if !held {
            return;
        }
        if Instant::now() >= deadline {
            tracing::warn!("修饰键一直按着，仍然发送按键");
            return;
        }
        thread::sleep(Duration::from_millis(25));
    }
}

#[derive(Debug)]
struct ClipboardSnapshot {
    mime: String,
    data: Vec<u8>,
    /// 原内容带 `x-kde-passwordManagerHint`（例如从密码管理器复制的密码）。
    /// 恢复时必须保留这个标记，否则剪贴板历史会把密码记下来。
    sensitive: bool,
}

pub fn inject_text(text: &str, options: &InjectOptions) -> Result<PasteBackend> {
    if text.trim().is_empty() {
        bail!("拒绝上屏空文本");
    }
    let snapshot = snapshot_clipboard().context("保存当前剪贴板失败")?;
    if let Err(error) = copy_text(text) {
        // 写入可能部分成功，尽力恢复原内容。
        return match restore_clipboard(snapshot) {
            Ok(()) => Err(error.context("写入识别结果到剪贴板失败")),
            Err(restore_error) => Err(error.context(format!(
                "写入识别结果到剪贴板失败，且恢复原剪贴板也失败: {restore_error:#}"
            ))),
        };
    }
    if !options.auto_paste {
        return Ok(PasteBackend::ClipboardOnly);
    }
    let backend = paste_with_available_backend();
    if backend == PasteBackend::ClipboardOnly {
        tracing::warn!("所有粘贴后端都不可用，识别结果保留在剪贴板");
        return Ok(backend);
    }
    thread::sleep(options.paste_delay);
    restore_clipboard(snapshot).context("上屏完成，但恢复原剪贴板失败")?;
    Ok(backend)
}

fn snapshot_clipboard() -> Result<Option<ClipboardSnapshot>> {
    let types = Command::new("wl-paste")
        .arg("--list-types")
        .stderr(Stdio::null())
        .output()
        .context("执行 wl-paste --list-types 失败；请确认 wl-clipboard 已安装")?;
    if !types.status.success() {
        // 剪贴板为空时 wl-paste 返回非零。
        return Ok(None);
    }
    let listed = String::from_utf8_lossy(&types.stdout);
    let sensitive = listed
        .lines()
        .any(|mime| mime.trim() == "x-kde-passwordManagerHint");
    let mime = listed
        .lines()
        .map(str::trim)
        .find(|mime| mime.starts_with("text/plain") || mime.starts_with("image/"))
        .map(str::to_owned);
    let Some(mime) = mime else {
        return Ok(None);
    };
    let output = Command::new("wl-paste")
        .args(["--no-newline", "--type", &mime])
        .stderr(Stdio::null())
        .output()
        .with_context(|| format!("读取剪贴板内容失败: {mime}"))?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(Some(ClipboardSnapshot {
        mime,
        data: output.stdout,
        sensitive,
    }))
}

fn wl_copy(args: &[&str], data: &[u8]) -> Result<()> {
    let mut child = Command::new("wl-copy")
        .args(args)
        .stdin(Stdio::piped())
        .spawn()
        .context("执行 wl-copy 失败；请确认 wl-clipboard 已安装且 Wayland 会话可用")?;
    child
        .stdin
        .take()
        .context("无法打开 wl-copy stdin")?
        .write_all(data)
        .context("向 wl-copy 写入数据失败")?;
    let status = child.wait().context("等待 wl-copy 结束失败")?;
    if !status.success() {
        bail!("wl-copy 返回失败状态: {status}");
    }
    Ok(())
}

fn copy_text(text: &str) -> Result<()> {
    // --sensitive：提示剪贴板历史不要记录这条临时内容。
    wl_copy(
        &["--type", "text/plain;charset=utf-8", "--sensitive"],
        text.as_bytes(),
    )
}

fn restore_clipboard(snapshot: Option<ClipboardSnapshot>) -> Result<()> {
    match snapshot {
        Some(snapshot) if snapshot.sensitive => wl_copy(
            &["--type", snapshot.mime.as_str(), "--sensitive"],
            &snapshot.data,
        ),
        Some(snapshot) => wl_copy(&["--type", snapshot.mime.as_str()], &snapshot.data),
        None => wl_copy(&["--clear"], &[]),
    }
}

fn paste_with_available_backend() -> PasteBackend {
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
        wait_for_released_modifiers(Duration::from_millis(1_500));
        match Command::new("hyprctl")
            .args(["eval", HYPRLAND_PASTE_LUA])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
        {
            Ok(status) if status.success() => return PasteBackend::Hyprland,
            Ok(status) => tracing::warn!(%status, "hyprctl 粘贴失败，尝试 wtype"),
            Err(error) => tracing::warn!(%error, "hyprctl 不可用，尝试 wtype"),
        }
    }
    if run_quietly("wtype", &["-M", "ctrl", "-k", "v", "-m", "ctrl"]) {
        return PasteBackend::Wtype;
    }
    if run_quietly("ydotool", &["key", "29:1", "47:1", "47:0", "29:0"]) {
        return PasteBackend::Ydotool;
    }
    PasteBackend::ClipboardOnly
}

fn run_quietly(program: &str, args: &[&str]) -> bool {
    match Command::new(program)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(status) if status.success() => true,
        Ok(status) => {
            tracing::warn!(program, %status, "粘贴后端返回失败");
            false
        }
        Err(error) => {
            tracing::debug!(program, %error, "粘贴后端不可用");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_whitespace_without_touching_clipboard() {
        let error = inject_text(" \n\t", &InjectOptions::default())
            .expect_err("whitespace-only input must be rejected");
        assert_eq!(error.to_string(), "拒绝上屏空文本");
    }

    #[test]
    fn hyprland_paste_mirrors_omarchy_universal_paste() {
        assert!(HYPRLAND_PASTE_LUA.contains("\"terminal\""));
        assert!(HYPRLAND_PASTE_LUA.contains("CTRL SHIFT"));
        assert!(HYPRLAND_PASTE_LUA.contains("state = \"down\""));
        assert!(HYPRLAND_PASTE_LUA.contains("state = \"up\""));
    }
}
