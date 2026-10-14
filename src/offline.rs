//! 离线引擎：官方 sherpa-onnx 运行时 + Silero VAD。
//!
//! 中文 / 英文默认用 zipformer transducer 中英模型，粤语 / 日语 / 韩语用 SenseVoice-Small；
//! 模型族按目录里的文件自动识别（`config::ModelFiles`）。
//!
//! 两者都按非流式方式使用。为了像豆包一样边说边出字，这里用 sherpa-onnx 推荐的
//! 「模拟流式」做法：VAD 把语音切成句段，已结束的句段解码一次后固定下来；
//! 正在说的尾段每隔几百毫秒重新解码一次作为实时预览。松手时只需解码最后一段，
//! 所以最终结果几乎没有等待。
//!
//! VAD 只决定在哪里切，不决定丢掉什么：句段前后补边距，VAD 漏判的有声音频（句段之间、
//! 最后一段之后的轻声）并入相邻句段或在松手时解码。

use anyhow::{bail, Context, Result};
use sherpa_onnx::{
    LinearResampler, OfflineRecognizer, OfflineRecognizerConfig, OfflineSenseVoiceModelConfig,
    OfflineTransducerModelConfig, SileroVadModelConfig, VadModelConfig, VoiceActivityDetector,
    Wave,
};
use std::{
    path::Path,
    time::{Duration, Instant},
};

use crate::{
    config::{Config, ModelFiles},
    text::join_segments,
};

pub const SAMPLE_RATE: i32 = 16_000;
/// Silero VAD 每次处理 512 个样本（32 ms）。
const VAD_WINDOW: usize = 512;
/// 两次实时预览之间至少间隔多久。
const PARTIAL_INTERVAL: Duration = Duration::from_millis(400);
/// 尾段短于该长度不做预览解码。
const MIN_TAIL_SAMPLES: usize = SAMPLE_RATE as usize * 3 / 10;
/// 句段解码时前后各多取的音频。Silero VAD 的句段起点随 512 样本窗口与语音开头的相位
/// 变化，实测可比语音开头晚 0.14 s，第一个字被切掉（「把九点改成十点」→「九点改成十点」）。
const SEGMENT_PADDING: usize = SAMPLE_RATE as usize / 5;

/// VAD 句段之外的音频要解码时要求的最低能量（100 ms 窗口 RMS 的峰值）。低于它基本是
/// 静音或底噪，送进模型只会得到「그.」一类的幻听。
const MIN_SPEECH_DBFS: f64 = -45.0;

pub struct OfflineEngine {
    recognizer: OfflineRecognizer,
    vad: Option<VadModelConfig>,
    /// 模型目录名，用于日志。
    name: String,
}

fn path_string(path: &Path) -> Option<String> {
    Some(path.to_string_lossy().into_owned())
}

