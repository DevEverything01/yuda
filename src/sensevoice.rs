//! 离线引擎：官方 sherpa-onnx 的 SenseVoice-Small int8 + Silero VAD。
//!
//! SenseVoice 是非流式模型。为了像豆包一样边说边出字，这里用 sherpa-onnx 推荐的
//! 「模拟流式」做法：VAD 把语音切成句段，已结束的句段解码一次后固定下来；
//! 正在说的尾段每隔几百毫秒重新解码一次作为实时预览。松手时只需解码最后一段，
//! 所以最终结果几乎没有等待。

use anyhow::{bail, Context, Result};
use sherpa_onnx::{
    LinearResampler, OfflineRecognizer, OfflineRecognizerConfig, OfflineSenseVoiceModelConfig,
    SileroVadModelConfig, VadModelConfig, VoiceActivityDetector, Wave,
};
use std::{
    path::Path,
    time::{Duration, Instant},
};

use crate::{
    config::{Config, OfflineModelPaths},
    text::join_segments,
};

pub const SAMPLE_RATE: i32 = 16_000;
/// Silero VAD 每次处理 512 个样本（32 ms）。
const VAD_WINDOW: usize = 512;
/// 两次实时预览之间至少间隔多久。
const PARTIAL_INTERVAL: Duration = Duration::from_millis(400);
/// 尾段短于该长度不做预览解码。
const MIN_TAIL_SAMPLES: usize = SAMPLE_RATE as usize * 3 / 10;

/// 整段兜底解码前要求的最低能量（100 ms 窗口 RMS 的峰值）。低于它基本是静音或底噪，
/// 送进 SenseVoice 只会得到「그.」一类的幻听。
const MIN_SPEECH_DBFS: f64 = -45.0;

pub struct SenseVoice {
    recognizer: OfflineRecognizer,
    vad: Option<VadModelConfig>,
}

impl SenseVoice {
    pub fn load(config: &Config) -> Result<Self> {
        // 实测 zh 提示下英文与粤语仍能正确输出，且比 auto 更不容易把短句误判成日韩语。
        Self::load_with_language(config, config.language.as_str())
    }

    /// 直接指定 SenseVoice 的语言提示（auto / zh / en / yue / ja / ko），用于评估。
    pub fn load_with_language(config: &Config, language: &str) -> Result<Self> {
        let paths = config.offline_model_paths();
        validate_model_paths(&paths)?;
        let mut recognizer_config = OfflineRecognizerConfig::default();
        recognizer_config.model_config.sense_voice = OfflineSenseVoiceModelConfig {
            model: Some(paths.model.to_string_lossy().into_owned()),
            language: Some(language.to_owned()),
            use_itn: config.offline.use_itn,
        };
        recognizer_config.model_config.tokens = Some(paths.tokens.to_string_lossy().into_owned());
        recognizer_config.model_config.provider = Some("cpu".to_owned());
        recognizer_config.model_config.num_threads = config.offline.num_threads;
        let recognizer = OfflineRecognizer::create(&recognizer_config)
            .context("创建 SenseVoice-Small 离线识别器失败")?;

        let vad = if paths.vad.is_file() {
            Some(VadModelConfig {
                silero_vad: SileroVadModelConfig {
                    model: Some(paths.vad.to_string_lossy().into_owned()),
                    threshold: 0.5,
                    min_silence_duration: 0.5,
                    min_speech_duration: 0.25,
                    window_size: VAD_WINDOW as i32,
                    max_speech_duration: 20.0,
                },
                sample_rate: SAMPLE_RATE,
                num_threads: 1,
                provider: Some("cpu".to_owned()),
                ..VadModelConfig::default()
            })
        } else {
            tracing::warn!(path = %paths.vad.display(), "缺少 Silero VAD 模型，离线识别退化为整段解码（无实时预览）");
            None
        };
        Ok(Self { recognizer, vad })
    }

    /// 解码一段 16 kHz 单声道音频。
    pub fn decode(&self, samples: &[f32]) -> String {
        if samples.is_empty() {
            return String::new();
        }
        let stream = self.recognizer.create_stream();
        stream.accept_waveform(SAMPLE_RATE, samples);
        self.recognizer.decode(&stream);
        stream
            .get_result()
            .map(|result| result.text.trim().to_owned())
            .unwrap_or_default()
    }

    /// 用与 daemon 相同的流式路径（VAD 分段 + 能量门）转写 WAV 文件。
    pub fn transcribe_wav(&self, path: &Path) -> Result<String> {
        let path_str = path
            .to_str()
            .with_context(|| format!("音频路径不是有效 UTF-8: {}", path.display()))?;
        let wave =
            Wave::read(path_str).with_context(|| format!("读取音频失败: {}", path.display()))?;
        if wave.samples().is_empty() {
            bail!("音频为空: {}", path.display());
        }
        let samples = if wave.sample_rate() == SAMPLE_RATE {
            wave.samples().to_vec()
        } else {
            LinearResampler::create(wave.sample_rate(), SAMPLE_RATE)
                .context("创建重采样器失败")?
                .resample(wave.samples(), true)
        };
        let mut session = self.session();
        for chunk in samples.chunks(SAMPLE_RATE as usize / 10) {
            session.accept(chunk);
        }
        Ok(session.finish())
    }

