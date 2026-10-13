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

/// 常见技术名词的规范写法。识别模型常输出全小写或拆开的写法（python、mac book）；
/// 只按完整英文单词匹配，不改中文，也不改普通英文单词（main、state、bug 保持原样）。
const TERMS: &[&str] = &[
    "AI",
    "API",
    "APIs",
    "AWS",
    "Android",
    "Bash",
    "C++",
    "CI",
    "CPU",
    "CSS",
    "ChatGPT",
    "Chrome",
    "Claude",
    "Docker",
    "Excel",
    "Figma",
    "Firefox",
    "GPU",
    "Git",
    "GitHub",
    "GitLab",
    "Go",
    "Golang",
    "Google",
    "GraphQL",
    "HTML",
    "HTTP",
    "HTTPS",
    "Hyprland",
    "ID",
    "iOS",
    "iPad",
    "iPhone",
    "Java",
    "JavaScript",
    "JSON",
    "Kubernetes",
    "Linux",
    "LLM",
    "Lua",
    "MacBook",
    "macOS",
    "Markdown",
    "MySQL",
    "Neovim",
    "Nginx",
    "Node",
    "Node.js",
    "Notion",
    "npm",
    "Omarchy",
    "OpenAI",
    "PDF",
    "PostgreSQL",
    "PPT",
    "PR",
    "Python",
    "React",
    "Redis",
    "Rust",
    "SDK",
    "Slack",
    "SQL",
    "SSH",
    "TypeScript",
    "UI",
    "URL",
    "UX",
    "Vim",
    "VS Code",
    "Vue",
    "Wayland",
    "WiFi",
    "Windows",
    "Word",
    "XML",
    "YAML",
    "Zoom",
];

/// 依照配置处理一段最终识别文本。
pub fn process(text: &str, config: &TextConfig) -> String {
    let mut output = text.trim().to_owned();
    if config.remove_fillers {
        output = remove_fillers(&output);
    }
    if config.itn {
        output = normalize_times(&output);
    }
    output = collapse_spelled_acronyms(&output);
    if config.fix_term_case {
        output = fix_term_case(&output);
    }
    output = normalize_spacing(&output, config.cjk_spacing);
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

/// 把技术名词改成规范写法：按完整英文词（含被拆开的两词写法，如 mac book）不区分大小写匹配。
pub fn fix_term_case(text: &str) -> String {
    // 纯英文句子里 react、word、go 是普通词，只在中英混输的文本里规范术语。
    if !text.chars().any(is_cjk) {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let is_word = |c: char| c.is_ascii_alphanumeric() || c == '+' || c == '.';
    let mut output = String::with_capacity(text.len());
    let mut i = 0;
    'scan: while i < chars.len() {
        let at_boundary = i == 0 || !is_word(chars[i - 1]);
        if at_boundary && chars[i].is_ascii_alphabetic() {
            // 优先匹配最长的规范写法（「Node.js」先于「Node」，「mac book」→「MacBook」）。
            let mut best: Option<(&str, usize)> = None;
            for term in TERMS {
                let squeezed: Vec<char> = term.chars().filter(|c| *c != ' ').collect();
                let mut j = i;
                let mut k = 0;
                while k < squeezed.len() && j < chars.len() {
                    if chars[j] == ' '
                        && k > 0
                        && j + 1 < chars.len()
                        && chars[j + 1].is_ascii_alphabetic()
                    {
                        // 允许识别结果把一个词拆成两段（mac book、git hub）。
                        j += 1;
                        continue;
                    }
                    if !chars[j].eq_ignore_ascii_case(&squeezed[k]) {
                        break;
                    }
                    j += 1;
                    k += 1;
                }
                let complete = k == squeezed.len();
                let ends_word =
                    j == chars.len() || !(chars[j].is_ascii_alphanumeric() || chars[j] == '+');
                if complete && ends_word && best.is_none_or(|(_, end)| j > end) {
                    best = Some((term, j));
                }
            }
            if let Some((term, end)) = best {
                // 词尾的英文句点属于句子，不属于词（「用 Node.」）。
                let mut end = end;
                if chars[end - 1] == '.' && !term.ends_with('.') {
                    end -= 1;
                }
                output.push_str(&term.replace(' ', if term == "VS Code" { " " } else { "" }));
                i = end;
                continue 'scan;
            }
        }
        output.push(chars[i]);
        i += 1;
    }
    output
}

/// 把逐个字母读出的缩写合起来：「A P I」→「API」，「J S O N」→「JSON」。只处理中英混输文本。
pub fn collapse_spelled_acronyms(text: &str) -> String {
    if !text.chars().any(is_cjk) {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let single_capital = |i: usize| {
        chars[i].is_ascii_uppercase()
            && (i == 0 || !chars[i - 1].is_ascii_alphanumeric())
            && (i + 1 == chars.len() || !chars[i + 1].is_ascii_alphanumeric())
    };
    let mut output = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if single_capital(i) {
            // 收集「X Y Z」：单个大写字母之间只隔一个空格。
            let mut letters = vec![chars[i]];
            let mut j = i;
            while j + 2 < chars.len() && chars[j + 1] == ' ' && single_capital(j + 2) {
                letters.push(chars[j + 2]);
                j += 2;
            }
            if letters.len() >= 2 {
                output.extend(letters);
                i = j + 1;
                continue;
            }
        }
        output.push(chars[i]);
        i += 1;
    }
    output
}

/// 整理空格：中文标点两侧不留空格；中文与英文、数字之间按 `cjk_spacing` 加一个空格或不加。
/// 纯英文文本只合并连续空格。
pub fn normalize_spacing(text: &str, cjk_spacing: bool) -> String {
    let has_cjk = text.chars().any(is_cjk);
    let chars: Vec<char> = text.chars().collect();
    let mut output = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            let mut j = i;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            let previous = output.chars().last();
            let next = chars.get(j).copied();
            let keep = match (previous, next) {
                (None, _) | (_, None) => false,
                (Some(a), Some(b)) if !has_cjk => !(a.is_whitespace() || b.is_whitespace()),
                (Some(a), Some(b)) if is_cjk_mark(a) || is_cjk_mark(b) => false,
                (Some(a), Some(b)) if is_cjk(a) && is_cjk(b) => false,
                (Some(a), Some(b)) if is_cjk(a) || is_cjk(b) => cjk_spacing,
                _ => true,
            };
            if keep {
                output.push(' ');
            }
            i = j;
            continue;
        }
        if cjk_spacing {
            if let Some(previous) = output.chars().last() {
                let mixed = (is_cjk(previous) && c.is_ascii_alphanumeric())
                    || (previous.is_ascii_alphanumeric() && is_cjk(c));
                if mixed {
                    output.push(' ');
                }
            }
        }
        output.push(c);
        i += 1;
    }
    output
}

