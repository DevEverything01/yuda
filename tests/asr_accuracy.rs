#![cfg(feature = "daemon")]

use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    fmt::Write as FmtWrite,
    fs,
    path::{Component, Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use chrono::Utc;
use serde::Deserialize;
use yuda::{config::Config, offline_asr::SenseVoiceRecognizer};

const DEFAULT_MAX_CER: f64 = 0.15;
const REQUIRED_SCENARIOS: [&str; 3] = ["coding", "chat", "email"];
const REQUIRED_TAGS: [&str; 6] = [
    "short",
    "long",
    "technical",
    "numbers-punctuation",
    "noise",
    "silence-trim",
];

#[derive(Debug, Deserialize)]
struct Corpus {
    #[serde(default)]
    default_max_cer: Option<f64>,
    #[serde(default)]
    sample: Vec<Sample>,
}

#[derive(Debug, Clone, Deserialize)]
struct Sample {
    file: String,
    scenario: String,
    lang: String,
    transcript: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    max_cer: Option<f64>,
}

#[derive(Debug)]
struct EvalRow {
    sample: Sample,
    actual: String,
    cer: Option<f64>,
    limit: f64,
    error: Option<String>,
}

#[derive(Debug, Default)]
struct ScenarioStats {
    count: usize,
    failures: usize,
    total_cer: f64,
}

#[test]
#[ignore = "requires the SenseVoice model and local real-audio corpus"]
fn asr_accuracy_matches_manifest_thresholds() {
    if let Err(error) = run_accuracy_eval() {
        panic!("ASR 准确性评估失败：{error:#}");
    }
}

fn run_accuracy_eval() -> Result<()> {
    let manifest_path =
        env_path("YUDA_ASR_MANIFEST").unwrap_or_else(|| PathBuf::from("evals/asr/corpus.toml"));
    let raw = fs::read_to_string(&manifest_path)
        .with_context(|| format!("读取 ASR 语料清单失败: {}", manifest_path.display()))?;
    let corpus: Corpus = toml::from_str(&raw)
        .with_context(|| format!("解析 ASR 语料清单失败: {}", manifest_path.display()))?;
    validate_corpus(&corpus)?;

    let mut config = load_config()?;
    let default_limit = corpus.default_max_cer.unwrap_or(DEFAULT_MAX_CER);
    validate_cer_limit(default_limit, "default_max_cer")?;
    let manifest_dir = manifest_path.parent().unwrap_or_else(|| Path::new("."));

    let mut by_language: BTreeMap<String, Vec<Sample>> = BTreeMap::new();
    for sample in corpus.sample {
        by_language
            .entry(sample.lang.clone())
            .or_default()
            .push(sample);
    }

    let mut rows = Vec::new();
    for (language, samples) in by_language {
        config.offline.language = language;
        let recognizer = SenseVoiceRecognizer::from_config(&config)
            .context("按语料语言初始化 SenseVoice 失败")?;
        for sample in samples {
            let path = resolve_sample_path(manifest_dir, &sample.file)?;
            if !path.is_file() {
                bail!("语料音频不存在: {}", path.display());
            }
            let limit = sample.max_cer.unwrap_or(default_limit);
            validate_cer_limit(limit, &format!("{} max_cer", sample.file))?;
            let reference = sample.transcript.trim().to_owned();
            let result = recognizer.transcribe_wav(&path);
            match result {
                Ok(transcript) => {
                    let actual = transcript.text.trim().to_owned();
                    let cer = character_error_rate(&reference, &actual);
                    rows.push(EvalRow {
                        sample,
                        actual,
                        cer: Some(cer),
                        limit,
                        error: None,
                    });
                }
                Err(error) => rows.push(EvalRow {
                    sample,
                    actual: String::new(),
                    cer: None,
                    limit,
                    error: Some(format!("{error:#}")),
                }),
            }
        }
    }

    rows.sort_by(|left, right| {
        left.sample
            .scenario
            .cmp(&right.sample.scenario)
            .then_with(|| left.sample.file.cmp(&right.sample.file))
    });
    let report = render_report(&manifest_path, &rows)?;
    let report_path = report_path(&manifest_path);
    if let Some(parent) = report_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("创建 ASR 报告目录失败: {}", parent.display()))?;
    }
    fs::write(&report_path, report)
        .with_context(|| format!("写入 ASR 报告失败: {}", report_path.display()))?;

    let failures = rows
        .iter()
        .filter(|row| row.error.is_some() || row.cer.is_some_and(|cer| cer > row.limit))
        .count();
    if failures > 0 {
        bail!(
            "ASR 准确性评估有 {failures} 个失败样本；详细报告: {}",
            report_path.display()
        );
    }
    Ok(())
}

