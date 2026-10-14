//! 语音修改：把口头指令应用到「上一段上屏的文字」。
//!
//! 用户按住修改键（Shift+F9）说「把九点改成十点」「删掉上一句」「最后加上问号」，
//! 语打删除上一段并粘贴改好的版本。规则能识别的指令本地处理；其他自由指令
//! （「改得正式一点」「翻译成英文」）在配置了 LLM 时交给 LLM。纯逻辑，可单测。

/// 解析出的修改动作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditOp {
    Replace {
        from: String,
        to: String,
    },
    Remove(String),
    /// 撤回整段。
    DeleteAll,
    Append(String),
    Prepend(String),
    /// 规则无法理解的指令。
    Freeform(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    /// 上一段里找不到要修改的文字。
    NotFound(String),
    /// 需要 LLM 才能执行的自由指令。
    NeedsLlm(String),
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(target) => write!(f, "上一段里没有「{target}」"),
            Self::NeedsLlm(instruction) => write!(
                f,
                "没听懂修改指令「{instruction}」；开启 AI 纠错后可以使用自由指令"
            ),
        }
    }
}

const REPLACE_VERBS: [&str; 10] = [
    "替换成",
    "替换为",
    "修改成",
    "修改为",
    "改成",
    "改为",
    "换成",
    "换为",
    "变成",
    "写成",
];
const REMOVE_VERBS: [&str; 4] = ["删掉", "删除", "去掉", "删去"];
const UNDO_PHRASES: [&str; 6] = ["撤销", "撤回", "清空", "全部删掉", "全部删除", "都删掉"];
const UNDO_TARGETS: [&str; 9] = [
    "上一句",
    "上句",
    "刚才的",
    "刚才那句",
    "刚才说的",
    "这句",
    "这句话",
    "全部",
    "所有",
];
const APPEND_PLACES: [&str; 5] = ["最后", "末尾", "结尾", "后面", "句末"];
const PREPEND_PLACES: [&str; 3] = ["开头", "最前面", "句首"];
const ADD_VERBS: [&str; 7] = ["加上", "加一个", "加个", "加", "添加", "补上", "补充"];

/// 解析一条修改指令（已经过识别后处理的文本）。
pub fn parse(instruction: &str) -> EditOp {
    let text = strip_edges(instruction);
    if let Some(op) = parse_english(&text) {
        return op;
    }
    let body = text.strip_prefix("请").unwrap_or(&text).trim();
    let object = body.strip_prefix('把').or_else(|| body.strip_prefix('将'));

    // 「上一句删掉」：识别常吞掉句首的「把」，这里不要求有「把」。
    let undo_object = object.unwrap_or(body);
    if UNDO_PHRASES.contains(&body)
        || REMOVE_VERBS.iter().any(|verb| {
            UNDO_TARGETS
                .iter()
                .any(|target| body == format!("{verb}{target}"))
        })
        || UNDO_TARGETS.iter().any(|target| {
            REMOVE_VERBS
                .iter()
                .any(|verb| undo_object == format!("{target}{verb}"))
        })
    {
        return EditOp::DeleteAll;
    }

    if let Some(rest) = object {
        // 「把 A 改成 B」：取第一个出现的动词，A 不能为空。
        if let Some((from, to)) = split_on_first(rest, &REPLACE_VERBS) {
            if !from.is_empty() {
                return EditOp::Replace {
                    from: from.to_owned(),
                    to: strip_edges(to),
                };
            }
        }
        // 「把 A 删掉」
        for verb in REMOVE_VERBS {
            if let Some(target) = rest.strip_suffix(verb) {
                if !target.is_empty() {
                    return EditOp::Remove(target.to_owned());
                }
            }
        }
    }

    for verb in REMOVE_VERBS {
        if let Some(target) = body.strip_prefix(verb) {
            let target = strip_edges(target);
            if !target.is_empty() {
                return EditOp::Remove(target);
            }
        }
    }

    let located = body.strip_prefix('在').unwrap_or(body);
    for (places, append) in [(&APPEND_PLACES[..], true), (&PREPEND_PLACES[..], false)] {
        for place in places {
            if let Some(rest) = located.strip_prefix(place) {
                for verb in ADD_VERBS {
                    if let Some(addition) = rest.strip_prefix(verb) {
                        let addition = spoken_punctuation(&strip_edges(addition));
                        if !addition.is_empty() {
                            return if append {
                                EditOp::Append(addition)
                            } else {
                                EditOp::Prepend(addition)
                            };
                        }
                    }
                }
            }
        }
    }

    EditOp::Freeform(text)
}

