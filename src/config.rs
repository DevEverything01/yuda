//! 用户配置：`~/.config/yuda/config.toml`（权限 0600）。
//!
//! 模型名、endpoint 等默认值只允许出现在本文件的常量里（AGENTS.md 纪律）。

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fmt, fs,
    path::{Path, PathBuf},
    str::FromStr,
};

pub const DEFAULT_CLOUD_ENDPOINT: &str =
    "wss://openspeech.bytedance.com/api/v3/sauc/bigmodel_async";
pub const DEFAULT_CLOUD_RESOURCE_ID: &str = "volc.bigasr.sauc.duration";
/// 中文 / 英文默认离线模型：官方 sherpa-onnx 的 zipformer transducer 中英模型（带标点，int8）。
/// 实测技术名词（Python、JSON、GitHub、MacBook）远好于 SenseVoice-Small，速度也更快。
pub const DEFAULT_OFFLINE_MODEL: &str =
    "sherpa-onnx-x-asr-zipformer-transducer-zh-en-punct-int8-2026-06-03";
/// 粤语 / 日语 / 韩语使用的多语种离线模型：SenseVoice-Small int8。
pub const DEFAULT_MULTILINGUAL_MODEL: &str =
    "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17";
/// 官方 sherpa-onnx 发布的 Silero VAD 模型文件。
pub const DEFAULT_VAD_MODEL: &str = "silero_vad.onnx";
pub const DEFAULT_MODEL_DIR: &str = "~/.local/share/yuda/models";
pub const DEFAULT_LLM_MODEL: &str = "gpt-4o-mini";
pub const DEFAULT_EVDEV_KEY: &str = "KEY_RIGHTCTRL";

/// 识别引擎模式。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EngineMode {
    /// 只用火山引擎豆包大模型。
    Cloud,
    /// 只用本地 SenseVoice。
    Offline,
    /// 云端优先，2 秒连不上自动切换离线。
    #[default]
    Auto,
}

impl EngineMode {
    pub const ALL: [Self; 3] = [Self::Auto, Self::Cloud, Self::Offline];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cloud => "cloud",
            Self::Offline => "offline",
            Self::Auto => "auto",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Cloud => "云端（豆包大模型）",
            Self::Offline => "离线（SenseVoice）",
            Self::Auto => "自动（云端优先，断网离线）",
        }
    }
}

impl fmt::Display for EngineMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for EngineMode {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "cloud" => Ok(Self::Cloud),
            "offline" => Ok(Self::Offline),
            "auto" => Ok(Self::Auto),
            _ => bail!("engine 必须是 cloud / offline / auto，收到 {value}"),
        }
    }
}

/// 识别语言：SenseVoice 支持的集合。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    /// 中文（中英混输）。
    #[default]
    Zh,
    En,
    Yue,
    Ja,
    Ko,
}

impl Language {
    pub const ALL: [Self; 5] = [Self::Zh, Self::En, Self::Yue, Self::Ja, Self::Ko];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Zh => "zh",
            Self::En => "en",
            Self::Yue => "yue",
            Self::Ja => "ja",
            Self::Ko => "ko",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Zh => "中文（中英混输）",
            Self::En => "English",
            Self::Yue => "粤语",
            Self::Ja => "日本語",
            Self::Ko => "한국어",
        }
    }
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Language {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|language| language.as_str() == value)
            .with_context(|| format!("language 必须是 zh / en / yue / ja / ko，收到 {value}"))
    }
}

