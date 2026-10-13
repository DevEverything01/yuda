//! 识别工作线程：持有常驻内存的离线模型，按会话选择云端或离线引擎，
//! 把实时预览与最终结果以事件形式交给 daemon。
//!
//! 路由规则（engine = auto）：云端已配置且在 connect_timeout 内连上 → 云端；
//! 否则离线，并标记 fallback；云端中途失败 → 用缓存的整段音频离线重识别。

use anyhow::{Context, Result};
use std::{
    sync::{
        mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError},
        Arc,
    },
    thread,
    time::Duration,
};

use crate::{
    config::{Config, EngineMode},
    ipc::Engine,
    offline::OfflineEngine,
};

pub enum Request {
    Begin { session: u64, config: Arc<Config> },
    Audio { session: u64, samples: Vec<f32> },
    End { session: u64 },
    Cancel { session: u64 },
    Reload(Arc<Config>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// 本次会话实际使用的引擎；`fallback` 为真表示云端不可用已降级。
    Engine {
        session: u64,
        engine: Engine,
        fallback: bool,
    },
    Partial {
        session: u64,
        text: String,
    },
    Final {
        session: u64,
        result: Result<String, String>,
    },
}

#[derive(Clone)]
pub struct Handle {
    tx: Sender<Request>,
}

impl Handle {
    pub fn begin(&self, session: u64, config: Arc<Config>) {
        let _ = self.tx.send(Request::Begin { session, config });
    }

    pub fn audio(&self, session: u64, samples: Vec<f32>) {
        let _ = self.tx.send(Request::Audio { session, samples });
    }

    pub fn end(&self, session: u64) {
        let _ = self.tx.send(Request::End { session });
    }

    pub fn cancel(&self, session: u64) {
        let _ = self.tx.send(Request::Cancel { session });
    }

    pub fn reload(&self, config: Arc<Config>) {
        let _ = self.tx.send(Request::Reload(config));
    }
}

/// 启动工作线程。模型在线程内加载，daemon 的 socket 不必等待。
pub fn spawn<F>(config: Arc<Config>, emit: F) -> Result<Handle>
where
    F: Fn(Event) + Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("yuda-asr".to_owned())
        .spawn(move || {
            let mut worker = Worker {
                emit,
                offline: OfflineModel::default(),
                pending: None,
            };
            worker.offline.ensure(&config);
            worker.run(&rx);
        })
        .context("启动识别线程失败")?;
    Ok(Handle { tx })
}

/// 离线模型及其加载参数；参数变化时重新加载。
#[derive(Default)]
struct OfflineModel {
    engine: Option<OfflineEngine>,
    error: Option<String>,
    key: Option<String>,
}

impl OfflineModel {
    fn ensure(&mut self, config: &Config) {
        if config.engine == EngineMode::Cloud {
            // 纯云端模式不占用离线模型的内存。
            self.engine = None;
            self.key = None;
            return;
        }
        let key = format!("{:?}|{:?}", config.offline, config.language);
        if self.key.as_deref() == Some(key.as_str()) {
            return;
        }
        self.key = Some(key);
        let started = std::time::Instant::now();
        match OfflineEngine::load(config) {
            Ok(engine) => {
                tracing::info!(
                    model = engine.name(),
                    ms = started.elapsed().as_millis(),
                    "离线模型已加载"
                );
                self.engine = Some(engine);
                self.error = None;
            }
            Err(error) => {
                tracing::error!(error = %format!("{error:#}"), "离线模型加载失败");
                self.engine = None;
                self.error = Some(format!("{error:#}"));
            }
        }
    }

    fn unavailable(&self) -> String {
        self.error
            .clone()
            .unwrap_or_else(|| "离线模型未加载".to_owned())
    }
}

struct Worker<F> {
    emit: F,
    offline: OfflineModel,
    /// 会话进行中收到、需要会话结束后处理的请求。
    pending: Option<Request>,
}

/// 会话中途收到的控制请求。
enum Interrupt {
    End,
    Cancel,
    Disconnected,
}

impl<F: Fn(Event)> Worker<F> {
    fn run(&mut self, rx: &Receiver<Request>) {
        loop {
            let request = match self.pending.take() {
                Some(request) => request,
                None => match rx.recv() {
                    Ok(request) => request,
                    Err(_) => return,
                },
            };
            match request {
                Request::Begin { session, config } => self.run_session(session, &config, rx),
                Request::Reload(config) => self.offline.ensure(&config),
                // 已取消或过期会话的残余消息。
                Request::Audio { .. } | Request::End { .. } | Request::Cancel { .. } => {}
            }
        }
    }