fn is_cjk_mark(c: char) -> bool {
    "，。！？；：、（）「」『』“”‘’《》【】…—".contains(c)
}

/// 窄范围的数字规整：只在时间、日期语境里把汉字数字写成阿拉伯数字。
/// 「下午三点」→「下午3点」，「三点十五分」→「3点15分」，「十月十四号」→「10月14号」；
/// 「有一点累」「写一个」保持不变。
pub fn normalize_times(text: &str) -> String {
    const NUMERALS: &str = "零一二两三四五六七八九十";
    const TIME_CONTEXT: [&str; 18] = [
        "上午", "下午", "早上", "晚上", "凌晨", "中午", "傍晚", "早晨", "今天", "明天", "后天",
        "昨天", "今晚", "明早", "明晚", "每天", "早", "晚",
    ];
    let chars: Vec<char> = text.chars().collect();
    let is_numeral = |c: char| NUMERALS.contains(c);
    let run_end = |start: usize| {
        let mut end = start;
        while end < chars.len() && is_numeral(chars[end]) {
            end += 1;
        }
        end
    };
    let value =
        |run: &str| -> Option<u32> { crate::edit::chinese_numerals_to_digits(run).parse().ok() };
    let mut output = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if is_numeral(chars[i]) && (i == 0 || !is_numeral(chars[i - 1])) {
            let end = run_end(i);
            let run: String = chars[i..end].iter().collect();
            let before: String = chars[..i].iter().collect();
            let next = chars.get(end).copied();
            let after_next = chars.get(end + 1).copied();
            let hour = next == Some('点')
                && (TIME_CONTEXT.iter().any(|word| before.ends_with(word))
                    || after_next == Some('半')
                    || after_next.is_some_and(is_numeral));
            let month = next == Some('月') && after_next.is_some_and(is_numeral) && {
                let day_end = run_end(end + 1);
                matches!(chars.get(day_end), Some('日' | '号'))
            };
            let minute = next == Some('分') && before.ends_with('点');
            let day = matches!(next, Some('日' | '号')) && before.ends_with('月');
            let limit = if hour {
                24
            } else if month {
                12
            } else {
                59
            };
            if hour || month || minute || day {
                if let Some(number) = value(&run).filter(|number| *number <= limit) {
                    output.push_str(&number.to_string());
                    i = end;
                    continue;
                }
            }
        }
        output.push(chars[i]);
        i += 1;
    }
    output
}

