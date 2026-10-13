//! Omarchy 菜单扩展：把语打的条目合并进用户的
//! `~/.config/omarchy/extensions/omarchy-menu.jsonc`，不覆盖用户内容。
//!
//! 语打的条目放在一对标记行之间，安装时先删旧块再插入到最外层 `}` 之前；
//! 写入前按 Omarchy 的 JSONC 规则（整行 `//` 注释 + 尾逗号）解析校验，
//! 解析失败就不写——Omarchy 遇到坏文件会静默丢掉用户的全部扩展。

use anyhow::{bail, Context, Result};
use std::{fs, path::Path};

pub const BEGIN_MARK: &str =
    "  // >>> yuda：以下条目由 omarchy-install-dictation-yuda 管理，卸载时自动删除";
pub const END_MARK: &str = "  // <<< yuda";

/// 语打的菜单条目（每行一个键，均以逗号结尾；Omarchy 的解析器接受尾逗号）。
const ENTRIES: &[&str] = &[
    r#""setup.default.dictation.yuda": {"icon":"󰍬","label":"Yuda 语打（中文）","checked":"[[ \"$(omarchy-default-dictation)\" == \"yuda\" ]]","action":"omarchy-default-dictation yuda"},"#,
    r#""setup.yuda": {"icon":"󰍬","label":"Yuda 语打","title":"语打 · 中文语音输入","when":"command -v yuda >/dev/null"},"#,
    r#""setup.yuda.engine": {"icon":"󰒓","label":"识别引擎","description":"云端豆包大模型 / 离线 SenseVoice / 自动","action":"yuda menu engine"},"#,
    r#""setup.yuda.language": {"icon":"󰗊","label":"识别语言","description":"中文（中英混输）/ English / 粤语 / 日本語 / 한국어","action":"yuda menu language"},"#,
    r#""setup.yuda.fillers": {"icon":"󰗨","label":"去除语气词","checked":"[[ \"$(yuda config get text.remove_fillers)\" == \"true\" ]]","action":"yuda menu fillers"},"#,
    r#""setup.yuda.polish": {"icon":"󰁨","label":"AI 纠错","description":"OpenAI 兼容端点，只修明显识别错误","checked":"[[ \"$(yuda config get llm.enabled)\" == \"true\" ]]","action":"yuda menu polish"},"#,
    r#""setup.yuda.history": {"icon":"󰋚","label":"历史记录 · 重新上屏","action":"yuda menu history"},"#,
    r#""setup.yuda.config": {"icon":"","label":"编辑配置","action":"omarchy-launch-editor ~/.config/yuda/config.toml"},"#,
    r#""setup.yuda.restart": {"icon":"󰜉","label":"重启语打","action":"systemctl --user restart yuda.service"},"#,
    r#""remove.dictation": {"when":"omarchy-pkg-present voxtype-bin || omarchy-pkg-present superwhisper-bin || command -v omarchy-remove-dictation-yuda >/dev/null"},"#,
    r#""remove.dictation.yuda": {"icon":"󰍬","label":"Yuda 语打","when":"command -v omarchy-remove-dictation-yuda >/dev/null","action":"omarchy-launch-floating-terminal-with-presentation omarchy-remove-dictation-yuda"},"#,
];

/// 返回插入语打条目后的文件内容。
pub fn install(raw: &str) -> Result<String> {
    let base = remove(raw);
    let mut lines: Vec<String> = base.lines().map(str::to_owned).collect();
    let Some(close) = lines
        .iter()
        .rposition(|line| line.trim_end().ends_with('}'))
    else {
        // 文件为空或只有注释：新建对象。
        let mut output = String::new();
        if !base.trim().is_empty() {
            output.push_str(base.trim_end());
            output.push('\n');
        }
        output.push_str("{\n");
        output.push_str(&block());
        output.push_str("}\n");
        return validated(output);
    };
    // 最外层右括号所在行可能是 `}` 或 `  "x": {...}}`；只处理独占一行的情况。
    if lines[close].trim() != "}" {
        bail!("无法安全修改菜单扩展：最外层的 }} 必须独占一行");
    }
    // 保证插入点之前的最后一个有效条目以逗号结尾。
    if let Some(previous) = lines[..close]
        .iter()
        .rposition(|line| !line.trim().is_empty() && !line.trim_start().starts_with("//"))
    {
        let trimmed = lines[previous].trim_end().to_owned();
        if !trimmed.ends_with('{') && !trimmed.ends_with(',') {
            lines[previous] = format!("{trimmed},");
        }
    }
    let block = block();
    lines.insert(close, block.trim_end_matches('\n').to_owned());
    let mut output = lines.join("\n");
    output.push('\n');
    validated(output)
}

/// 删除语打的条目块（没有则原样返回）。
pub fn remove(raw: &str) -> String {
    let mut output = String::with_capacity(raw.len());
    let mut skipping = false;
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed == BEGIN_MARK.trim() {
            skipping = true;
            continue;
        }
        if skipping {
            if trimmed == END_MARK.trim() {
                skipping = false;
            }
            continue;
        }
        output.push_str(line);
        output.push('\n');
    }
    output
}

