use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub const DEFAULT_ENGINE: &str = "auto";
pub const DEFAULT_LANGUAGE: &str = "zh";
pub const DEFAULT_HOTKEY: &str = "KEY_RIGHTCTRL";
pub const DEFAULT_CLOUD_ENDPOINT: &str = "wss://openspeech.bytedance.com/api/v3/sauc/bigmodel";
/// Official sherpa-onnx SenseVoice-Small int8 package directory.
/// It contains `model.int8.onnx`, `tokens.txt`, and test audio assets.
pub const DEFAULT_OFFLINE_MODEL: &str = "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17";
pub const DEFAULT_LLM_MODEL: &str = "gpt-4o-mini";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Config {
    #[serde(default = "default_engine")]
    pub engine: String,
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default = "default_hotkey")]
    pub hotkey: String,
    #[serde(default)]
    pub cloud: CloudConfig,
    #[serde(default)]
    pub offline: OfflineConfig,
    #[serde(default)]
    pub llm: LlmConfig,
    #[serde(default)]
    pub injection: InjectionConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CloudConfig {
    #[serde(default)]
    pub app_id: String,
    #[serde(default)]
    pub access_token: String,
    #[serde(default = "default_cloud_endpoint")]
    pub endpoint: String,
    #[serde(default)]
    pub resource_id: String,
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_ms: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OfflineConfig {
    #[serde(default = "default_offline_model")]
    pub model: String,
    #[serde(default = "default_model_dir")]
    pub model_dir: String,
    #[serde(default = "default_offline_language")]
    pub language: String,
    #[serde(default = "default_offline_use_itn")]
    pub use_itn: bool,
    #[serde(default = "default_offline_num_threads")]
    pub num_threads: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub api_base: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_llm_model")]
    pub model: String,
    #[serde(default = "default_llm_timeout")]
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InjectionConfig {
    #[serde(default = "default_paste_delay")]
    pub paste_delay_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            engine: default_engine(),
            language: default_language(),
            hotkey: default_hotkey(),
            cloud: CloudConfig::default(),
            offline: OfflineConfig::default(),
            llm: LlmConfig::default(),
            injection: InjectionConfig::default(),
        }
    }
}

impl Default for CloudConfig {
    fn default() -> Self {
        Self {
            app_id: String::new(),
            access_token: String::new(),
            endpoint: default_cloud_endpoint(),
            resource_id: String::new(),
            connect_timeout_ms: default_connect_timeout(),
        }
    }
}

impl Default for OfflineConfig {
    fn default() -> Self {
        Self {
            model: default_offline_model(),
            model_dir: default_model_dir(),
            language: default_offline_language(),
            use_itn: default_offline_use_itn(),
            num_threads: default_offline_num_threads(),
        }
    }
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            api_base: String::new(),
            api_key: String::new(),
            model: default_llm_model(),
            timeout_ms: default_llm_timeout(),
        }
    }
}

impl Default for InjectionConfig {
    fn default() -> Self {
        Self {
            paste_delay_ms: default_paste_delay(),
        }
    }
}

impl Config {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = fs::read_to_string(path)
            .with_context(|| format!("读取配置失败: {}", path.display()))?;
        let config: Self = toml::from_str(&raw).with_context(|| {
            format!(
                "解析配置失败: {}；请检查 TOML 和 engine/language/hotkey 字段",
                path.display()
            )
        })?;
        config
            .validate()
            .with_context(|| format!("配置无效: {}", path.display()))?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if !matches!(self.engine.as_str(), "auto" | "offline" | "cloud") {
            anyhow::bail!("engine 必须是 cloud / offline / auto");
        }
        if !matches!(self.language.as_str(), "zh" | "en" | "yue" | "ja" | "ko") {
            anyhow::bail!("language 必须是 zh / en / yue / ja / ko");
        }
        if self.hotkey.trim().is_empty() {
            anyhow::bail!("hotkey 不能为空");
        }
        if self.offline.language != "auto"
            && !matches!(
                self.offline.language.as_str(),
                "zh" | "en" | "yue" | "ja" | "ko"
            )
        {
            anyhow::bail!("offline.language 必须是 auto / zh / en / yue / ja / ko");
        }
        if self.offline.num_threads < 1 {
            anyhow::bail!("offline.num_threads 必须大于 0");
        }
        Ok(())
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        self.validate().context("拒绝保存无效配置")?;
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("创建配置目录失败: {}", parent.display()))?;
        }
        let serialized = toml::to_string_pretty(self).context("序列化配置失败")?;
        fs::write(path, serialized).with_context(|| format!("写入配置失败: {}", path.display()))?;
        set_private_permissions(path)?;
        Ok(())
    }

    pub fn default_path() -> Option<PathBuf> {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config/yuda/config.toml"))
    }

    pub fn offline_model_paths(&self) -> OfflineModelPaths {
        OfflineModelPaths::from_config(&self.offline)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfflineModelPaths {
    pub directory: PathBuf,
    pub model: PathBuf,
    pub tokens: PathBuf,
}

impl OfflineModelPaths {
    pub fn from_config(config: &OfflineConfig) -> Self {
        let directory = expand_model_dir(&config.model_dir).join(&config.model);
        Self {
            model: directory.join("model.int8.onnx"),
            tokens: directory.join("tokens.txt"),
            directory,
        }
    }

    pub fn is_complete(&self) -> bool {
        self.model.is_file() && self.tokens.is_file()
    }
}

fn expand_model_dir(value: &str) -> PathBuf {
    if value == "~" {
        return std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(value));
    }
    if let Some(rest) = value.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(value)
}