fn parse_english(text: &str) -> Option<EditOp> {
    let lower = text.to_lowercase();
    if [
        "undo",
        "undo that",
        "scratch that",
        "delete that",
        "delete all",
    ]
    .contains(&lower.as_str())
    {
        return Some(EditOp::DeleteAll);
    }
    for verb in ["replace ", "change "] {
        if let Some(rest) = lower.strip_prefix(verb) {
            for joiner in [" with ", " to "] {
                if let Some(at) = rest.find(joiner) {
                    // 用原文切片保留大小写。
                    let offset = verb.len();
                    let from = text[offset..offset + at].trim();
                    let to = text[offset + at + joiner.len()..].trim();
                    if !from.is_empty() {
                        return Some(EditOp::Replace {
                            from: from.to_owned(),
                            to: to.to_owned(),
                        });
                    }
                }
            }
        }
    }
    for verb in ["delete ", "remove "] {
        if lower.starts_with(verb) {
            let target = text[verb.len()..].trim();
            if !target.is_empty() {
                return Some(EditOp::Remove(target.to_owned()));
            }
        }
    }
    None
}

/// 把指令应用到上一段文字。返回 `Ok(None)` 表示整段删除。
pub fn apply(op: &EditOp, last: &str) -> Result<Option<String>, EditError> {
    match op {
        EditOp::DeleteAll => Ok(None),
        EditOp::Replace { from, to } => {
            let (from, to) = locate(last, from, to)?;
            Ok(Some(last.replace(&from, &to)))
        }
        EditOp::Remove(target) => {
            let (target, _) = locate(last, target, "")?;
            Ok(Some(last.replace(&target, "")))
        }
        EditOp::Append(addition) => Ok(Some(append(last, addition))),
        EditOp::Prepend(addition) => Ok(Some(format!("{addition}{last}"))),
        EditOp::Freeform(instruction) => {
            bare_edit(instruction, last).ok_or_else(|| EditError::NeedsLlm(instruction.clone()))
        }
    }
}

/// 识别常吞掉句首的「把」，「把九点改成十点」变成「九点改成十点」。不带「把」的
/// 「A 改成 B」「A 删掉」只在 A 确实出现在上一段里时按替换或删除处理；否则仍是
/// 自由指令（「语气改成正式一点」交给 LLM）。
fn bare_edit(instruction: &str, last: &str) -> Option<Option<String>> {
    let body = instruction.strip_prefix('请').unwrap_or(instruction).trim();
    let op = match split_on_first(body, &REPLACE_VERBS) {
        Some((from, to)) => EditOp::Replace {
            from: from.to_owned(),
            to: strip_edges(to),
        },
        None => EditOp::Remove(
            REMOVE_VERBS
                .iter()
                .find_map(|verb| body.strip_suffix(verb))?
                .trim()
                .to_owned(),
        ),
    };
    match &op {
        EditOp::Replace { from, to } if from.is_empty() || to.is_empty() => None,
        EditOp::Remove(target) if target.is_empty() => None,
        _ => apply(&op, last).ok(),
    }
}