/// 谁来触发录音。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TriggerSource {
    /// Omarchy 的听写快捷键（Right Alt / F9 按住，Super+Ctrl+X 切换）通过
    /// `omarchy-dictation-yuda` 驱动 daemon；不需要 input 组。
    #[default]
    Omarchy,
    /// 语打自己读 evdev 监听按键（非 Omarchy 的 Hyprland；需要 input 组）。
    Evdev,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub engine: EngineMode,
    #[serde(default)]
    pub language: Language,
    #[serde(default)]
    pub trigger: TriggerConfig,
    #[serde(default)]
    pub cloud: CloudConfig,
    #[serde(default)]
    pub offline: OfflineConfig,
    #[serde(default)]
    pub text: TextConfig,
    #[serde(default)]
    pub llm: LlmConfig,
    #[serde(default)]
    pub injection: InjectionConfig,
    #[serde(default)]
    pub history: HistoryConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerConfig {
    #[serde(default)]
    pub source: TriggerSource,
    #[serde(default = "default_evdev_key")]
    pub evdev_key: String,
    /// 按住短于该时长视为误触，不识别。
    #[serde(default = "default_min_hold")]
    pub min_hold_ms: u64,
    /// 单次录音上限，防止忘记松手。
    #[serde(default = "default_max_recording")]
    pub max_recording_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CloudConfig {
    /// 火山引擎控制台的 APP ID（请求头 X-Api-App-Key）；新版控制台的单一 API Key 用户留空。
    #[serde(default)]
    pub app_key: String,
    /// 火山引擎控制台的 Access Token（请求头 X-Api-Access-Key）；或新版控制台的 API Key（X-Api-Key）。
    #[serde(default)]
    pub access_key: String,
    #[serde(default = "default_cloud_resource_id")]
    pub resource_id: String,
    #[serde(default = "default_cloud_endpoint")]
    pub endpoint: String,
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OfflineConfig {
    #[serde(default = "default_model_dir")]
    pub model_dir: String,
    /// 中文（中英混输）与英文使用的模型目录名。
    #[serde(default = "default_offline_model")]
    pub model: String,
    /// 粤语、日语、韩语使用的模型目录名。
    #[serde(default = "default_multilingual_model")]
    pub multilingual_model: String,
    #[serde(default = "default_vad_model")]
    pub vad_model: String,
    #[serde(default = "default_offline_num_threads")]
    pub num_threads: i32,
    #[serde(default = "default_true")]
    pub use_itn: bool,
}

/// 识别后的本地文本处理（不依赖 LLM）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextConfig {
    /// 去掉句首/逗号间独立出现的「嗯、呃、额」等语气词。
    #[serde(default = "default_true")]
    pub remove_fillers: bool,
    /// 技术名词规范写法：python → Python、json → JSON、mac book → MacBook。
    #[serde(default = "default_true")]
    pub fix_term_case: bool,
    /// 时间与日期写成阿拉伯数字：「下午三点」→「下午3点」，「十月十四号」→「10月14号」。
    #[serde(default = "default_true")]
    pub itn: bool,
    /// 中文与英文、数字之间加空格（「用 Python 写」）；默认不加（「用Python写」）。
    #[serde(default)]
    pub cjk_spacing: bool,
    /// 用户词库：识别结果中的左侧文本替换为右侧（例如 "配森" = "Python"）。
    #[serde(default)]
    pub replacements: BTreeMap<String, String>,
    /// 热词：提交给云端引擎提升专有名词识别率；离线引擎忽略。
    #[serde(default)]
    pub hotwords: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LlmConfig {
    #[serde(default)]
    pub enabled: bool,
    /// OpenAI 兼容端点，例如 https://api.openai.com/v1
    #[serde(default)]
    pub api_base: String,
    /// 可以清空；为空时不发送 Authorization 头。
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_llm_model")]
    pub model: String,
    #[serde(default = "default_llm_timeout")]
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InjectionConfig {
    /// 发送粘贴快捷键后，等待目标应用读取剪贴板的时间，之后恢复原剪贴板。
    #[serde(default = "default_paste_delay")]
    pub paste_delay_ms: u64,
    /// 为 false 时只把结果放进剪贴板，不模拟粘贴。
    #[serde(default = "default_true")]
    pub auto_paste: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryConfig {
    /// 在本机 `~/.local/state/yuda/history.jsonl`（0600）保留最近的识别结果。
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_history_entries")]
    pub max_entries: usize,
}

impl Default for TriggerConfig {
    fn default() -> Self {
        Self {
            source: TriggerSource::default(),
            evdev_key: default_evdev_key(),
            min_hold_ms: default_min_hold(),
            max_recording_secs: default_max_recording(),
        }
    }
}

impl Default for CloudConfig {
    fn default() -> Self {
        Self {
            app_key: String::new(),
            access_key: String::new(),
            resource_id: default_cloud_resource_id(),
            endpoint: default_cloud_endpoint(),
            connect_timeout_ms: default_connect_timeout(),
        }
    }
}

impl Default for OfflineConfig {
    fn default() -> Self {
        Self {
            model_dir: default_model_dir(),
            model: default_offline_model(),
            multilingual_model: default_multilingual_model(),
            vad_model: default_vad_model(),
            num_threads: default_offline_num_threads(),
            use_itn: true,
        }
    }
}

impl Default for TextConfig {
    fn default() -> Self {
        Self {
            remove_fillers: true,
            fix_term_case: true,
            itn: true,
            cjk_spacing: false,
            replacements: BTreeMap::new(),
            hotwords: Vec::new(),
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
            auto_paste: true,
        }
    }
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_entries: default_history_entries(),
        }
    }
}

impl CloudConfig {
    /// 云端引擎是否具备最基本的凭据。新版控制台只发一个 API Key：
    /// 此时 app_key 留空，access_key 填 API Key。
    pub fn is_configured(&self) -> bool {
        !self.access_key.trim().is_empty()
    }
}

impl LlmConfig {
    pub fn is_usable(&self) -> bool {
        self.enabled && !self.api_base.trim().is_empty() && !self.model.trim().is_empty()
    }
}

impl Config {
    /// 读取配置；文件不存在时返回默认值。
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = fs::read_to_string(path)
            .with_context(|| format!("读取配置失败: {}", path.display()))?;
        let config: Self = toml::from_str(&raw).with_context(|| {
            format!("解析配置失败: {}；请检查 TOML 语法与字段名", path.display())
        })?;
        config
            .validate()
            .with_context(|| format!("配置无效: {}", path.display()))?;
        Ok(config)
    }

    /// 读取默认路径的配置。
    pub fn load_default() -> Result<Self> {
        Self::load(Self::default_path()?)
    }

    pub fn validate(&self) -> Result<()> {
        if self.trigger.evdev_key.trim().is_empty() {
            bail!("trigger.evdev_key 不能为空");
        }
        if self.trigger.max_recording_secs == 0 {
            bail!("trigger.max_recording_secs 必须大于 0");
        }
        if self.offline.num_threads < 1 {
            bail!("offline.num_threads 必须大于 0");
        }
        if self.history.enabled && self.history.max_entries == 0 {
            bail!("history.max_entries 必须大于 0");
        }
        if self.text.replacements.keys().any(|key| key.is_empty()) {
            bail!("text.replacements 的键不能为空字符串");
        }
        Ok(())
    }

    /// 写入完整配置（会丢失注释；用户配置的局部修改请用 [`set_value`]）。
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        self.validate().context("拒绝保存无效配置")?;
        let serialized = toml::to_string_pretty(self).context("序列化配置失败")?;
        write_private(path.as_ref(), &serialized)
    }

    pub fn default_path() -> Result<PathBuf> {
        Ok(config_home()?.join("yuda/config.toml"))
    }

    /// 当前语言使用的离线模型。
    pub fn offline_model_paths(&self) -> OfflineModelPaths {
        OfflineModelPaths::for_language(&self.offline, self.language)
    }
}

