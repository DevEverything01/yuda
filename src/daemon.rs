use anyhow::{Context, Result};
use std::{
    fs,
    path::PathBuf,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::{
    app::{Session, Transcript},
    audio::AudioRecorder,
    config::Config,
    hotkey::{HotkeyAction, HotkeyListener},
    injection::inject_text,
    offline_asr::SenseVoiceRecognizer,
};

const MIN_HOLD: Duration = Duration::from_millis(300);
const POST_RECORDING_DELAY: Duration = Duration::from_millis(20);

pub fn run() -> Result<()> {
    let config_path = Config::default_path().context("无法确定 HOME，不能加载 Yuda 配置")?;
    let config = Config::load(&config_path)
        .with_context(|| format!("加载配置失败: {}", config_path.display()))?;
    tracing::info!(path = %config_path.display(), hotkey = %config.hotkey, engine = %config.engine, "Yuda daemon 启动");

    if config.engine == "cloud" {
        tracing::warn!(
            "云端 ASR 尚未接入当前 daemon，改用本地 SenseVoice；请将 engine 设为 offline 或 auto"
        );
    }
    let recognizer = SenseVoiceRecognizer::from_config(&config)
        .context("初始化 SenseVoice 失败；请确认模型文件已下载且配置路径正确")?;
    let listener = HotkeyListener::start(&config.hotkey).context("启动全局热键监听失败")?;
    tracing::info!(
        "已监听 {}；按住至少 300ms 后说话，松开触发上屏",
        config.hotkey
    );

    let mut session = Session::default();
    loop {
        match listener.recv()? {
            HotkeyAction::Pressed => {
                if !session.start_recording() {
                    tracing::debug!(state = ?session.state, "忽略录音触发：当前会话忙碌");
                    continue;
                }
                let recorder = match AudioRecorder::start() {
                    Ok(recorder) => recorder,
                    Err(error) => {
                        session.fail(error.to_string());
                        tracing::error!(%error, "启动录音失败");
                        continue;
                    }
                };
                tracing::info!("开始录音");
                handle_recording(&config, &recognizer, &listener, &mut session, recorder)?;
            }
            HotkeyAction::Cancelled => {
                tracing::debug!("忽略取消的热键序列");
            }
            HotkeyAction::Released { .. } => {
                tracing::debug!("忽略没有对应按下事件的热键释放");
            }
        }
    }
}

fn handle_recording(
    config: &Config,
    recognizer: &SenseVoiceRecognizer,
    listener: &HotkeyListener,
    session: &mut Session,
    recorder: AudioRecorder,
) -> Result<()> {
    let action = listener.recv()?;
    match action {
        HotkeyAction::Cancelled => {
            tracing::info!("录音已取消");
            session.reset();
            drop(recorder);
            Ok(())
        }
        HotkeyAction::Pressed => {
            tracing::debug!("忽略重复热键按下");
            handle_recording(config, recognizer, listener, session, recorder)
        }
        HotkeyAction::Released { held_for } => {
            if held_for < MIN_HOLD {
                tracing::info!(held_ms = held_for.as_millis(), "忽略短按");
                session.reset();
                drop(recorder);
                return Ok(());
            }
            thread::sleep(POST_RECORDING_DELAY);
            let wav_path = temporary_wav_path()?;
            let recorded = recorder.finish(&wav_path).context("结束录音失败")?;
            tracing::info!(
                duration_ms = held_for.as_millis(),
                samples = recorded.samples,
                sample_rate = recorded.sample_rate,
                "录音结束"
            );
            session.begin_transcribing();
            session.begin_refining();
            let transcript = recognizer.transcribe_wav(&recorded.path);
            let _ = fs::remove_file(&recorded.path);
            let transcript = match transcript {
                Ok(transcript) if !transcript.text.trim().is_empty() => transcript,
                Ok(_) => {
                    let message = "SenseVoice 返回空文本";
                    session.fail(message);
                    tracing::warn!(message);
                    return Ok(());
                }
                Err(error) => {
                    session.fail(error.to_string());
                    tracing::error!(%error, "SenseVoice 转写失败");
                    return Ok(());
                }
            };
            let text = transcript.text;
            session.transcript = Some(Transcript::new(text.clone(), 0));
            session.mark_ready();
            session.begin_injection();
            match inject_text(
                &text,
                Duration::from_millis(config.injection.paste_delay_ms),
            ) {
                Ok(backend) => {
                    session.complete_injection();
                    tracing::info!(backend = backend.label(), "中文文本已上屏");
                }
                Err(error) => {
                    session.fail(error.to_string());
                    tracing::error!(%error, "文本上屏失败");
                }
            }
            Ok(())
        }
    }
}

fn temporary_wav_path() -> Result<PathBuf> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("系统时钟早于 Unix epoch")?
        .as_nanos();
    let path = std::env::temp_dir().join(format!("yuda-{timestamp}-{}.wav", std::process::id()));
    if path.exists() {
        anyhow::bail!("临时录音路径已存在: {}", path.display());
    }
    Ok(path)
}