impl OfflineEngine {
    /// 按配置的语言加载对应模型。
    pub fn load(config: &Config) -> Result<Self> {
        let paths = config.offline_model_paths();
        let files = paths.files()?;
        let mut recognizer_config = OfflineRecognizerConfig::default();
        let model_config = &mut recognizer_config.model_config;
        match &files {
            ModelFiles::SenseVoice { model, tokens } => {
                // 实测 zh 提示下英文与粤语仍能正确输出，且比 auto 更不容易把短句误判成日韩语。
                model_config.sense_voice = OfflineSenseVoiceModelConfig {
                    model: path_string(model),
                    language: Some(config.language.as_str().to_owned()),
                    use_itn: config.offline.use_itn,
                };
                model_config.tokens = path_string(tokens);
            }
            ModelFiles::Transducer {
                encoder,
                decoder,
                joiner,
                tokens,
            } => {
                model_config.transducer = OfflineTransducerModelConfig {
                    encoder: path_string(encoder),
                    decoder: path_string(decoder),
                    joiner: path_string(joiner),
                };
                model_config.tokens = path_string(tokens);
                recognizer_config.decoding_method = Some("greedy_search".to_owned());
            }
        }
        let model_config = &mut recognizer_config.model_config;
        model_config.provider = Some("cpu".to_owned());
        model_config.num_threads = config.offline.num_threads;
        let name = paths
            .directory
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let recognizer = OfflineRecognizer::create(&recognizer_config)
            .with_context(|| format!("创建离线识别器失败：{name}"))?;

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
        Ok(Self {
            recognizer,
            vad,
            name,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
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
        Ok(self.transcribe_samples(&samples))
    }

    /// 用流式路径（VAD 分段 + 能量门）转写一整段 16 kHz 音频。
    pub fn transcribe_samples(&self, samples: &[f32]) -> String {
        let mut session = self.session();
        for chunk in samples.chunks(SAMPLE_RATE as usize / 10) {
            session.accept(chunk);
        }
        session.finish()
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
            last_decode: Duration::ZERO,
            partial_text: String::new(),
        }
    }
}

pub struct StreamingSession<'a> {
    engine: &'a OfflineEngine,
    vad: Option<VoiceActivityDetector>,
    audio: Vec<f32>,
    /// 已送入 VAD 的样本数。
    vad_cursor: usize,
    /// 已固定的句段文本。
    committed: String,
    /// 尚未固定的尾段起点（样本下标）。
    tail_start: usize,
    last_partial: Option<Instant>,
    /// 上一次预览解码的耗时；刷新间隔至少是它的两倍，慢机器上不会被预览拖垮。
    last_decode: Duration,
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
            let start = usize::try_from(segment.start()).unwrap_or(0);
            let (from, to) =
                segment_range(&self.audio, start, segment.samples().len(), self.tail_start);
            let text = self.engine.decode(&self.audio[from..to]);
            self.committed = join_segments(&self.committed, &text);
            self.tail_start = self.tail_start.max(to);
            vad.pop();
        }
    }

    /// 返回新的预览文本；与上次相同或还没到刷新时间时返回 None。
    pub fn partial(&mut self) -> Option<String> {
        let interval = PARTIAL_INTERVAL.max(self.last_decode * 2);
        if self
            .last_partial
            .is_some_and(|last| last.elapsed() < interval)
        {
            return None;
        }
        let started = Instant::now();
        self.last_partial = Some(started);
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
        self.last_decode = started.elapsed();
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
        // 最后一段之后还有声音：VAD 漏判的轻声（停顿后小声补的半句），或没有 VAD、
        // VAD 一段都没切出来。有足够能量才解码。
        let rest = &self.audio[self.tail_start.min(self.audio.len())..];
        if peak_dbfs(rest) >= MIN_SPEECH_DBFS {
            let text = self.engine.decode(rest);
            self.committed = join_segments(&self.committed, &text);
        }
        self.committed
    }

    pub fn duration(&self) -> Duration {
        Duration::from_secs_f32(self.audio.len() as f32 / SAMPLE_RATE as f32)
    }
}

/// 句段在整段音频里的解码范围：前后各加 `SEGMENT_PADDING`，不早于上一段解码的终点，
/// 不超出已录音频。与上一段之间的音频有声音（VAD 漏判的轻声）时从上一段终点开始，
/// 不丢有声音的音频。
fn segment_range(audio: &[f32], start: usize, len: usize, previous_end: usize) -> (usize, usize) {
    let total = audio.len();
    let previous_end = previous_end.min(total);
    let mut from = start
        .saturating_sub(SEGMENT_PADDING)
        .clamp(previous_end, total);
    if peak_dbfs(&audio[previous_end..from]) >= MIN_SPEECH_DBFS {
        from = previous_end;
    }
    let to = (start + len + SEGMENT_PADDING).min(total).max(from);
    (from, to)
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
    fn segments_are_decoded_with_context_but_never_overlap() {
        let pad = SEGMENT_PADDING;
        let silence = vec![0.0; 80_000];
        // 语音前有足够的静音：前后各补 pad。
        assert_eq!(
            segment_range(&silence, 32_000, 16_000, 0),
            (32_000 - pad, 48_000 + pad)
        );
        // 开头和结尾不越界。
        assert_eq!(
            segment_range(&silence[..9_500], 1_000, 8_000, 0),
            (0, 9_500)
        );
        // 不回退到上一段已经解码过的音频。
        assert_eq!(
            segment_range(&silence, 40_000, 8_000, 39_000),
            (39_000, 48_000 + pad)
        );
    }

    #[test]
    fn audible_audio_that_vad_skipped_joins_the_next_segment() {
        // 上一段在 16 000 结束；24 000–28 000 是 VAD 没判为语音的轻声（约 -30 dBFS）。
        let mut audio = vec![0.0; 80_000];
        for (i, sample) in audio[24_000..28_000].iter_mut().enumerate() {
            *sample = 0.03 * (i as f32 / 10.0).sin();
        }
        assert_eq!(
            segment_range(&audio, 48_000, 8_000, 16_000),
            (16_000, 56_000 + SEGMENT_PADDING)
        );
    }
}