/// 离线模型目录与其中的模型文件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfflineModelPaths {
    pub directory: PathBuf,
    pub vad: PathBuf,
}

/// 按目录里的文件识别出的模型族。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelFiles {
    /// `model.int8.onnx` + `tokens.txt`
    SenseVoice { model: PathBuf, tokens: PathBuf },
    /// `encoder*.onnx` + `decoder*.onnx` + `joiner*.onnx` + `tokens.txt`
    Transducer {
        encoder: PathBuf,
        decoder: PathBuf,
        joiner: PathBuf,
        tokens: PathBuf,
    },
}

impl Language {
    /// 粤语、日语、韩语需要多语种模型。
    pub const fn needs_multilingual_model(self) -> bool {
        matches!(self, Self::Yue | Self::Ja | Self::Ko)
    }
}

impl OfflineModelPaths {
    pub fn for_language(config: &OfflineConfig, language: Language) -> Self {
        let root = expand_home(&config.model_dir);
        let name = if language.needs_multilingual_model() {
            &config.multilingual_model
        } else {
            &config.model
        };
        Self {
            directory: root.join(name),
            vad: root.join(&config.vad_model),
        }
    }

    /// 识别目录里的模型文件；int8 版本优先。
    pub fn files(&self) -> Result<ModelFiles> {
        if !self.directory.is_dir() {
            bail!(
                "离线模型目录不存在: {}；运行 omarchy-install-dictation-yuda 下载模型",
                self.directory.display()
            );
        }
        let tokens = self.directory.join("tokens.txt");
        if !tokens.is_file() {
            bail!("离线模型缺少 tokens.txt: {}", self.directory.display());
        }
        let sense_voice = self.directory.join("model.int8.onnx");
        if sense_voice.is_file() {
            return Ok(ModelFiles::SenseVoice {
                model: sense_voice,
                tokens,
            });
        }
        let find = |prefix: &str| -> Result<PathBuf> {
            let mut candidates: Vec<PathBuf> = fs::read_dir(&self.directory)?
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .filter(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with(prefix) && name.ends_with(".onnx"))
                })
                .collect();
            candidates.sort_by_key(|path| !path.to_string_lossy().contains("int8"));
            candidates.into_iter().next().with_context(|| {
                format!("离线模型缺少 {prefix}*.onnx: {}", self.directory.display())
            })
        };
        Ok(ModelFiles::Transducer {
            encoder: find("encoder")?,
            decoder: find("decoder")?,
            joiner: find("joiner")?,
            tokens,
        })
    }

    pub fn is_complete(&self) -> bool {
        self.files().is_ok()
    }
}

