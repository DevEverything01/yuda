//! 识别结果的本地后处理：语气词过滤、用户词库替换、分段拼接与标点整理。
//!
//! 规则保守：只删除几乎不可能是正文的语气词，绝不改写句子内容。

use crate::config::TextConfig;
use std::collections::BTreeMap;

/// 任何位置出现都只会是语气词的字符。
const ALWAYS_FILLER: [char; 2] = ['嗯', '呃'];
/// 只有独立成句（被标点包围）时才视为语气词：「额外」「好啊」不受影响。
const STANDALONE_FILLERS: [&str; 9] = ["额", "啊", "唔", "um", "uh", "umm", "uhm", "erm", "hmm"];

/// 语气词判断用的子句分隔符；只用于切分，拼回时原样保留。
const CLAUSE_BREAKS: [char; 13] = [
    '，', ',', '、', '。', '.', '！', '？', '!', '?', '；', ';', '：', ':',
];
const SENTENCE_ENDS: [char; 6] = ['。', '.', '！', '？', '!', '?'];
/// 整理时合并的中文标点；不含英文 `.`，避免破坏省略号、小数与域名。
const CJK_MARKS: [char; 7] = ['，', '、', '。', '！', '？', '；', '：'];
const CJK_ENDS: [char; 3] = ['。', '！', '？'];

/// 依照配置处理一段最终识别文本。
pub fn process(text: &str, config: &TextConfig) -> String {
    let mut output = text.trim().to_owned();
    if config.remove_fillers {
        output = remove_fillers(&output);
    }
    if !config.replacements.is_empty() {
        output = apply_replacements(&output, &config.replacements);
    }
    tidy(&output)
}

/// 删除语气词。
pub fn remove_fillers(text: &str) -> String {
    let stripped: String = text
        .chars()
        .filter(|c| !ALWAYS_FILLER.contains(c))
        .collect();
    let mut kept: Vec<(String, Option<char>)> = Vec::new();
    for (clause, delimiter) in split_clauses(&stripped) {
        if is_standalone_filler(&clause) {
            // 被删掉的子句如果带句末标点，把它交给前一个保留子句，避免句子失去结尾。
            if let (Some(end), Some(previous)) = (delimiter, kept.last_mut()) {
                if SENTENCE_ENDS.contains(&end) {
                    previous.1 = Some(end);
                }
            }
            continue;
        }
        kept.push((clause, delimiter));
    }
    let mut output = String::with_capacity(stripped.len());
    for (clause, delimiter) in kept {
        output.push_str(&clause);
        if let Some(delimiter) = delimiter {
            output.push(delimiter);
        }
    }
    output
}

fn split_clauses(text: &str) -> Vec<(String, Option<char>)> {
    let mut clauses = Vec::new();
    let mut current = String::new();
    for c in text.chars() {
        if CLAUSE_BREAKS.contains(&c) {
            clauses.push((std::mem::take(&mut current), Some(c)));
        } else {
            current.push(c);
        }
    }
    if !current.is_empty() {
        clauses.push((current, None));
    }
    clauses
}

fn is_standalone_filler(clause: &str) -> bool {
    let trimmed = clause.trim();
    if trimmed.is_empty() {
        // 只剩标点的子句（例如删掉「嗯」后的「，」）。
        return true;
    }
    let lower = trimmed.to_lowercase();
    // 「啊啊」「额额」这样的重复也算。
    STANDALONE_FILLERS.iter().any(|filler| {
        let mut rest = lower.as_str();
        while let Some(next) = rest.strip_prefix(filler) {
            rest = next.trim_start();
        }
        rest.is_empty()
    })
}

/// 用户词库替换：每个位置取最长匹配，替换结果不再参与匹配。
pub fn apply_replacements(text: &str, replacements: &BTreeMap<String, String>) -> String {
    let mut keys: Vec<&String> = replacements.keys().filter(|key| !key.is_empty()).collect();
    keys.sort_by_key(|key| std::cmp::Reverse(key.len()));
    let mut output = String::with_capacity(text.len());
    let mut rest = text;
    'outer: while !rest.is_empty() {
        for key in &keys {
            if let Some(after) = rest.strip_prefix(key.as_str()) {
                output.push_str(&replacements[*key]);
                rest = after;
                continue 'outer;
            }
        }
        let mut chars = rest.chars();
        if let Some(c) = chars.next() {
            output.push(c);
        }
        rest = chars.as_str();
    }
    output
}