fn is_cjk(c: char) -> bool {
    matches!(c, '\u{4E00}'..='\u{9FFF}' | '\u{3400}'..='\u{4DBF}' | '\u{3040}'..='\u{30FF}' | '\u{AC00}'..='\u{D7AF}')
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
    fn fixes_term_case_only_on_whole_words() {
        assert_eq!(
            fix_term_case("我们用python写一个解析json的脚本"),
            "我们用Python写一个解析JSON的脚本"
        );
        assert_eq!(fix_term_case("记得带上mac book。"), "记得带上MacBook。");
        assert_eq!(fix_term_case("提交到git hub"), "提交到GitHub");
        assert_eq!(
            fix_term_case("在docker里跑node服务"),
            "在Docker里跑Node服务"
        );
        assert_eq!(fix_term_case("用 node.js 写"), "用 Node.js 写");
        assert_eq!(
            fix_term_case("合并到main分支，state 没更新，bug"),
            "合并到main分支，state 没更新，bug"
        );
        assert_eq!(fix_term_case("pythonic 与 gopher"), "pythonic 与 gopher");
        assert_eq!(fix_term_case("我用 rust."), "我用 Rust.");
        assert_eq!(
            fix_term_case("I react to every word you say, let's go."),
            "I react to every word you say, let's go.",
            "纯英文句子不改"
        );
    }

    #[test]
    fn collapses_spelled_acronyms_in_mixed_text() {
        assert_eq!(
            collapse_spelled_acronyms("把这个 A P I 的返回值改成 J S O N 格式"),
            "把这个 API 的返回值改成 JSON 格式"
        );
        assert_eq!(collapse_spelled_acronyms("请把 P R 合并"), "请把 PR 合并");
        assert_eq!(collapse_spelled_acronyms("A 计划"), "A 计划");
        assert_eq!(
            collapse_spelled_acronyms("Plan A I think"),
            "Plan A I think"
        );
    }

    #[test]
    fn spacing_follows_the_cjk_spacing_option() {
        let raw = "我们用 Python 写一个解析 JSON 的脚本， 下午3点发给你。";
        assert_eq!(
            normalize_spacing(raw, false),
            "我们用Python写一个解析JSON的脚本，下午3点发给你。"
        );
        assert_eq!(
            normalize_spacing("我们用Python写", true),
            "我们用 Python 写"
        );
        assert_eq!(
            normalize_spacing("call  me   later", false),
            "call me later"
        );
        assert_eq!(normalize_spacing("用 VS Code 打开", false), "用VS Code打开");
    }

    #[test]
    fn normalizes_only_times_and_dates() {
        assert_eq!(normalize_times("下午三点发给你"), "下午3点发给你");
        assert_eq!(
            normalize_times("开饭时间早上九点至下午五点"),
            "开饭时间早上9点至下午5点"
        );
        assert_eq!(normalize_times("明天上午十点开会"), "明天上午10点开会");
        assert_eq!(normalize_times("三点十五分出发"), "3点15分出发");
        assert_eq!(normalize_times("十二点半见"), "12点半见");
        assert_eq!(normalize_times("十月十四号提交"), "10月14号提交");
        assert_eq!(
            normalize_times("有一点累，写一个脚本，跑一下"),
            "有一点累，写一个脚本，跑一下"
        );
        assert_eq!(normalize_times("三十五点"), "三十五点", "超过 24 不是时刻");
    }

    #[test]
    fn process_turns_raw_zipformer_output_into_doubao_style_text() {
        let config = TextConfig::default();
        assert_eq!(
            process(
                "我们用 Python 写一个解析 JSON 的脚本， 下午三点发给你。",
                &config
            ),
            "我们用Python写一个解析JSON的脚本，下午3点发给你。"
        );
        assert_eq!(
            process(
                "把这个 A P I 的返回值改成 J S O N 格式， 然后提交到 GitHub。",
                &config
            ),
            "把这个API的返回值改成JSON格式，然后提交到GitHub。"
        );
        assert_eq!(
            process("帮我在 docker 里面跑一下这个 node 服务。", &config),
            "帮我在Docker里面跑一下这个Node服务。"
        );
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