/// 就地修改配置文件中的一个键，保留用户的注释与格式；修改后的整体配置必须仍然有效。
///
/// `key` 形如 `engine`、`llm.enabled`；`value` 按 TOML 字面量解析，失败时视为字符串。
pub fn set_value(path: impl AsRef<Path>, key: &str, value: &str) -> Result<()> {
    let path = path.as_ref();
    let raw = if path.exists() {
        fs::read_to_string(path).with_context(|| format!("读取配置失败: {}", path.display()))?
    } else {
        String::new()
    };
    let updated = set_value_in(&raw, key, value)?;
    let config: Config = toml::from_str(&updated).context("修改后的配置无法解析")?;
    config.validate().context("修改后的配置无效")?;
    write_private(path, &updated)
}

fn set_value_in(raw: &str, key: &str, value: &str) -> Result<String> {
    let mut document: toml_edit::DocumentMut = raw.parse().context("配置不是有效的 TOML")?;
    let parts: Vec<&str> = key.split('.').collect();
    let Some((last, tables)) = parts.split_last() else {
        bail!("配置键不能为空");
    };
    if parts.iter().any(|part| part.is_empty()) {
        bail!("无效的配置键: {key}");
    }
    let parsed = parse_toml_value(value);
    let mut table = document.as_table_mut();
    for name in tables {
        let entry = table
            .entry(name)
            .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()));
        table = entry
            .as_table_mut()
            .with_context(|| format!("配置项 {name} 不是表"))?;
    }
    match table.get_mut(last).and_then(toml_edit::Item::as_value_mut) {
        // 只替换值本身，保留键前注释与行尾注释。
        Some(existing) => {
            let decor = existing.decor().clone();
            *existing = parsed;
            *existing.decor_mut() = decor;
        }
        None => {
            table.insert(last, toml_edit::Item::Value(parsed));
        }
    }
    Ok(document.to_string())
}

/// 读取配置中的一个键，返回 TOML 字面量（字符串不带引号）。
pub fn get_value(config: &Config, key: &str) -> Result<String> {
    let value = toml::Value::try_from(config).context("序列化配置失败")?;
    let mut current = &value;
    for part in key.split('.') {
        current = current
            .get(part)
            .with_context(|| format!("未知配置键: {key}"))?;
    }
    Ok(match current {
        toml::Value::String(text) => text.clone(),
        other => other.to_string(),
    })
}

fn parse_toml_value(value: &str) -> toml_edit::Value {
    let probe = format!("v = {value}");
    probe
        .parse::<toml_edit::DocumentMut>()
        .ok()
        .and_then(|document| document.get("v").and_then(|item| item.as_value().cloned()))
        .unwrap_or_else(|| toml_edit::Value::from(value))
}