/// 拼接分段识别结果：英文单词与英文句末标点之后补一个空格，中文直接相连。
pub fn join_segments(left: &str, right: &str) -> String {
    let left = left.trim_end();
    let right = right.trim_start();
    if left.is_empty() {
        return right.to_owned();
    }
    if right.is_empty() {
        return left.to_owned();
    }
    let needs_space = match (left.chars().last(), right.chars().next()) {
        (Some(a), Some(b)) => {
            b.is_ascii_alphanumeric() && (a.is_ascii_alphanumeric() || ".,!?;:".contains(a))
        }
        _ => false,
    };
    if needs_space {
        format!("{left} {right}")
    } else {
        format!("{left}{right}")
    }
}

/// 整理标点：去掉开头的标点，合并重复或相邻的中文标点。
pub fn tidy(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    for c in text.trim().chars() {
        let is_mark = CJK_MARKS.contains(&c) || (output.is_empty() && ",.;:".contains(c));
        if is_mark && output.trim().is_empty() {
            output.clear();
            continue;
        }
        if is_mark {
            if let Some(previous) = output.chars().last() {
                if CJK_MARKS.contains(&previous) {
                    // 「，。」→「。」，「。。」→「。」；保留更强的那个。
                    if CJK_ENDS.contains(&c) && !CJK_ENDS.contains(&previous) {
                        output.pop();
                        output.push(c);
                    }
                    continue;
                }
            }
        }
        output.push(c);
    }
    output.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_interjections_without_touching_words() {
        assert_eq!(remove_fillers("嗯，好的。"), "好的。");
        assert_eq!(remove_fillers("嗯嗯我觉得可以。"), "我觉得可以。");
        assert_eq!(remove_fillers("这个呃方案不错"), "这个方案不错");
        assert_eq!(
            remove_fillers("额外的费用，额头有点烫"),
            "额外的费用，额头有点烫"
        );
        assert_eq!(remove_fillers("好啊，那就这样。"), "好啊，那就这样。");
        assert_eq!(remove_fillers("啊，我想起来了。"), "我想起来了。");
    }

    #[test]
    fn standalone_filler_at_sentence_end_keeps_the_period() {
        assert_eq!(remove_fillers("明天见，额。"), "明天见。");
        assert_eq!(remove_fillers("Let's ship it, um."), "Let's ship it.");
    }

    #[test]
    fn english_fillers_only_as_whole_clauses() {
        assert_eq!(remove_fillers("Um, I think so."), " I think so.".to_owned());
        assert_eq!(tidy(&remove_fillers("Um, I think so.")), "I think so.");
        assert_eq!(remove_fillers("umbrella, uhm, yes"), "umbrella, yes");
    }

    #[test]
    fn replacements_prefer_longest_match_and_do_not_cascade() {
        let mut map = BTreeMap::new();
        map.insert("配森".to_owned(), "Python".to_owned());
        map.insert("杰森".to_owned(), "JSON".to_owned());
        map.insert("杰森文件".to_owned(), "JSON 文件".to_owned());
        map.insert("Python".to_owned(), "蟒蛇".to_owned());
        assert_eq!(
            apply_replacements("用配森解析杰森文件和杰森", &map),
            "用Python解析JSON 文件和JSON"
        );
    }

    #[test]
    fn joins_segments_by_script() {
        assert_eq!(
            join_segments("今天开会。", "下午三点"),
            "今天开会。下午三点"
        );
        assert_eq!(
            join_segments("Hello world.", "This is fine"),
            "Hello world. This is fine"
        );
        assert_eq!(join_segments("用 Python", "and Rust"), "用 Python and Rust");
        assert_eq!(join_segments("", "开始"), "开始");
    }

    #[test]
    fn tidy_merges_adjacent_punctuation() {
        assert_eq!(tidy("，，好的，。"), "好的。");
        assert_eq!(tidy("我们，，明天。。见"), "我们，明天。见");
    }

    #[test]
    fn process_applies_the_configured_pipeline() {
        let mut config = TextConfig::default();
        config
            .replacements
            .insert("配森".to_owned(), "Python".to_owned());
        assert_eq!(
            process(" 嗯，用配森写个脚本。 ", &config),
            "用Python写个脚本。"
        );
        config.remove_fillers = false;
        assert_eq!(process("嗯，好的。", &config), "嗯，好的。");
    }
}