    /// 开始一次流式会话。
    pub fn session(&self) -> StreamingSession<'_> {
        let vad = self.vad.as_ref().and_then(|config| {
            let detector = VoiceActivityDetector::create(config, 120.0);
            if detector.is_none() {
                tracing::warn!("创建 Silero VAD 失败，本次会话退化为整段解码");
            }
            detector
        });
        StreamingSession {
            engine: self,
            vad,
            audio: Vec::new(),
            vad_cursor: 0,
            committed: String::new(),
            tail_start: 0,
            last_partial: None,
            partial_text: String::new(),
        }
    }
}

pub struct StreamingSession<'a> {
    engine: &'a SenseVoice,
    vad: Option<VoiceActivityDetector>,
    audio: Vec<f32>,
    /// 已送入 VAD 的样本数。
    vad_cursor: usize,
    /// 已固定的句段文本。
    committed: String,
    /// 尚未固定的尾段起点（样本下标）。
    tail_start: usize,
    last_partial: Option<Instant>,
    partial_text: String,
}

impl StreamingSession<'_> {
    pub fn accept(&mut self, samples: &[f32]) {
        self.audio.extend_from_slice(samples);
        let Some(vad) = &self.vad else {
            return;
        };
        while self.audio.len() - self.vad_cursor >= VAD_WINDOW {
            vad.accept_waveform(&self.audio[self.vad_cursor..self.vad_cursor + VAD_WINDOW]);
            self.vad_cursor += VAD_WINDOW;
        }
        self.drain_segments();
    }

    fn drain_segments(&mut self) {
        let Some(vad) = &self.vad else {
            return;
        };
        while !vad.is_empty() {
            let Some(segment) = vad.front() else {
                break;
            };
            let text = self.engine.decode(segment.samples());
            self.committed = join_segments(&self.committed, &text);
            let end = usize::try_from(segment.start()).unwrap_or(0) + segment.samples().len();
            self.tail_start = self.tail_start.max(end.min(self.audio.len()));
            vad.pop();
        }
    }

    /// 返回新的预览文本；与上次相同或还没到刷新时间时返回 None。
    pub fn partial(&mut self) -> Option<String> {
        if self
            .last_partial
            .is_some_and(|last| last.elapsed() < PARTIAL_INTERVAL)
        {
            return None;
        }
        self.last_partial = Some(Instant::now());
        let tail = &self.audio[self.tail_start.min(self.audio.len())..];
        let text = if self.vad.is_some() {
            if tail.len() >= MIN_TAIL_SAMPLES {
                join_segments(&self.committed, &self.engine.decode(tail))
            } else {
                self.committed.clone()
            }
        } else if self.audio.len() <= SAMPLE_RATE as usize * 30 {
            // 没有 VAD：30 秒内整段重解码，更长时只在松手后解码。
            self.engine.decode(&self.audio)
        } else {
            return None;
        };
        if text == self.partial_text || text.is_empty() {
            return None;
        }
        self.partial_text = text.clone();
        Some(text)
    }

    /// 松手：处理剩余音频，返回整段最终文本。
    pub fn finish(mut self) -> String {
        if let Some(vad) = &self.vad {
            let rest = &self.audio[self.vad_cursor..];
            if !rest.is_empty() {
                vad.accept_waveform(rest);
                self.vad_cursor = self.audio.len();
            }
            vad.flush();
        }
        self.drain_segments();
        if !self.committed.is_empty() {
            return self.committed;
        }
        // 没有 VAD，或 VAD 一段语音都没切出来（声音很轻）：有足够能量才整段解码兜底。
        if peak_dbfs(&self.audio) < MIN_SPEECH_DBFS {
            return String::new();
        }
        self.engine.decode(&self.audio)
    }

    pub fn duration(&self) -> Duration {
        Duration::from_secs_f32(self.audio.len() as f32 / SAMPLE_RATE as f32)
    }
}

/// 100 ms 窗口 RMS 的峰值（dBFS）。
pub fn peak_dbfs(samples: &[f32]) -> f64 {
    let window = SAMPLE_RATE as usize / 10;
    samples
        .chunks(window)
        .map(|chunk| {
            let energy: f64 = chunk.iter().map(|s| f64::from(*s) * f64::from(*s)).sum();
            (energy / chunk.len() as f64).sqrt()
        })
        .fold(f64::NEG_INFINITY, f64::max)
        .max(1e-9)
        .log10()
        * 20.0
}

fn validate_model_paths(paths: &OfflineModelPaths) -> Result<()> {
    if !paths.directory.is_dir() {
        bail!(
            "SenseVoice 模型目录不存在: {}；运行 omarchy-install-dictation-yuda 下载模型",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_and_noise_floor_are_below_the_speech_gate() {
        assert!(peak_dbfs(&[0.0; 16_000]) < MIN_SPEECH_DBFS);
        let hum: Vec<f32> = (0..16_000)
            .map(|i| 0.001 * (i as f32 / 10.0).sin())
            .collect();
        assert!(peak_dbfs(&hum) < MIN_SPEECH_DBFS);
        let speech: Vec<f32> = (0..16_000).map(|i| 0.2 * (i as f32 / 10.0).sin()).collect();
        assert!(peak_dbfs(&speech) > MIN_SPEECH_DBFS);
        assert_eq!(peak_dbfs(&[]), -180.0);
    }

    #[test]
    fn reports_missing_model_files_without_loading_native_runtime() {
        let mut config = Config::default();
        config.offline.model_dir = "/tmp/yuda-no-sensevoice-model".to_owned();
        let error = validate_model_paths(&config.offline_model_paths())
            .expect_err("missing model should fail");
        assert!(error.to_string().contains("模型目录不存在"));
    }
}