/// 在上一段里找到要改的文字；识别结果的数字写法可能不一致（「九点」与「9点」），
/// 依次尝试原文、汉字数字转阿拉伯数字，并对替换后的文字做同样转换。
fn locate(last: &str, from: &str, to: &str) -> Result<(String, String), EditError> {
    if last.contains(from) {
        return Ok((from.to_owned(), to.to_owned()));
    }
    let digits = chinese_numerals_to_digits(from);
    if digits != from && last.contains(&digits) {
        return Ok((digits, chinese_numerals_to_digits(to)));
    }
    let trimmed = strip_edges(from);
    if !trimmed.is_empty() && trimmed != from && last.contains(&trimmed) {
        return Ok((trimmed, to.to_owned()));
    }
    Err(EditError::NotFound(from.to_owned()))
}

fn append(last: &str, addition: &str) -> String {
    let is_mark = |c: char| "。，！？；：.,!?;:".contains(c);
    let addition_is_mark = addition.chars().count() == 1 && addition.chars().all(is_mark);
    if addition_is_mark {
        // 「最后加上问号」：替换掉原来的句末标点。
        let base = last.trim_end_matches(is_mark);
        return format!("{base}{addition}");
    }
    format!("{last}{addition}")
}

/// 去掉首尾空白与标点（识别结果常在指令末尾带句号）。
fn strip_edges(text: &str) -> String {
    text.trim()
        .trim_matches(|c: char| "。，！？、；：.,!?;:「」“”\"'".contains(c) || c.is_whitespace())
        .to_owned()
}

fn split_on_first<'a>(text: &'a str, verbs: &[&str]) -> Option<(&'a str, &'a str)> {
    verbs
        .iter()
        .filter_map(|verb| text.find(verb).map(|at| (at, verb.len())))
        .min_by_key(|(at, len)| (*at, std::cmp::Reverse(*len)))
        .map(|(at, len)| (text[..at].trim(), text[at + len..].trim()))
}

/// 口述的标点名转成符号：「句号」→「。」。
fn spoken_punctuation(text: &str) -> String {
    match text {
        "句号" => "。",
        "逗号" => "，",
        "问号" => "？",
        "感叹号" | "叹号" => "！",
        "冒号" => "：",
        "分号" => "；",
        "顿号" => "、",
        other => other,
    }
    .to_owned()
}

