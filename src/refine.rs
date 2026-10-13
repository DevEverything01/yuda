//! 保守纠错：OpenAI 兼容的 chat/completions，只修明显的识别错误。
//! 超时或输出偏离原文时，daemon 直接上屏原文（不阻塞上屏）。

use anyhow::{Context, Result};
use std::time::Duration;

use crate::config::LlmConfig;

/// PROMPT.md 规定的系统提示词，逐字使用。
pub const SYSTEM_PROMPT: &str = "You are a speech-to-text correction filter. Only fix obvious speech-recognition errors: Chinese homophone mistakes (e.g. 他/她/它 by context) and English technical terms mistakenly transcribed as Chinese (配森→Python, 杰森→JSON, 吉特→Git). Never rewrite, polish, translate, add or remove content. If the input looks correct, return it exactly as-is. Return only the corrected text, no explanations.";

/// 语音修改的自由指令（「改得正式一点」「翻译成英文」）。
pub const EDIT_PROMPT: &str = "You edit a short piece of dictated text by following the user's instruction. Apply the instruction to TEXT and return only the resulting text, with no quotes, labels or explanations. Keep everything the instruction does not ask to change, and keep the language of TEXT unless the instruction asks to translate.";

pub fn refine(config: &LlmConfig, text: &str) -> Result<String> {
    let content = chat(config, SYSTEM_PROMPT, text)?;
    accept(text, &content).context("LLM 输出偏离原文过多，已使用原文")
}

/// 按自由指令改写上一段文字；改写是用户明确要求的，不做偏离检查。
pub fn rewrite(config: &LlmConfig, text: &str, instruction: &str) -> Result<String> {
    let user = format!("TEXT:\n{text}\n\nINSTRUCTION:\n{instruction}");
    let content = chat(config, EDIT_PROMPT, &user)?;
    let content = content
        .trim()
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    if content.is_empty() {
        anyhow::bail!("LLM 没有返回改写结果");
    }
    Ok(content.to_owned())
}

fn chat(config: &LlmConfig, system: &str, user: &str) -> Result<String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_millis(config.timeout_ms)))
        .build()
        .into();
    let url = format!("{}/chat/completions", config.api_base.trim_end_matches('/'));
    let body = serde_json::json!({
        "model": config.model,
        "temperature": 0,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user },
        ],
    });
    let mut request = agent.post(&url);
    if !config.api_key.trim().is_empty() {
        request = request.header("Authorization", format!("Bearer {}", config.api_key.trim()));
    }
    let mut response = request.send_json(&body).context("LLM 请求失败")?;
    let value: serde_json::Value = response
        .body_mut()
        .read_json()
        .context("LLM 返回无法解析")?;
    value
        .pointer("/choices/0/message/content")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .context("LLM 返回中没有文本")
}

/// 只接受「看起来像同一句话」的纠错结果。
pub fn accept(original: &str, refined: &str) -> Option<String> {
    let refined = refined
        .trim()
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim()
        .trim_matches(|c| c == '"' || c == '“' || c == '”')
        .trim();
    if refined.is_empty() {
        return None;
    }
    if refined.contains('\n') && !original.contains('\n') {
        return None;
    }
    let before = units(original);
    let after = units(refined);
    let drift = before.abs_diff(after);
    let allowed = (before * 3 / 10).max(4);
    (drift <= allowed).then(|| refined.to_owned())
}

/// 长度单位：一个英文单词或数字串算 1，其他非空白字符各算 1。
/// 这样「配森 → Python」不会被误判为大幅改写。
fn units(text: &str) -> usize {
    let mut count = 0;
    let mut in_word = false;
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            if !in_word {
                count += 1;
                in_word = true;
            }
        } else {
            in_word = false;
            if !c.is_whitespace() {
                count += 1;
            }
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_small_fixes() {
        assert_eq!(
            accept("用配森解析杰森", "用 Python 解析 JSON").as_deref(),
            Some("用 Python 解析 JSON")
        );
        assert_eq!(accept("好的。", "好的。").as_deref(), Some("好的。"));
        assert_eq!(accept("他来了", "“她来了”").as_deref(), Some("她来了"));
    }

    #[test]
    fn rejects_rewrites_and_explanations() {
        assert_eq!(accept("今天下午三点开会", ""), None);
        assert_eq!(
            accept(
                "今天下午三点开会",
                "今天下午三点开会。\n说明：原文没有错误，我只添加了句号。"
            ),
            None
        );
        assert_eq!(accept("今天下午三点在会议室讨论发布计划", "会议"), None);
    }
}
