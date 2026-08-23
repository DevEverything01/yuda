use anyhow::{bail, Context, Result};
#[cfg(feature = "linux-runtime")]
use cpal::Sample;
use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

#[derive(Debug, Clone)]
pub struct RecordedAudio {
    pub path: PathBuf,
    pub sample_rate: u32,
    pub channels: u16,
    pub samples: usize,
}

#[derive(Debug, Default)]
struct PcmBuffer {
    samples: Vec<i16>,
    peak: f32,
}

pub struct AudioRecorder {
    stream: cpal::Stream,
    buffer: Arc<Mutex<PcmBuffer>>,
    sample_rate: u32,
}

impl AudioRecorder {
    pub fn start() -> Result<Self> {
        use cpal::traits::{DeviceTrait, HostTrait};

        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .context("找不到默认麦克风；请确认 PipeWire 输入设备可用")?;
        let supported = device
            .default_input_config()
            .context("读取默认麦克风配置失败")?;
        let sample_rate = supported.sample_rate().0;
        let channels = usize::from(supported.channels());
        if channels == 0 {
            bail!("默认麦克风报告了 0 个声道");
        }

        let buffer = Arc::new(Mutex::new(PcmBuffer::default()));
        let config = supported.config();
        let stream = match supported.sample_format() {
            cpal::SampleFormat::I8 => {
                build_stream::<i8>(&device, &config, Arc::clone(&buffer), channels)?
            }
            cpal::SampleFormat::I16 => {
                build_stream::<i16>(&device, &config, Arc::clone(&buffer), channels)?
            }
            cpal::SampleFormat::I32 => {
                build_stream::<i32>(&device, &config, Arc::clone(&buffer), channels)?
            }
            cpal::SampleFormat::I64 => {
                build_stream::<i64>(&device, &config, Arc::clone(&buffer), channels)?
            }
            cpal::SampleFormat::U8 => {
                build_stream::<u8>(&device, &config, Arc::clone(&buffer), channels)?
            }
            cpal::SampleFormat::U16 => {
                build_stream::<u16>(&device, &config, Arc::clone(&buffer), channels)?
            }
            cpal::SampleFormat::U32 => {
                build_stream::<u32>(&device, &config, Arc::clone(&buffer), channels)?
            }
            cpal::SampleFormat::U64 => {
                build_stream::<u64>(&device, &config, Arc::clone(&buffer), channels)?
            }
            cpal::SampleFormat::F32 => {
                build_stream::<f32>(&device, &config, Arc::clone(&buffer), channels)?
            }
            cpal::SampleFormat::F64 => {
                build_stream::<f64>(&device, &config, Arc::clone(&buffer), channels)?
            }
            format => bail!("不支持的麦克风采样格式: {format:?}"),
        };
        use cpal::traits::StreamTrait;
        stream.play().context("启动麦克风输入流失败")?;
        Ok(Self {
            stream,
            buffer,
            sample_rate,
        })
    }

    pub fn peak(&self) -> Result<f32> {
        Ok(self
            .buffer
            .lock()
            .map_err(|_| anyhow::anyhow!("音频缓冲区锁已中毒"))?
            .peak)
    }

    pub fn finish(self, path: impl AsRef<Path>) -> Result<RecordedAudio> {
        let path = path.as_ref().to_path_buf();
        drop(self.stream);
        let buffer = Arc::try_unwrap(self.buffer)
            .map_err(|_| anyhow::anyhow!("录音流关闭失败：回调仍持有音频缓冲区"))?
            .into_inner()
            .map_err(|_| anyhow::anyhow!("读取音频缓冲区失败"))?;
        if buffer.samples.is_empty() {
            bail!("录音为空；没有采集到麦克风样本");
        }
        write_wav(&path, self.sample_rate, &buffer.samples)?;
        Ok(RecordedAudio {
            path,
            sample_rate: self.sample_rate,
            channels: 1,
            samples: buffer.samples.len(),
        })
    }
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    buffer: Arc<Mutex<PcmBuffer>>,
    channels: usize,
) -> Result<cpal::Stream>
where
    T: cpal::SizedSample + cpal::Sample + Send + 'static,
    f32: cpal::FromSample<T>,
{
    use cpal::traits::DeviceTrait;
    device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                if let Ok(mut buffer) = buffer.lock() {
                    let mut peak = 0.0_f32;
                    for frame in data.chunks(channels) {
                        let mono = frame
                            .iter()
                            .map(|sample| f32::from_sample(*sample))
                            .sum::<f32>()
                            / frame.len() as f32;
                        peak = peak.max(mono.abs());
                        buffer
                            .samples
                            .push((mono.clamp(-1.0, 1.0) * i16::MAX as f32) as i16);
                    }
                    buffer.peak = buffer.peak * 0.85 + peak * 0.15;
                }
            },
            |error| tracing::error!(%error, "音频输入流错误"),
            None,
        )
        .context("创建麦克风输入流失败")
}

fn write_wav(path: &Path, sample_rate: u32, samples: &[i16]) -> Result<()> {
    let mut file =
        File::create(path).with_context(|| format!("创建 WAV 文件失败: {}", path.display()))?;
    let data_len = samples
        .len()
        .checked_mul(std::mem::size_of::<i16>())
        .context("录音太大，WAV 数据长度溢出")?;
    let data_len_u32 = u32::try_from(data_len).context("录音超过 WAV 格式支持的大小")?;
    let riff_len = 36_u32
        .checked_add(data_len_u32)
        .context("录音太大，WAV 文件长度溢出")?;
    file.write_all(b"RIFF")?;
    file.write_all(&riff_len.to_le_bytes())?;
    file.write_all(b"WAVEfmt ")?;
    file.write_all(&16_u32.to_le_bytes())?;
    file.write_all(&1_u16.to_le_bytes())?;
    file.write_all(&1_u16.to_le_bytes())?;
    file.write_all(&sample_rate.to_le_bytes())?;
    let byte_rate = sample_rate.checked_mul(2).context("WAV 字节率溢出")?;
    file.write_all(&byte_rate.to_le_bytes())?;
    file.write_all(&2_u16.to_le_bytes())?;
    file.write_all(&16_u16.to_le_bytes())?;
    file.write_all(b"data")?;
    file.write_all(&data_len_u32.to_le_bytes())?;
    for sample in samples {
        file.write_all(&sample.to_le_bytes())?;
    }
    file.flush().context("写入 WAV 文件失败")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::write_wav;
    use std::{
        fs,
        io::Read,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn writes_mono_pcm16_wav_header_and_samples() {
        let path = std::env::temp_dir().join(format!(
            "yuda-audio-{}.wav",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock should be after epoch")
                .as_nanos()
        ));
        write_wav(&path, 16_000, &[0, i16::MAX, -1]).expect("WAV should write");
        let mut bytes = Vec::new();
        fs::File::open(&path)
            .expect("WAV should exist")
            .read_to_end(&mut bytes)
            .expect("WAV should read");
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(u16::from_le_bytes(bytes[22..24].try_into().unwrap()), 1);
        assert_eq!(&bytes[36..40], b"data");
        assert_eq!(u32::from_le_bytes(bytes[40..44].try_into().unwrap()), 6);
        let _ = fs::remove_file(path);
    }
}