    fn fail(&self, session: u64, message: impl Into<String>) {
        (self.emit)(Event::Final {
            session,
            result: Err(message.into()),
        });
    }

    /// 等待本会话的 End/Cancel，丢弃中间的音频。
    fn wait_for_end(&mut self, session: u64, rx: &Receiver<Request>) {
        // End、Cancel、断开或新会话都会带回结束信号。
        while drain_audio(session, rx, &mut self.pending).1.is_none() {}
    }

    fn run_session(&mut self, session: u64, config: &Config, rx: &Receiver<Request>) {
        #[cfg(feature = "cloud")]
        let mut cloud: Option<crate::cloud_asr::CloudSession> = None;
        let mut fallback = false;

        let wants_cloud = config.engine != EngineMode::Offline && config.cloud.is_configured();
        if config.engine == EngineMode::Cloud && !config.cloud.is_configured() {
            self.fail(session, "云端引擎缺少火山引擎 App Key / Access Key");
            self.wait_for_end(session, rx);
            return;
        }
        if wants_cloud {
            #[cfg(feature = "cloud")]
            match crate::cloud_asr::CloudSession::connect(&cloud_config(config)) {
                Ok(connected) => cloud = Some(connected),
                Err(error) => {
                    tracing::warn!(%error, "云端识别不可用");
                    if config.engine == EngineMode::Cloud {
                        self.fail(session, format!("云端识别不可用：{error}"));
                        self.wait_for_end(session, rx);
                        return;
                    }
                    fallback = true;
                }
            }
            #[cfg(not(feature = "cloud"))]
            {
                if config.engine == EngineMode::Cloud {
                    self.fail(session, "此构建不含云端引擎");
                    self.wait_for_end(session, rx);
                    return;
                }
                fallback = true;
            }
        }

        #[cfg(feature = "cloud")]
        if let Some(cloud) = cloud {
            self.cloud_session(session, config, cloud, rx);
            return;
        }

        self.run_offline(session, &[], fallback, rx);
    }

    /// 离线流式会话；`buffered` 是切换前已经采到的音频（云端中途失败时）。
    fn run_offline(
        &mut self,
        session: u64,
        buffered: &[f32],
        fallback: bool,
        rx: &Receiver<Request>,
    ) {
        let Some(engine) = self.offline.engine.as_ref() else {
            let message = self.offline.unavailable();
            self.fail(session, message);
            self.wait_for_end(session, rx);
            return;
        };
        (self.emit)(Event::Engine {
            session,
            engine: Engine::Offline,
            fallback,
        });
        let mut stream = engine.session();
        stream.accept(buffered);
        let interrupt = loop {
            // 先吃掉队列里已到的全部音频，再解码一次预览：解码慢于音频到达时不会越积越多。
            let (samples, stop) = drain_audio(session, rx, &mut self.pending);
            stream.accept(&samples);
            if let Some(interrupt) = stop {
                break interrupt;
            }
            if let Some(text) = stream.partial() {
                (self.emit)(Event::Partial { session, text });
            }
        };
        match interrupt {
            Interrupt::End => {
                let seconds = stream.duration().as_secs_f32();
                let started = std::time::Instant::now();
                let text = stream.finish();
                tracing::info!(
                    audio_s = format!("{seconds:.1}"),
                    finish_ms = started.elapsed().as_millis(),
                    "离线识别完成"
                );
                (self.emit)(Event::Final {
                    session,
                    result: Ok(text),
                });
            }
            Interrupt::Cancel | Interrupt::Disconnected => {}
        }
    }