/// 把 0–99 的汉字数字转成阿拉伯数字：「九点」→「9点」，「二十五」→「25」。
pub fn chinese_numerals_to_digits(text: &str) -> String {
    fn digit(c: char) -> Option<u32> {
        "零一二三四五六七八九"
            .chars()
            .position(|d| d == c)
            .map(|p| p as u32)
            .or(if c == '两' { Some(2) } else { None })
    }
    let chars: Vec<char> = text.chars().collect();
    let mut output = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        let after = chars.get(i + 2).copied();
        match (digit(c), c) {
            // 「二十五」「二十」
            (Some(tens), _) if next == Some('十') => {
                let ones = after.and_then(digit);
                output.push_str(&(tens * 10 + ones.unwrap_or(0)).to_string());
                i += if ones.is_some() { 3 } else { 2 };
            }
            // 「十五」「十」
            (None, '十') => {
                let ones = next.and_then(digit);
                output.push_str(&(10 + ones.unwrap_or(0)).to_string());
                i += if ones.is_some() { 2 } else { 1 };
            }
            (Some(value), _) => {
                output.push_str(&value.to_string());
                i += 1;
            }
            _ => {
                output.push(c);
                i += 1;
            }
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn replace(from: &str, to: &str) -> EditOp {
        EditOp::Replace {
            from: from.to_owned(),
            to: to.to_owned(),
        }
    }

    #[test]
    fn parses_replace_instructions() {
        assert_eq!(parse("把9点改成10点。"), replace("9点", "10点"));
        assert_eq!(parse("请将张三换成李四"), replace("张三", "李四"));
        assert_eq!(parse("把配森替换成Python"), replace("配森", "Python"));
        assert_eq!(
            parse("Replace Monday with Tuesday."),
            replace("Monday", "Tuesday")
        );
    }

    #[test]
    fn parses_removal_and_undo() {
        assert_eq!(parse("删掉上一句。"), EditOp::DeleteAll);
        assert_eq!(parse("撤销"), EditOp::DeleteAll);
        assert_eq!(parse("把这句话删掉"), EditOp::DeleteAll);
        assert_eq!(parse("scratch that"), EditOp::DeleteAll);
        assert_eq!(parse("删掉“非常”"), EditOp::Remove("非常".to_owned()));
        assert_eq!(parse("把其实删掉"), EditOp::Remove("其实".to_owned()));
    }

    #[test]
    fn parses_additions_with_spoken_punctuation() {
        assert_eq!(parse("最后加上问号"), EditOp::Append("？".to_owned()));
        assert_eq!(parse("在末尾加上谢谢。"), EditOp::Append("谢谢".to_owned()));
        assert_eq!(parse("开头加上你好，"), EditOp::Prepend("你好".to_owned()));
    }

    #[test]
    fn unknown_instructions_are_freeform() {
        assert_eq!(
            parse("改得正式一点。"),
            EditOp::Freeform("改得正式一点".to_owned())
        );
        assert_eq!(
            parse("翻译成英文"),
            EditOp::Freeform("翻译成英文".to_owned())
        );
    }

    #[test]
    fn applies_edits_to_the_last_text() {
        let last = "开饭时间早上9点至下午5点。";
        assert_eq!(
            apply(&parse("把9点改成10点"), last).expect("replace"),
            Some("开饭时间早上10点至下午5点。".to_owned())
        );
        assert_eq!(apply(&EditOp::DeleteAll, last).expect("undo"), None);
        assert_eq!(
            apply(&parse("最后加上问号"), last).expect("append"),
            Some("开饭时间早上9点至下午5点？".to_owned())
        );
        assert_eq!(
            apply(&parse("删掉开饭"), last).expect("remove"),
            Some("时间早上9点至下午5点。".to_owned())
        );
    }

    #[test]
    fn matches_spoken_numerals_against_itn_digits() {
        let last = "开饭时间早上9点至下午5点。";
        assert_eq!(
            apply(&parse("把九点改成十点"), last).expect("numeral replace"),
            Some("开饭时间早上10点至下午5点。".to_owned())
        );
        assert_eq!(
            chinese_numerals_to_digits("二十五号下午两点"),
            "25号下午2点"
        );
        assert_eq!(chinese_numerals_to_digits("十一点半"), "11点半");
    }

    #[test]
    fn accepts_instructions_whose_leading_ba_was_not_recognized() {
        let last = "开饭时间早上9点至下午5点。";
        assert_eq!(
            apply(&parse("九点改成十点。"), last).expect("bare replace"),
            Some("开饭时间早上10点至下午5点。".to_owned())
        );
        assert_eq!(
            apply(&parse("开饭删掉"), last).expect("bare remove"),
            Some("时间早上9点至下午5点。".to_owned())
        );
        assert_eq!(parse("上一句删掉"), EditOp::DeleteAll);
        // 目标不在上一段里：仍是自由指令，开启 AI 纠错时交给 LLM。
        assert!(matches!(
            apply(&parse("语气改成正式一点"), last),
            Err(EditError::NeedsLlm(_))
        ));
        assert!(matches!(
            apply(&parse("改成英文"), last),
            Err(EditError::NeedsLlm(_))
        ));
    }

    #[test]
    fn reports_missing_targets_and_freeform_without_llm() {
        let last = "今天开会。";
        assert_eq!(
            apply(&parse("把明天改成后天"), last),
            Err(EditError::NotFound("明天".to_owned()))
        );
        assert!(matches!(
            apply(&parse("改得正式一点"), last),
            Err(EditError::NeedsLlm(_))
        ));
    }

    #[test]
    fn content_that_merely_mentions_change_is_not_an_edit_in_dictation_mode() {
        // 语音修改只在修改模式下解析；这里只确认解析本身不会吞掉主语。
        assert_eq!(
            parse("我们需要把接口改成异步的"),
            EditOp::Freeform("我们需要把接口改成异步的".to_owned())
        );
    }
}
