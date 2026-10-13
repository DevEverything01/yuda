//! 麦克风采集：cpal（PipeWire/ALSA）→ 单声道 → 16 kHz f32 流，同时计算电平。
//!
//! cpal 的 Stream 不保证可跨线程，因此每次录音在独立线程里创建并持有；
//! `Capture::stop` 关闭流并等待线程结束，保证此前采到的样本都已交给回调。

use anyhow::{bail, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::{
    sync::mpsc,
    thread::{self, JoinHandle},
    time::Duration,
};

/// 识别引擎统一使用的采样率。
pub const SAMPLE_RATE: u32 = 16_000;
/// 电平回调的窗口：约 33 ms，对应 30 Hz 刷新。
const LEVEL_WINDOW: usize = (SAMPLE_RATE as usize) / 30;

pub struct Capture {
    stop: mpsc::Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl Capture {
    /// 打开默认麦克风开始采集。`on_samples` 收到 16 kHz 单声道样本，
    /// `on_level` 收到 0.0–1.0 的电平；两者都在音频线程调用，必须很快返回。
    pub fn start<S, L>(on_samples: S, on_level: L) -> Result<Self>
    where
        S: FnMut(&[f32]) + Send + 'static,
        L: FnMut(f32) + Send + 'static,
    {
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<String>>(1);
        let thread = thread::Builder::new()
            .name("yuda-capture".to_owned())
            .spawn(move || {
                let stream = match open_stream(on_samples, on_level) {
                    Ok((stream, description)) => {
                        let _ = ready_tx.send(Ok(description));
                        stream
                    }
                    Err(error) => {
                        let _ = ready_tx.send(Err(error));
                        return;
                    }
                };
                // 等待 stop 信号或 Capture 被丢弃。
                let _ = stop_rx.recv();
                if let Err(error) = stream.pause() {
                    tracing::debug!(%error, "暂停麦克风输入流失败");
                }
                drop(stream);
            })
            .context("启动录音线程失败")?;
        match ready_rx.recv_timeout(Duration::from_secs(3)) {
            Ok(Ok(description)) => {
                tracing::debug!(device = %description, "麦克风已打开");
                Ok(Self {
                    stop: stop_tx,
                    thread: Some(thread),
                })
            }
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => bail!("打开麦克风超时（3 秒）；请检查 PipeWire 是否正常"),
        }
    }

    /// 停止采集并等待音频线程退出。
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            if thread.join().is_err() {
                tracing::error!("录音线程异常退出");
            }
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn open_stream<S, L>(on_samples: S, on_level: L) -> Result<(cpal::Stream, String)>
where
    S: FnMut(&[f32]) + Send + 'static,
    L: FnMut(f32) + Send + 'static,
{
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .context("找不到麦克风；请确认 PipeWire 中有可用的输入设备")?;
    let name = device.name().unwrap_or_else(|_| "未知设备".to_owned());
    let (config, format) = pick_config(&device)?;
    let channels = usize::from(config.channels);
    let rate = config.sample_rate.0;
    if rate != SAMPLE_RATE {
        tracing::info!(rate, "麦克风不支持 16 kHz，使用线性重采样");
    }
    let mut pipeline = Pipeline::new(channels, rate, on_samples, on_level);
    let on_error = |error| tracing::error!(%error, "麦克风输入流错误");
    let stream = match format {
        cpal::SampleFormat::F32 => device.build_input_stream(
            &config,
            move |data: &[f32], _| pipeline.push(data.iter().copied()),
            on_error,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_input_stream(
            &config,
            move |data: &[i16], _| pipeline.push(data.iter().map(|s| f32::from(*s) / 32_768.0)),
            on_error,
            None,
        ),
        cpal::SampleFormat::I32 => device.build_input_stream(
            &config,
            move |data: &[i32], _| pipeline.push(data.iter().map(|s| *s as f32 / 2_147_483_648.0)),
            on_error,
            None,
        ),
        cpal::SampleFormat::U16 => device.build_input_stream(
            &config,
            move |data: &[u16], _| {
                pipeline.push(data.iter().map(|s| (f32::from(*s) - 32_768.0) / 32_768.0))
            },
            on_error,
            None,
        ),
        other => bail!("不支持的麦克风采样格式: {other:?}"),
    }
    .context("创建麦克风输入流失败")?;
    stream.play().context("启动麦克风输入流失败")?;
    Ok((stream, format!("{name} {rate} Hz × {channels}")))
}

/// 优先选择原生 16 kHz（PipeWire 会替我们重采样），否则用默认配置。
fn pick_config(device: &cpal::Device) -> Result<(cpal::StreamConfig, cpal::SampleFormat)> {
    let supported = [
        cpal::SampleFormat::F32,
        cpal::SampleFormat::I16,
        cpal::SampleFormat::I32,
        cpal::SampleFormat::U16,
    ];
    if let Ok(ranges) = device.supported_input_configs() {
        let mut candidates: Vec<_> = ranges
            .filter(|range| supported.contains(&range.sample_format()))
            .filter(|range| {
                range.min_sample_rate().0 <= SAMPLE_RATE && SAMPLE_RATE <= range.max_sample_rate().0
            })
            .collect();
        // 单声道、f32 优先。
        candidates.sort_by_key(|range| {
            (
                range.channels(),
                range.sample_format() != cpal::SampleFormat::F32,
            )
        });
        if let Some(range) = candidates.into_iter().next() {
            let format = range.sample_format();
            let config = range
                .with_sample_rate(cpal::SampleRate(SAMPLE_RATE))
                .config();
            return Ok((config, format));
        }
    }
    let default = device
        .default_input_config()
        .context("读取麦克风默认配置失败")?;
    Ok((default.config(), default.sample_format()))
}

/// 下混 → 重采样 → 回调，并按窗口计算电平。
struct Pipeline<S, L> {
    channels: usize,
    resampler: Option<LinearResampler>,
    frame: Vec<f32>,
    mono: Vec<f32>,
    output: Vec<f32>,
    level: LevelMeter,
    on_samples: S,
    on_level: L,
}

impl<S, L> Pipeline<S, L>
where
    S: FnMut(&[f32]),
    L: FnMut(f32),
{
    fn new(channels: usize, rate: u32, on_samples: S, on_level: L) -> Self {
        Self {
            channels: channels.max(1),
            resampler: (rate != SAMPLE_RATE).then(|| LinearResampler::new(rate, SAMPLE_RATE)),
            frame: Vec::new(),
            mono: Vec::new(),
            output: Vec::new(),
            level: LevelMeter::default(),
            on_samples,
            on_level,
        }
    }

    fn push(&mut self, samples: impl Iterator<Item = f32>) {
        self.mono.clear();
        for sample in samples {
            self.frame.push(sample);
            if self.frame.len() == self.channels {
                let sum: f32 = self.frame.iter().sum();
                self.mono.push(sum / self.channels as f32);
                self.frame.clear();
            }
        }
        let output = match &mut self.resampler {
            Some(resampler) => {
                self.output.clear();
                resampler.process(&self.mono, &mut self.output);
                &self.output
            }
            None => &self.mono,
        };
        if output.is_empty() {
            return;
        }
        for &sample in output {
            if let Some(level) = self.level.push(sample) {
                (self.on_level)(level);
            }
        }
        (self.on_samples)(output);
    }
}

/// RMS 电平：每 33 ms 输出一次，按 -60…-12 dBFS 线性映射到 0…1。
#[derive(Default)]
pub struct LevelMeter {
    sum_squares: f64,
    count: usize,
}

impl LevelMeter {
    pub fn push(&mut self, sample: f32) -> Option<f32> {
        self.sum_squares += f64::from(sample) * f64::from(sample);
        self.count += 1;
        if self.count < LEVEL_WINDOW {
            return None;
        }
        let rms = (self.sum_squares / self.count as f64).sqrt();
        self.sum_squares = 0.0;
        self.count = 0;
        Some(level_from_rms(rms))
    }
}

pub fn level_from_rms(rms: f64) -> f32 {
    if rms <= 0.0 {
        return 0.0;
    }
    let db = 20.0 * rms.log10();
    (((db + 60.0) / 48.0).clamp(0.0, 1.0)) as f32
}

/// 流式线性重采样；跨块保持相位。
pub struct LinearResampler {
    step: f64,
    /// 相对「上一块最后一个样本 + 本块」虚拟缓冲起点的读取位置。
    position: f64,
    carry: Option<f32>,
}

impl LinearResampler {
    pub fn new(from: u32, to: u32) -> Self {
        Self {
            step: f64::from(from) / f64::from(to),
            position: 0.0,
            carry: None,
        }
    }

    pub fn process(&mut self, input: &[f32], output: &mut Vec<f32>) {
        let Some(&last) = input.last() else {
            return;
        };
        let offset = usize::from(self.carry.is_some());
        let carry = self.carry.unwrap_or(last);
        let at = |index: usize| {
            if index < offset {
                carry
            } else {
                input[index - offset]
            }
        };
        let end = (offset + input.len() - 1) as f64;
        while self.position < end {
            let index = self.position.floor();
            let fraction = (self.position - index) as f32;
            let index = index as usize;
            let left = at(index);
            output.push(left + (at(index + 1) - left) * fraction);
            self.position += self.step;
        }
        // 本块最后一个样本成为下一块的索引 0。
        self.position -= end;
        self.carry = Some(last);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_maps_silence_to_zero_and_loud_speech_to_one() {
        assert_eq!(level_from_rms(0.0), 0.0);
        assert!(level_from_rms(0.000_1) < 0.05);
        assert!((level_from_rms(0.3) - 1.0).abs() < 1e-6);
        let mid = level_from_rms(0.01);
        assert!(
            mid > 0.3 && mid < 0.6,
            "-40 dBFS should be mid-scale, got {mid}"
        );
    }

    #[test]
    fn meter_emits_thirty_times_per_second() {
        let mut meter = LevelMeter::default();
        let emitted = (0..SAMPLE_RATE).filter_map(|_| meter.push(0.1)).count();
        assert_eq!(emitted, 30);
    }

    #[test]
    fn resampler_keeps_duration_across_chunks() {
        let mut resampler = LinearResampler::new(48_000, 16_000);
        let input: Vec<f32> = (0..48_000).map(|i| (i as f32 / 48.0).sin()).collect();
        let mut output = Vec::new();
        for chunk in input.chunks(480) {
            resampler.process(chunk, &mut output);
        }
        let expected = 16_000_i64;
        assert!(
            (output.len() as i64 - expected).abs() <= 2,
            "expected about {expected} samples, got {}",
            output.len()
        );
    }

    #[test]
    fn resampler_is_identity_shaped_for_upsampling_constant_signal() {
        let mut resampler = LinearResampler::new(8_000, 16_000);
        let mut output = Vec::new();
        for _ in 0..10 {
            resampler.process(&[0.5; 80], &mut output);
        }
        assert!((output.len() as i64 - 1_600).abs() <= 2);
        assert!(output.iter().all(|sample| (sample - 0.5).abs() < 1e-6));
    }

    #[test]
    fn pipeline_downmixes_stereo() {
        let mut collected = Vec::new();
        let mut pipeline = Pipeline::new(
            2,
            SAMPLE_RATE,
            |s: &[f32]| collected.extend_from_slice(s),
            |_| {},
        );
        pipeline.push([1.0, 0.0, 0.5, 0.5].into_iter());
        drop(pipeline);
        assert_eq!(collected, vec![0.5, 0.5]);
    }
}
