use std::path::Path;

use anyhow::{bail, Context, Result};
use sherpa_onnx::{OfflineRecognizer, OfflineRecognizerConfig, OfflineSenseVoiceModelConfig, Wave};

use crate::config::{Config, OfflineModelPaths};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfflineTranscript {
    pub text: String,
    pub language: String,
}

pub struct SenseVoiceRecognizer {
    recognizer: OfflineRecognizer,
    language: String,
}

impl SenseVoiceRecognizer {
    pub fn from_config(config: &Config) -> Result<Self> {
        let paths = config.offline_model_paths();
        Self::from_paths(
            &paths,
            &config.offline.language,
            config.offline.use_itn,
            config.offline.num_threads,
        )
    }

    pub fn from_paths(
        paths: &OfflineModelPaths,
        language: &str,
        use_itn: bool,
        num_threads: i32,
    ) -> Result<Self> {
        validate_language(language)?;
        validate_model_paths(paths)?;
        if num_threads < 1 {
            bail!("SenseVoice num_threads 必须大于 0");
        }

        let mut recognizer_config = OfflineRecognizerConfig::default();
        recognizer_config.model_config.sense_voice = OfflineSenseVoiceModelConfig {
            model: Some(paths.model.to_string_lossy().into_owned()),
            language: Some(language.to_owned()),
            use_itn,
        };
        recognizer_config.model_config.tokens = Some(paths.tokens.to_string_lossy().into_owned());
        recognizer_config.model_config.provider = Some("cpu".to_owned());
        recognizer_config.model_config.num_threads = num_threads;

        let recognizer = OfflineRecognizer::create(&recognizer_config)
            .context("创建 SenseVoice-Small 离线识别器失败")?;
        Ok(Self {
            recognizer,
            language: language.to_owned(),
        })
    }

    pub fn transcribe_wav(&self, wav_path: impl AsRef<Path>) -> Result<OfflineTranscript> {
        let wav_path = wav_path.as_ref();
        let wav_path_str = wav_path
            .to_str()
            .with_context(|| format!("音频路径不是有效 UTF-8: {}", wav_path.display()))?;
        let wave = Wave::read(wav_path_str)
            .with_context(|| format!("读取音频失败: {}", wav_path.display()))?;
        if wave.samples().is_empty() {
            bail!("音频为空: {}", wav_path.display());
        }

        let stream = self.recognizer.create_stream();
        stream.accept_waveform(wave.sample_rate(), wave.samples());
        self.recognizer.decode(&stream);
        let result = stream
            .get_result()
            .context("SenseVoice-Small 未返回识别结果")?;
        Ok(OfflineTranscript {
            text: result.text,
            language: self.language.clone(),
        })
    }
}

fn validate_model_paths(paths: &OfflineModelPaths) -> Result<()> {
    if !paths.directory.is_dir() {
        bail!(
            "SenseVoice 模型目录不存在: {}；请下载并解压 SenseVoice-Small int8 包",
            paths.directory.display()
        );
    }
    if !paths.model.is_file() {
        bail!("SenseVoice 模型文件不存在: {}", paths.model.display());
    }
    if !paths.tokens.is_file() {
        bail!("SenseVoice tokens 文件不存在: {}", paths.tokens.display());
    }
    Ok(())
}

fn validate_language(language: &str) -> Result<()> {
    match language {
        "auto" | "zh" | "en" | "yue" | "ja" | "ko" => Ok(()),
        _ => bail!("不支持的 SenseVoice 语言: {language}；可选 auto/zh/en/yue/ja/ko"),
    }
}

#[cfg(test)]
mod tests {
    use super::{validate_language, validate_model_paths};
    use crate::config::{Config, OfflineModelPaths};

    #[test]
    fn accepts_all_sense_voice_languages() {
        for language in ["auto", "zh", "en", "yue", "ja", "ko"] {
            assert!(validate_language(language).is_ok(), "{language}");
        }
        assert!(validate_language("fr").is_err());
    }

    #[test]
    fn reports_missing_model_files_without_loading_native_runtime() {
        let mut config = Config::default();
        config.offline.model_dir = "/tmp/yuda-no-sensevoice-model".to_owned();
        let paths = OfflineModelPaths::from_config(&config.offline);
        let error = validate_model_paths(&paths).expect_err("missing model should fail");
        assert!(error.to_string().contains("模型目录不存在"));
    }
}
