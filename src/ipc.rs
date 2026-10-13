//! daemon 与客户端（CLI、Omarchy 适配器、shell 插件）之间的 Unix socket 协议。
//! 契约见 `docs/ipc.md`。

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    str::FromStr,
    time::Duration,
};

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Start,
    Stop,
    Toggle,
    Cancel,
    Status,
    Subscribe,
    /// 重新读取配置文件（菜单切换引擎/语言后调用）。
    Reload,
}

impl Command {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Toggle => "toggle",
            Self::Cancel => "cancel",
            Self::Status => "status",
            Self::Subscribe => "subscribe",
            Self::Reload => "reload",
        }
    }
}

impl fmt::Display for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Command {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        Ok(match value.trim() {
            "start" => Self::Start,
            "stop" => Self::Stop,
            "toggle" => Self::Toggle,
            "cancel" => Self::Cancel,
            "status" => Self::Status,
            "subscribe" => Self::Subscribe,
            "reload" => Self::Reload,
            other => bail!("未知命令: {other}"),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Reply {
    pub fn ok() -> Self {
        Self {
            ok: true,
            error: None,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(message.into()),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    #[default]
    Idle,
    Recording,
    Transcribing,
    Refining,
    Injecting,
    Error,
}

impl State {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Idle => "空闲",
            Self::Recording => "录音中",
            Self::Transcribing => "识别中",
            Self::Refining => "优化中",
            Self::Injecting => "上屏中",
            Self::Error => "出错",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    Cloud,
    #[default]
    Offline,
}

impl Engine {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Cloud => "云端",
            Self::Offline => "离线",
        }
    }
}

/// 一行完整状态；UI 用最新一行替换旧状态。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub v: u32,
    pub state: State,
    pub level: f32,
    pub text: String,
    pub engine: Engine,
    pub fallback: bool,
    pub language: String,
    pub message: String,
    pub elapsed_ms: u64,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            v: PROTOCOL_VERSION,
            state: State::Idle,
            level: 0.0,
            text: String::new(),
            engine: Engine::Offline,
            fallback: false,
            language: "zh".to_owned(),
            message: String::new(),
            elapsed_ms: 0,
        }
    }
}

impl Snapshot {
    pub fn to_line(&self) -> String {
        // Snapshot 只有基础类型字段，序列化不会失败；保底返回最小合法 JSON。
        let mut line = serde_json::to_string(self).unwrap_or_else(|_| "{}".to_owned());
        line.push('\n');
        line
    }
}

/// `$XDG_RUNTIME_DIR/yuda/yuda.sock`。
pub fn socket_path() -> Result<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|dir| !dir.is_empty())
        .context("XDG_RUNTIME_DIR 未设置；语打需要在登录会话中运行")?;
    Ok(PathBuf::from(runtime).join("yuda/yuda.sock"))
}

fn connect(timeout: Duration) -> Result<UnixStream> {
    let path = socket_path()?;
    let stream = UnixStream::connect(&path)
        .with_context(|| format!("无法连接语打 daemon（{}）", path.display()))?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    Ok(stream)
}

fn exchange(command: Command, timeout: Duration) -> Result<String> {
    let mut stream = connect(timeout)?;
    stream.write_all(format!("{command}\n").as_bytes())?;
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .context("等待 daemon 回应超时")?;
    if line.trim().is_empty() {
        bail!("daemon 关闭了连接但没有回应");
    }
    Ok(line)
}

/// 发送控制命令（start/stop/toggle/cancel/reload）。
pub fn call(command: Command, timeout: Duration) -> Result<Reply> {
    let line = exchange(command, timeout)?;
    serde_json::from_str(&line).with_context(|| format!("无法解析 daemon 回应: {}", line.trim()))
}

pub fn status(timeout: Duration) -> Result<Snapshot> {
    let line = exchange(Command::Status, timeout)?;
    serde_json::from_str(&line).with_context(|| format!("无法解析状态: {}", line.trim()))
}

/// 订阅状态流；迭代器在 daemon 断开时结束。
pub fn subscribe() -> Result<impl Iterator<Item = Result<Snapshot>>> {
    let mut stream = connect(Duration::from_secs(2))?;
    stream.write_all(b"subscribe\n")?;
    stream.set_read_timeout(None)?;
    Ok(BufReader::new(stream).lines().map(|line| {
        let line = line.context("读取状态流失败")?;
        serde_json::from_str(&line).with_context(|| format!("无法解析状态: {line}"))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_round_trip_through_text() {
        for command in [
            Command::Start,
            Command::Stop,
            Command::Toggle,
            Command::Cancel,
            Command::Status,
            Command::Subscribe,
            Command::Reload,
        ] {
            assert_eq!(command.as_str().parse::<Command>().expect("parse"), command);
        }
        assert_eq!(" stop\n".parse::<Command>().expect("trim"), Command::Stop);
        assert!("explode".parse::<Command>().is_err());
    }

    #[test]
    fn snapshot_line_matches_documented_shape() {
        let snapshot = Snapshot {
            state: State::Recording,
            level: 0.5,
            text: "今天下午三点".to_owned(),
            elapsed_ms: 1840,
            ..Snapshot::default()
        };
        let line = snapshot.to_line();
        assert!(line.ends_with('\n'));
        let value: serde_json::Value = serde_json::from_str(&line).expect("json");
        assert_eq!(value["v"], 1);
        assert_eq!(value["state"], "recording");
        assert_eq!(value["engine"], "offline");
        assert_eq!(value["fallback"], false);
        assert_eq!(value["text"], "今天下午三点");
        assert_eq!(value["elapsed_ms"], 1840);
        for key in ["level", "language", "message"] {
            assert!(value.get(key).is_some(), "missing {key}");
        }
    }

    #[test]
    fn replies_serialize_without_null_error() {
        assert_eq!(
            serde_json::to_string(&Reply::ok()).expect("json"),
            r#"{"ok":true}"#
        );
        assert_eq!(
            serde_json::to_string(&Reply::error("忙")).expect("json"),
            r#"{"ok":false,"error":"忙"}"#
        );
    }
}