fn write_private(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("创建配置目录失败: {}", parent.display()))?;
    }
    // 先写同目录临时文件再改名，避免中途失败留下半截配置。
    let temporary = path.with_extension("toml.tmp");
    fs::write(&temporary, content)
        .with_context(|| format!("写入配置失败: {}", temporary.display()))?;
    set_private_permissions(&temporary)?;
    fs::rename(&temporary, path).with_context(|| format!("替换配置失败: {}", path.display()))?;
    Ok(())
}

fn config_home() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME").filter(|dir| !dir.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(".config"))
        .context("无法确定 HOME，配置路径不可用")
}

/// 状态目录：`$XDG_STATE_HOME/yuda` 或 `~/.local/state/yuda`。
pub fn state_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_STATE_HOME").filter(|dir| !dir.is_empty()) {
        return Ok(PathBuf::from(dir).join("yuda"));
    }
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(".local/state/yuda"))
        .context("无法确定 HOME，状态目录不可用")
}

pub fn expand_home(value: &str) -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    match (value, home) {
        ("~", Some(home)) => home,
        (path, Some(home)) if path.starts_with("~/") => home.join(&path[2..]),
        (path, _) => PathBuf::from(path),
    }
}

#[cfg(unix)]
pub fn set_private_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("设置文件权限失败: {}", path.display()))
}

#[cfg(not(unix))]
pub fn set_private_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