fn load_config() -> Result<Config> {
    let mut config = match env_path("YUDA_ASR_CONFIG") {
        Some(path) => Config::load(&path)?,
        None => Config::default(),
    };
    if let Some(model_dir) = env_path("YUDA_ASR_MODEL_DIR") {
        config.offline.model_dir = model_dir.to_string_lossy().into_owned();
    }
    Ok(config)
}

fn validate_corpus(corpus: &Corpus) -> Result<()> {
    if corpus.sample.is_empty() {
        bail!("ASR 语料清单为空；至少需要 coding/chat/email 三类真实音频");
    }
    let mut scenarios = BTreeSet::new();
    let mut tags = BTreeSet::new();
    let mut files = BTreeSet::new();
    for sample in &corpus.sample {
        if sample.file.trim().is_empty() {
            bail!("ASR 样本 file 不能为空");
        }
        resolve_sample_path(Path::new("."), &sample.file)?;
        if sample.scenario.trim().is_empty() {
            bail!("ASR 样本 {} 的 scenario 不能为空", sample.file);
        }
        if sample.lang.trim().is_empty() {
            bail!("ASR 样本 {} 的 lang 不能为空", sample.file);
        }
        if !matches!(
            sample.lang.as_str(),
            "auto" | "zh" | "en" | "yue" | "ja" | "ko"
        ) {
            bail!("ASR 样本 {} 的 lang 不受支持: {}", sample.file, sample.lang);
        }
        if sample.transcript.trim().is_empty() {
            bail!("ASR 样本 {} 的 transcript 不能为空", sample.file);
        }
        if !files.insert(sample.file.as_str()) {
            bail!("ASR 样本 file 重复: {}", sample.file);
        }
        scenarios.insert(sample.scenario.as_str());
        tags.extend(sample.tags.iter().map(String::as_str));
        if let Some(limit) = sample.max_cer {
            validate_cer_limit(limit, &format!("{} max_cer", sample.file))?;
        }
    }
    for scenario in REQUIRED_SCENARIOS {
        if !scenarios.contains(scenario) {
            bail!("ASR 语料缺少必需场景: {scenario}");
        }
    }
    for tag in REQUIRED_TAGS {
        if !tags.contains(tag) {
            bail!("ASR 语料缺少必需边界 tag: {tag}");
        }
    }
    Ok(())
}

fn validate_cer_limit(limit: f64, name: &str) -> Result<()> {
    if !limit.is_finite() || !(0.0..=1.0).contains(&limit) {
        bail!("{name} 必须是 0 到 1 之间的有限数字");
    }
    Ok(())
}

fn resolve_sample_path(manifest_dir: &Path, value: &str) -> Result<PathBuf> {
    let relative = Path::new(value);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        bail!("ASR 样本路径必须是 manifest 下的相对路径且不能包含 ..: {value}");
    }
    Ok(manifest_dir.join(relative))
}

fn character_error_rate(reference: &str, actual: &str) -> f64 {
    let reference: Vec<char> = reference.chars().collect();
    let actual: Vec<char> = actual.chars().collect();
    if reference.is_empty() {
        return if actual.is_empty() { 0.0 } else { 1.0 };
    }
    edit_distance(&reference, &actual) as f64 / reference.len() as f64
}

fn edit_distance(reference: &[char], actual: &[char]) -> usize {
    let mut previous: Vec<usize> = (0..=actual.len()).collect();
    for (row, expected) in reference.iter().enumerate() {
        let mut current = vec![row + 1; actual.len() + 1];
        for (column, observed) in actual.iter().enumerate() {
            let substitution = previous[column] + usize::from(expected != observed);
            current[column + 1] = (substitution)
                .min(previous[column + 1] + 1)
                .min(current[column] + 1);
        }
        previous = current;
    }
    previous[actual.len()]
}