#[cfg(unix)]
fn set_private_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("设置配置权限失败: {}", path.display()))
}

#[cfg(not(unix))]
fn set_private_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

fn default_engine() -> String {
    DEFAULT_ENGINE.to_owned()
}
fn default_language() -> String {
    DEFAULT_LANGUAGE.to_owned()
}
fn default_hotkey() -> String {
    DEFAULT_HOTKEY.to_owned()
}
fn default_cloud_endpoint() -> String {
    DEFAULT_CLOUD_ENDPOINT.to_owned()
}
fn default_offline_model() -> String {
    DEFAULT_OFFLINE_MODEL.to_owned()
}
fn default_model_dir() -> String {
    "~/.local/share/yuda/models".to_owned()
}
fn default_offline_language() -> String {
    "auto".to_owned()
}
fn default_offline_use_itn() -> bool {
    true
}
fn default_offline_num_threads() -> i32 {
    2
}
fn default_llm_model() -> String {
    DEFAULT_LLM_MODEL.to_owned()
}
fn default_connect_timeout() -> u64 {
    2_000
}
fn default_llm_timeout() -> u64 {
    5_000
}
fn default_paste_delay() -> u64 {
    120
}

#[cfg(test)]
mod tests {
    use super::{Config, DEFAULT_CLOUD_ENDPOINT, DEFAULT_OFFLINE_MODEL};
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn offline_paths_point_to_sense_voice_files() {
        let mut config = Config::default();
        config.offline.model_dir = "/tmp/yuda-models".to_owned();
        let paths = config.offline_model_paths();
        assert_eq!(
            paths.directory,
            std::path::PathBuf::from(
                "/tmp/yuda-models/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17"
            )
        );
        assert!(paths.model.ends_with("model.int8.onnx"));
        assert!(paths.tokens.ends_with("tokens.txt"));
        assert_eq!(config.offline.language, "auto");
        assert!(config.offline.use_itn);
        assert_eq!(config.offline.num_threads, 2);
    }

    #[test]
    fn defaults_use_sense_voice_small_int8_package() {
        let config = Config::default();
        assert_eq!(config.offline.model, DEFAULT_OFFLINE_MODEL);
        assert_eq!(
            DEFAULT_OFFLINE_MODEL,
            "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17"
        );
        assert!(config.offline.model.ends_with("-int8-2024-07-17"));
    }

    #[test]
    fn defaults_are_chinese_first_and_round_trip() {
        let config = Config::default();
        assert_eq!(config.language, "zh");
        assert_eq!(config.cloud.endpoint, DEFAULT_CLOUD_ENDPOINT);
        let path = std::env::temp_dir().join(format!(
            "yuda-config-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        config.save(&path).expect("config should save");
        let loaded = Config::load(&path).expect("config should load");
        assert_eq!(loaded, config);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn empty_llm_key_survives_serialization() {
        let mut config = Config::default();
        config.llm.enabled = true;
        config.llm.api_key.clear();
        let raw = toml::to_string(&config).expect("config should serialize");
        assert!(raw.contains("api_key = \"\""));
    }
}