fn default_true() -> bool {
    true
}
fn default_evdev_key() -> String {
    DEFAULT_EVDEV_KEY.to_owned()
}
fn default_min_hold() -> u64 {
    300
}
fn default_max_recording() -> u64 {
    300
}
fn default_cloud_endpoint() -> String {
    DEFAULT_CLOUD_ENDPOINT.to_owned()
}
fn default_cloud_resource_id() -> String {
    DEFAULT_CLOUD_RESOURCE_ID.to_owned()
}
fn default_connect_timeout() -> u64 {
    2_000
}
fn default_model_dir() -> String {
    DEFAULT_MODEL_DIR.to_owned()
}
fn default_offline_model() -> String {
    DEFAULT_OFFLINE_MODEL.to_owned()
}
fn default_multilingual_model() -> String {
    DEFAULT_MULTILINGUAL_MODEL.to_owned()
}
fn default_vad_model() -> String {
    DEFAULT_VAD_MODEL.to_owned()
}
fn default_offline_num_threads() -> i32 {
    2
}
fn default_llm_model() -> String {
    DEFAULT_LLM_MODEL.to_owned()
}
fn default_llm_timeout() -> u64 {
    5_000
}
fn default_paste_delay() -> u64 {
    200
}
fn default_history_entries() -> usize {
    200
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_chinese_first_and_omarchy_driven() {
        let config = Config::default();
        assert_eq!(config.language, Language::Zh);
        assert_eq!(config.engine, EngineMode::Auto);
        assert_eq!(config.trigger.source, TriggerSource::Omarchy);
        assert_eq!(config.trigger.min_hold_ms, 300);
        assert_eq!(config.cloud.endpoint, DEFAULT_CLOUD_ENDPOINT);
        assert!(config.text.remove_fillers);
        assert!(!config.cloud.is_configured());
        assert!(!config.llm.is_usable());
    }

    #[test]
    fn chinese_and_english_use_the_zipformer_others_use_sense_voice() {
        let mut config = Config::default();
        config.offline.model_dir = "/tmp/yuda-models".to_owned();
        let paths = config.offline_model_paths();
        assert_eq!(
            paths.directory,
            PathBuf::from("/tmp/yuda-models").join(DEFAULT_OFFLINE_MODEL)
        );
        assert_eq!(paths.vad, PathBuf::from("/tmp/yuda-models/silero_vad.onnx"));
        config.language = Language::Yue;
        assert_eq!(
            config.offline_model_paths().directory,
            PathBuf::from("/tmp/yuda-models").join(DEFAULT_MULTILINGUAL_MODEL)
        );
        config.language = Language::En;
        assert_eq!(config.offline_model_paths().directory, paths.directory);
    }

    #[test]
    fn detects_model_family_from_files() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let root = directory.path();
        let paths = OfflineModelPaths {
            directory: root.join("m"),
            vad: root.join("vad.onnx"),
        };
        assert!(paths.files().is_err(), "missing directory");
        fs::create_dir_all(root.join("m")).expect("mkdir");
        fs::write(root.join("m/tokens.txt"), "").expect("tokens");
        for name in [
            "encoder-epoch-99-avg-1.onnx",
            "encoder-epoch-99-avg-1.int8.onnx",
            "decoder-epoch-99-avg-1.onnx",
            "joiner-epoch-99-avg-1.int8.onnx",
        ] {
            fs::write(root.join("m").join(name), "").expect("model file");
        }
        match paths.files().expect("transducer") {
            ModelFiles::Transducer { encoder, .. } => {
                assert!(
                    encoder.ends_with("encoder-epoch-99-avg-1.int8.onnx"),
                    "int8 preferred"
                )
            }
            other => panic!("unexpected {other:?}"),
        }
        fs::write(root.join("m/model.int8.onnx"), "").expect("sense voice");
        assert!(matches!(
            paths.files().expect("sense voice"),
            ModelFiles::SenseVoice { .. }
        ));
    }

    #[test]
    fn round_trips_through_disk_with_private_permissions() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("yuda/config.toml");
        let mut config = Config::default();
        config
            .text
            .replacements
            .insert("配森".to_owned(), "Python".to_owned());
        config.save(&path).expect("config should save");
        assert_eq!(Config::load(&path).expect("config should load"), config);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).expect("metadata").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "config must not be readable by other users");
        }
    }

    #[test]
    fn empty_llm_key_survives_serialization() {
        let mut config = Config::default();
        config.llm.enabled = true;
        config.llm.api_key.clear();
        let raw = toml::to_string(&config).expect("config should serialize");
        assert!(raw.contains("api_key = \"\""));
    }

    #[test]
    fn rejects_unknown_engine_and_fields_with_path_in_message() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("broken.toml");
        fs::write(&path, "engine = \"telepathy\"\n").expect("write");
        let message = format!("{:#}", Config::load(&path).expect_err("must fail"));
        assert!(message.contains("解析配置失败"));
        assert!(message.contains(path.to_string_lossy().as_ref()));

        fs::write(&path, "[llm]\nenabeld = true\n").expect("write");
        assert!(Config::load(&path).is_err(), "typos must not be ignored");
    }

    #[test]
    fn missing_file_yields_defaults() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let config = Config::load(directory.path().join("absent.toml")).expect("defaults");
        assert_eq!(config, Config::default());
    }

    #[test]
    fn set_value_preserves_comments_and_validates() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("config.toml");
        fs::write(
            &path,
            "# 我的配置\nengine = \"auto\" # 引擎\n\n[llm]\n# 纠错\nenabled = false\n",
        )
        .expect("write");
        set_value(&path, "llm.enabled", "true").expect("set bool");
        set_value(&path, "engine", "offline").expect("set bare string");
        set_value(&path, "text.remove_fillers", "false").expect("create table");
        let raw = fs::read_to_string(&path).expect("read");
        assert!(raw.contains("# 我的配置"));
        assert!(raw.contains("# 纠错"));
        let config = Config::load(&path).expect("load");
        assert!(config.llm.enabled);
        assert_eq!(config.engine, EngineMode::Offline);
        assert!(!config.text.remove_fillers);

        assert!(set_value(&path, "engine", "telepathy").is_err());
        assert_eq!(
            Config::load(&path).expect("load").engine,
            EngineMode::Offline,
            "invalid edits must not be written"
        );
    }

    #[test]
    fn get_value_reads_nested_keys() {
        let config = Config::default();
        assert_eq!(get_value(&config, "engine").expect("engine"), "auto");
        assert_eq!(get_value(&config, "llm.enabled").expect("bool"), "false");
        assert!(get_value(&config, "llm.nope").is_err());
    }

    #[test]
    fn example_config_documents_the_defaults() {
        let example: Config = toml::from_str(include_str!("../examples/config.example.toml"))
            .expect("examples/config.example.toml must parse");
        assert_eq!(example, Config::default(), "示例配置必须与默认值一致");
    }

    #[test]
    fn engine_and_language_parse_from_strings() {
        assert_eq!(
            "cloud".parse::<EngineMode>().expect("cloud"),
            EngineMode::Cloud
        );
        assert_eq!("yue".parse::<Language>().expect("yue"), Language::Yue);
        assert!("fr".parse::<Language>().is_err());
    }
}