    #[cfg(feature = "cloud")]
    fn cloud_session(
        &mut self,
        session: u64,
        config: &Config,
        mut cloud: crate::cloud_asr::CloudSession,
        rx: &Receiver<Request>,
    ) {
        (self.emit)(Event::Engine {
            session,
            engine: Engine::Cloud,
            fallback: false,
        });
        // 缓存整段音频，云端中途失败时可离线重识别。
        let mut audio: Vec<f32> = Vec::new();
        let mut last_text = String::new();
        let mut broken: Option<String> = None;
        let interrupt = loop {
            let (samples, stop) = drain_audio(session, rx, &mut self.pending);
            if !samples.is_empty() {
                audio.extend_from_slice(&samples);
                if broken.is_none() {
                    let pcm: Vec<i16> = samples
                        .iter()
                        .map(|s| (s.clamp(-1.0, 1.0) * 32_767.0) as i16)
                        .collect();
                    if let Err(error) = cloud.send_audio(&pcm) {
                        tracing::warn!(%error, "云端识别中断");
                        if config.engine == EngineMode::Auto && self.offline.engine.is_some() {
                            // 立刻切到离线流式，用户继续看到实时预览。
                            self.run_offline(session, &audio, true, rx);
                            return;
                        }
                        broken = Some(error.to_string());
                    }
                }
            }
            if let Some(interrupt) = stop {
                break interrupt;
            }
            if let Some(text) = cloud.latest_text() {
                if text != last_text {
                    last_text = text.clone();
                    (self.emit)(Event::Partial { session, text });
                }
            }
        };
        if !matches!(interrupt, Interrupt::End) {
            return;
        }
        let result = match broken {
            Some(error) => Err(error),
            None => cloud
                .finish(Duration::from_secs(4))
                .map_err(|error| error.to_string()),
        };
        let result = match result {
            Ok(text) => Ok(text),
            Err(error) if config.engine == EngineMode::Auto => match self.offline.engine.as_ref() {
                Some(engine) => {
                    tracing::warn!(%error, "云端失败，改用离线重识别整段音频");
                    (self.emit)(Event::Engine {
                        session,
                        engine: Engine::Offline,
                        fallback: true,
                    });
                    Ok(engine.transcribe_samples(&audio))
                }
                None => Err(format!("云端识别失败：{error}")),
            },
            Err(error) => Err(format!("云端识别失败：{error}")),
        };
        (self.emit)(Event::Final { session, result });
    }
}

/// 取本会话已经到达的全部音频：最多等 100 ms 等来第一条消息，之后把队列里现成的消息一次取完。
/// 途中遇到 End / Cancel / 断开时一并返回该信号；信号之前的音频照常返回，不能丢。
/// 自由函数而非方法：调用方此时还借用着离线模型。
fn drain_audio(
    session: u64,
    rx: &Receiver<Request>,
    pending: &mut Option<Request>,
) -> (Vec<f32>, Option<Interrupt>) {
    let mut audio = Vec::new();
    let mut first = true;
    loop {
        let request = if first {
            first = false;
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(request) => request,
                Err(RecvTimeoutError::Timeout) => return (audio, None),
                Err(RecvTimeoutError::Disconnected) => {
                    return (audio, Some(Interrupt::Disconnected))
                }
            }
        } else {
            match rx.try_recv() {
                Ok(request) => request,
                Err(TryRecvError::Empty) => return (audio, None),
                Err(TryRecvError::Disconnected) => return (audio, Some(Interrupt::Disconnected)),
            }
        };
        match request {
            Request::Audio {
                session: id,
                samples,
            } if id == session => audio.extend_from_slice(&samples),
            Request::End { session: id } if id == session => return (audio, Some(Interrupt::End)),
            Request::Cancel { session: id } if id == session => {
                return (audio, Some(Interrupt::Cancel))
            }
            begin @ Request::Begin { .. } => {
                // 理论上不会发生（daemon 在会话结束前拒绝新会话）；保险起见放弃当前会话。
                *pending = Some(begin);
                return (audio, Some(Interrupt::Cancel));
            }
            Request::Reload(config) => *pending = Some(Request::Reload(config)),
            // 已取消或过期会话的残余消息。
            _ => {}
        }
    }
}

#[cfg(feature = "cloud")]
pub fn cloud_config(config: &Config) -> crate::cloud_asr::CloudAsrConfig {
    crate::cloud_asr::CloudAsrConfig {
        endpoint: config.cloud.endpoint.clone(),
        app_key: config.cloud.app_key.clone(),
        access_key: config.cloud.access_key.clone(),
        resource_id: config.cloud.resource_id.clone(),
        connect_timeout: Duration::from_millis(config.cloud.connect_timeout_ms),
        language: config.language.as_str().to_owned(),
        hotwords: config.text.hotwords.clone(),
        uid: "yuda".to_owned(),
    }
}
