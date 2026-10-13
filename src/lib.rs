//! 语打 Yuda：Omarchy 上的中文语音输入 daemon。
//!
//! 默认特性只包含纯逻辑（配置、IPC、会话状态机、文本后处理、上屏命令序列），
//! 任意平台可测；音频、识别引擎与热键由 feature 打开。

pub mod config;
pub mod edit;
pub mod history;
pub mod injection;
pub mod ipc;
pub mod omarchy;
pub mod session;
pub mod text;

#[cfg(feature = "audio")]
pub mod audio;
#[cfg(feature = "cloud")]
pub mod cloud_asr;
#[cfg(feature = "evdev")]
pub mod hotkey;
#[cfg(feature = "sensevoice")]
pub mod offline;
#[cfg(feature = "llm")]
pub mod refine;

#[cfg(all(feature = "audio", feature = "sensevoice"))]
pub mod asr;
#[cfg(all(feature = "audio", feature = "sensevoice"))]
pub mod daemon;