fn block() -> String {
    let mut block = String::new();
    block.push_str(BEGIN_MARK);
    block.push('\n');
    for entry in ENTRIES {
        block.push_str("  ");
        block.push_str(entry);
        block.push('\n');
    }
    block.push_str(END_MARK);
    block.push('\n');
    block
}

fn validated(output: String) -> Result<String> {
    parse_jsonc(&output).context("合并后的菜单扩展无法解析，已放弃写入")?;
    Ok(output)
}

/// 按 Omarchy 菜单的规则解析 JSONC：删除整行 `//` 注释与尾逗号。
pub fn parse_jsonc(raw: &str) -> Result<serde_json::Value> {
    let without_comments: String = raw
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut cleaned = String::with_capacity(without_comments.len());
    let mut chars = without_comments.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    while let Some(c) = chars.next() {
        if in_string {
            cleaned.push(c);
            match (escaped, c) {
                (true, _) => escaped = false,
                (false, '\\') => escaped = true,
                (false, '"') => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                cleaned.push(c);
            }
            ',' => {
                let rest: String = chars.clone().collect();
                let next = rest.trim_start().chars().next();
                if !matches!(next, Some('}' | ']')) {
                    cleaned.push(c);
                }
            }
            _ => cleaned.push(c),
        }
    }
    serde_json::from_str(&cleaned).context("JSONC 语法错误")
}

/// 修改用户的菜单扩展文件；原文件先备份到 `.yuda-backup`。
pub fn update_file(path: &Path, install_entries: bool) -> Result<()> {
    let raw = if path.exists() {
        fs::read_to_string(path).with_context(|| format!("读取菜单扩展失败: {}", path.display()))?
    } else {
        String::new()
    };
    if !raw.trim().is_empty() {
        parse_jsonc(&raw)
            .with_context(|| format!("现有菜单扩展已经无法解析，请先修复: {}", path.display()))?;
    }
    let updated = if install_entries {
        install(&raw)?
    } else {
        validated(remove(&raw))?
    };
    if updated == raw {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if path.exists() {
        fs::copy(path, path.with_extension("jsonc.yuda-backup")).context("备份菜单扩展失败")?;
    }
    let temporary = path.with_extension("jsonc.yuda-tmp");
    fs::write(&temporary, &updated).context("写入菜单扩展失败")?;
    fs::rename(&temporary, path).context("替换菜单扩展失败")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHIPPED: &str = "{\n  // Extend the Quickshell Omarchy menu with JSONC.\n  //\n  // \"personal\": {\"icon\":\"\",\"label\":\"Personal\"},\n}\n";

    fn keys(raw: &str) -> Vec<String> {
        parse_jsonc(raw)
            .expect("valid jsonc")
            .as_object()
            .expect("object")
            .keys()
            .cloned()
            .collect()
    }

    #[test]
    fn installs_into_the_shipped_comment_only_file() {
        let installed = install(SHIPPED).expect("install");
        assert!(installed.contains("// Extend the Quickshell Omarchy menu"));
        let keys = keys(&installed);
        assert!(keys.contains(&"setup.default.dictation.yuda".to_owned()));
        assert!(keys.contains(&"remove.dictation.yuda".to_owned()));
    }

    #[test]
    fn keeps_user_entries_and_adds_missing_comma() {
        let user = "{\n  \"personal\": {\"icon\":\"\",\"label\":\"Personal\"}\n}\n";
        let installed = install(user).expect("install");
        let keys = keys(&installed);
        assert!(keys.contains(&"personal".to_owned()));
        assert!(keys.contains(&"setup.yuda".to_owned()));
    }

    #[test]
    fn install_is_idempotent_and_remove_restores_original() {
        let once = install(SHIPPED).expect("install");
        let twice = install(&once).expect("reinstall");
        assert_eq!(once, twice);
        assert_eq!(remove(&twice), SHIPPED);
    }

    #[test]
    fn creates_object_for_empty_file() {
        let installed = install("").expect("install");
        assert!(keys(&installed).contains(&"setup.yuda.engine".to_owned()));
    }

    #[test]
    fn refuses_files_it_cannot_edit_safely() {
        assert!(install("{\"a\": {\"b\": 1}}\n").is_err());
    }

    #[test]
    fn jsonc_parser_keeps_commas_and_slashes_inside_strings() {
        let value = parse_jsonc("{\n  // c\n  \"a\": \"x, }\",\n  \"b\": \"http://e\",\n}\n")
            .expect("parse");
        assert_eq!(value["a"], "x, }");
        assert_eq!(value["b"], "http://e");
    }

    #[test]
    fn update_file_backs_up_and_round_trips() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("extensions/omarchy-menu.jsonc");
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(&path, SHIPPED).expect("write");
        update_file(&path, true).expect("install");
        assert!(fs::read_to_string(&path)
            .expect("read")
            .contains(BEGIN_MARK));
        assert!(path.with_extension("jsonc.yuda-backup").exists());
        update_file(&path, false).expect("remove");
        assert_eq!(fs::read_to_string(&path).expect("read"), SHIPPED);
    }
}
