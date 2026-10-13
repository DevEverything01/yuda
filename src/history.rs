//! 本地识别历史：`~/.local/state/yuda/history.jsonl`，权限 0600，只保留最近 N 条。
//! 用于「重新上屏上一条」与菜单里的历史选择；从不离开本机。

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
};

use crate::config::{set_private_permissions, state_dir};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Unix 时间戳（秒）。
    pub at: i64,
    pub text: String,
    pub engine: String,
    pub duration_ms: u64,
}

impl Entry {
    pub fn now(text: impl Into<String>, engine: impl Into<String>, duration_ms: u64) -> Self {
        Self {
            at: chrono::Utc::now().timestamp(),
            text: text.into(),
            engine: engine.into(),
            duration_ms,
        }
    }

    /// 本地时间，例如 `10-14 09:30`。
    pub fn local_time(&self) -> String {
        use chrono::TimeZone;
        chrono::Local
            .timestamp_opt(self.at, 0)
            .single()
            .map(|time| time.format("%m-%d %H:%M").to_string())
            .unwrap_or_default()
    }
}

pub fn default_path() -> Result<PathBuf> {
    Ok(state_dir()?.join("history.jsonl"))
}

/// 追加一条记录；超过上限的 1.25 倍时压缩回上限，避免每次重写整个文件。
pub fn append(path: &Path, entry: &Entry, max_entries: usize) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("创建历史目录失败: {}", parent.display()))?;
    }
    let mut line = serde_json::to_string(entry).context("序列化历史记录失败")?;
    line.push('\n');
    {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("打开历史文件失败: {}", path.display()))?;
        file.write_all(line.as_bytes())
            .context("写入历史记录失败")?;
    }
    set_private_permissions(path)?;
    let entries = read_all(path)?;
    if entries.len() > max_entries + max_entries / 4 {
        let keep = &entries[entries.len() - max_entries..];
        let mut content = String::new();
        for entry in keep {
            content.push_str(&serde_json::to_string(entry).context("序列化历史记录失败")?);
            content.push('\n');
        }
        let temporary = path.with_extension("jsonl.tmp");
        fs::write(&temporary, content).context("压缩历史文件失败")?;
        set_private_permissions(&temporary)?;
        fs::rename(&temporary, path).context("替换历史文件失败")?;
    }
    Ok(())
}

/// 按时间顺序读取全部记录；损坏的行被跳过。
pub fn read_all(path: &Path) -> Result<Vec<Entry>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file =
        fs::File::open(path).with_context(|| format!("读取历史文件失败: {}", path.display()))?;
    let mut entries = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line.context("读取历史文件失败")?;
        if let Ok(entry) = serde_json::from_str::<Entry>(&line) {
            entries.push(entry);
        }
    }
    Ok(entries)
}

/// 最新的 `limit` 条，最新的在前。
pub fn recent(path: &Path, limit: usize) -> Result<Vec<Entry>> {
    let mut entries = read_all(path)?;
    entries.reverse();
    entries.truncate(limit);
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_reads_newest_first_and_compacts() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("state/history.jsonl");
        for index in 0..30 {
            append(
                &path,
                &Entry::now(format!("第 {index} 条"), "offline", 1_000),
                20,
            )
            .expect("append");
        }
        let all = read_all(&path).expect("read");
        assert!(
            all.len() <= 25,
            "history must be compacted, got {}",
            all.len()
        );
        let newest = recent(&path, 2).expect("recent");
        assert_eq!(newest[0].text, "第 29 条");
        assert_eq!(newest[1].text, "第 28 条");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).expect("metadata").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn skips_corrupt_lines() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("history.jsonl");
        fs::write(&path, "not json\n").expect("write");
        append(&path, &Entry::now("好的", "cloud", 500), 10).expect("append");
        let entries = read_all(&path).expect("read");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].text, "好的");
    }
}