fn render_report(manifest_path: &Path, rows: &[EvalRow]) -> Result<String> {
    let mut stats: BTreeMap<&str, ScenarioStats> = BTreeMap::new();
    for row in rows {
        let entry = stats.entry(row.sample.scenario.as_str()).or_default();
        entry.count += 1;
        match (row.cer, row.error.is_some()) {
            (Some(cer), false) => {
                entry.total_cer += cer;
                if cer > row.limit {
                    entry.failures += 1;
                }
            }
            _ => entry.failures += 1,
        }
    }

    let mut report = String::new();
    writeln!(report, "# ASR 准确性评估").context("渲染 ASR 报告标题失败")?;
    writeln!(report, "\n- 生成时间：{}", Utc::now().to_rfc3339())?;
    writeln!(report, "- 清单：{}", manifest_path.display())?;
    writeln!(report, "- 指标：CER（字符级编辑距离 / 参考文本字符数）")?;
    writeln!(report, "\n## 场景汇总\n")?;
    writeln!(report, "| 场景 | 样本 | 平均 CER | 失败 |")?;
    writeln!(report, "|---|---:|---:|---:|")?;
    for (scenario, stat) in stats {
        let average = if stat.count == 0 {
            0.0
        } else {
            stat.total_cer / stat.count as f64
        };
        writeln!(
            report,
            "| {scenario} | {} | {average:.4} | {} |",
            stat.count, stat.failures
        )?;
    }

    writeln!(report, "\n## 样本明细\n")?;
    writeln!(report, "| 场景 | 文件 | CER | 阈值 | 状态 |")?;
    writeln!(report, "|---|---|---:|---:|---|")?;
    for row in rows {
        let status = match (row.cer, row.error.is_some()) {
            (_, true) => "ERROR".to_owned(),
            (Some(cer), false) if cer <= row.limit => "OK".to_owned(),
            (Some(_), false) => "FAIL".to_owned(),
            (None, false) => "ERROR".to_owned(),
        };
        let cer = row
            .cer
            .map_or_else(|| "-".to_owned(), |value| format!("{value:.4}"));
        writeln!(
            report,
            "| {} | `{}` | {cer} | {:.4} | {status} |",
            row.sample.scenario, row.sample.file, row.limit
        )?;
        if let Some(error) = &row.error {
            writeln!(report, "\n错误：`{error}`")?;
        }
        if row.cer.is_some_and(|cer| cer > row.limit) {
            writeln!(
                report,
                "\n参考：`{}`\n识别：`{}`",
                row.sample.transcript, row.actual
            )?;
        }
    }
    Ok(report)
}

fn report_path(manifest_path: &Path) -> PathBuf {
    env_path("YUDA_ASR_REPORT").unwrap_or_else(|| {
        manifest_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("reports")
            .join(format!("asr-{}.md", Utc::now().format("%Y%m%dT%H%M%SZ")))
    })
}

fn env_path(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[test]
fn checked_in_manifest_covers_required_scenarios_and_boundaries() {
    let raw = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/evals/asr/corpus.toml"
    ));
    let corpus: Corpus = toml::from_str(raw).expect("checked-in ASR manifest should parse");
    validate_corpus(&corpus).expect("checked-in ASR manifest should satisfy coverage contract");
}

#[cfg(test)]
mod tests {
    use super::{character_error_rate, edit_distance};

    #[test]
    fn cer_is_unicode_safe_and_handles_empty_edges() {
        assert_eq!(character_error_rate("", ""), 0.0);
        assert_eq!(character_error_rate("", "字"), 1.0);
        assert_eq!(character_error_rate("你 好", "你 好"), 0.0);
        let cer = character_error_rate("Python JSON", "配森 杰森");
        assert!((0.8..1.0).contains(&cer), "unexpected Unicode CER: {cer}");
    }

    #[test]
    fn edit_distance_covers_insert_delete_and_substitute() {
        let reference: Vec<char> = "abc".chars().collect();
        let actual: Vec<char> = "axcde".chars().collect();
        assert_eq!(edit_distance(&reference, &actual), 3);
    }
}
