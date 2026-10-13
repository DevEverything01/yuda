//! Volcano Engine (火山引擎) Doubao bigmodel streaming ASR client (大模型流式语音识别, v3 `sauc`).
//!
//! Protocol sources (verified 2026-10):
//! - Official API doc: <https://www.volcengine.com/docs/6561/1354869> (updated 2026-08-06).
//! - Official demos attached to that page (`sauc_python.zip`, `sauc_go.zip`).
//! - PyPI `doubao-speech` 0.1.0, `doubao_speech/_ws_client.py`.
//!
//! Wire format: every WebSocket binary frame starts with a 4-byte header
//! `[version<<4 | header_words, type<<4 | flags, serialization<<4 | compression, 0x00]`.
//! All integers are big-endian. Client frames carry `sequence (i32)`, `payload size (u32)`,
//! then a gzip payload. The full client request uses sequence 1, audio packets use 2, 3, ...
//! and the last audio packet sets flags `0b0011` with a negated sequence.
//!
//! Threading: `tungstenite` cannot split one socket into read and write halves, so a single
//! I/O thread owns the WebSocket. The session queues encoded frames over a channel. The thread
//! polls reads with a short socket timeout and publishes results through a `Mutex` + `Condvar`.

use std::{
    collections::HashSet,
    fmt,
    io::{self, Read, Write},
    net::{SocketAddr, TcpStream, ToSocketAddrs},
    sync::{mpsc, Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError},
    thread,
    time::{Duration, Instant},
};

use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use serde::Deserialize;
use serde_json::{json, Value};
use tungstenite::{
    client::IntoClientRequest,
    handshake::client::Request,
    http::{HeaderName, HeaderValue},
    stream::MaybeTlsStream,
    Connector, HandshakeError, Message, WebSocket,
};

/// Only sample rate the API accepts ("默认为 16000，目前只支持16000").
pub const SAMPLE_RATE: u32 = 16_000;
/// Samples per audio-only request: 100 ms of 16 kHz mono audio.
pub const PACKET_SAMPLES: usize = 1_600;
/// Bytes per audio-only request: 100 ms of s16le mono audio.
pub const PACKET_BYTES: usize = PACKET_SAMPLES * 2;

const PROTOCOL_VERSION: u8 = 0b0001;
const HEADER_SIZE_WORDS: u8 = 0b0001;

const MSG_FULL_CLIENT_REQUEST: u8 = 0b0001;
const MSG_AUDIO_ONLY_REQUEST: u8 = 0b0010;
const MSG_FULL_SERVER_RESPONSE: u8 = 0b1001;
const MSG_SERVER_ERROR: u8 = 0b1111;

const FLAG_POSITIVE_SEQUENCE: u8 = 0b0001;
const FLAG_LAST_PACKET: u8 = 0b0010;
const FLAG_NEGATIVE_SEQUENCE: u8 = FLAG_POSITIVE_SEQUENCE | FLAG_LAST_PACKET;
const FLAG_EVENT: u8 = 0b0100;

const SERIALIZATION_JSON: u8 = 0b0001;
const COMPRESSION_GZIP: u8 = 0b0001;

/// Protocol constant, not a model choice: the doc says `request.model_name` "目前只有bigmodel".
/// The model generation (1.0 / 2.0) is selected by `X-Api-Resource-Id`, which comes from config.
const REQUEST_MODEL_NAME: &str = "bigmodel";
/// `audio.language` is only honoured by the `bigmodel_nostream` endpoint.
const NOSTREAM_PATH_SUFFIX: &str = "/bigmodel_nostream";
/// Bidirectional endpoints accept about 100 tokens of direct hotwords.
const STREAMING_HOTWORDS_SOFT_LIMIT: usize = 100;

const IO_POLL_INTERVAL: Duration = Duration::from_millis(10);
const IO_WRITE_TIMEOUT: Duration = Duration::from_secs(2);
const CLOSE_GRACE: Duration = Duration::from_millis(300);
const MAX_ERROR_BODY_CHARS: usize = 200;

/// Settings for one cloud recognition session. The daemon builds this from `config.toml`.
#[derive(Clone)]
pub struct CloudAsrConfig {
    /// WebSocket endpoint, `wss://openspeech.bytedance.com/api/v3/sauc/<bigmodel|bigmodel_async|bigmodel_nostream>`.
    pub endpoint: String,
    /// `X-Api-App-Key` (APP ID, old console). Leave empty to use a new-console API key:
    /// `access_key` is then sent as `X-Api-Key`.
    pub app_key: String,
    /// `X-Api-Access-Key` (Access Token, old console) or the new-console API key.
    pub access_key: String,
    /// `X-Api-Resource-Id`, for example `volc.bigasr.sauc.duration`.
    pub resource_id: String,
    /// Upper bound for DNS + TCP + TLS + WebSocket handshake.
    pub connect_timeout: Duration,
    /// `zh` | `en` | `yue` | `ja` | `ko`. Sent as `audio.language` only to `bigmodel_nostream`.
    pub language: String,
    /// User dictionary, sent as `request.corpus.context` hotwords.
    pub hotwords: Vec<String>,
    /// `user.uid`, used by the server only to filter logs. Empty means omitted.
    pub uid: String,
}

impl fmt::Debug for CloudAsrConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CloudAsrConfig")
            .field("endpoint", &self.endpoint)
            .field("app_key", &self.app_key)
            .field("access_key", &redact(&self.access_key))
            .field("resource_id", &self.resource_id)
            .field("connect_timeout", &self.connect_timeout)
            .field("language", &self.language)
            .field("hotwords", &self.hotwords.len())
            .field("uid", &self.uid)
            .finish()
    }
}

fn redact(secret: &str) -> &'static str {
    if secret.is_empty() {
        "<empty>"
    } else {
        "<redacted>"
    }
}

/// Which wait ran out of time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeoutStage {
    /// DNS + TCP + TLS + WebSocket handshake.
    Connect,
    /// Waiting for the final result after the last audio packet.
    FinalResult,
}

impl fmt::Display for TimeoutStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Connect => "建立连接",
            Self::FinalResult => "等待最终结果",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CloudAsrError {
    #[error("云端识别配置无效：{0}")]
    InvalidConfig(String),
    #[error("无法连接云端识别服务：{0}")]
    Connect(String),
    #[error("云端识别服务拒绝连接（HTTP {status}），请检查 App ID、Access Token 与 Resource ID：{message}")]
    Rejected { status: u16, message: String },
    #[error("云端识别{stage}超时（{} ms）", .timeout.as_millis())]
    Timeout {
        stage: TimeoutStage,
        timeout: Duration,
    },
    #[error("云端识别服务返回错误 {code}（{}）：{message}", server_code_hint(*.code))]
    Server { code: u32, message: String },
    #[error("云端识别协议错误：{0}")]
    Protocol(String),
    #[error("云端识别连接中断：{0}")]
    Io(String),
}

/// Chinese meaning of the documented error codes.
pub fn server_code_hint(code: u32) -> &'static str {
    match code {
        20_000_000 => "成功",
        45_000_001 => "请求参数无效",
        45_000_002 => "空音频",
        45_000_081 => "等包超时",
        45_000_151 => "音频格式不正确",
        55_000_031 => "服务器繁忙",
        55_000_000..=55_099_999 => "服务内部处理错误",
        _ => "未知错误",
    }
}

/// One decoded server frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerMessage {
    /// Full server response (message type `0b1001`).
    Response(ServerResponse),
    /// Error message from server (message type `0b1111`).
    Error { code: u32, message: String },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServerResponse {
    /// Present when flags bit 0 is set. Negative on the final response.
    pub sequence: Option<i32>,
    /// Flags bit 1: this is the result for the last audio packet.
    pub is_last: bool,
    /// `result.text`: the whole transcript so far (`result_type = "full"`).
    pub text: Option<String>,
    /// `result.utterances` (requires `show_utterances = true`).
    pub utterances: Vec<Utterance>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Utterance {
    pub text: String,
    /// `true` once the server will no longer change this sentence.
    pub definite: bool,
    pub start_time_ms: Option<i64>,
    pub end_time_ms: Option<i64>,
}

/// Maps the app language to `audio.language`. Returns `None` (auto) unless the endpoint is
/// `bigmodel_nostream`, the only mode that supports the field. `zh` also stays auto: with no
/// language the model covers Mandarin, English and the listed dialects, which is what
/// mixed Chinese-English dictation needs (the doc treats `zh-CN` as the same default model).
pub fn language_code(language: &str, endpoint: &str) -> Option<&'static str> {
    if !endpoint_is_nostream(endpoint) {
        return None;
    }
    match language.trim() {
        "en" => Some("en-US"),
        "yue" => Some("yue-CN"),
        "ja" => Some("ja-JP"),
        "ko" => Some("ko-KR"),
        _ => None,
    }
}

fn endpoint_is_nostream(endpoint: &str) -> bool {
    let path = endpoint.split(['?', '#']).next().unwrap_or_default();
    path.trim_end_matches('/').ends_with(NOSTREAM_PATH_SUFFIX)
}

fn normalized_hotwords(hotwords: &[String]) -> Vec<&str> {
    let mut seen = HashSet::new();
    hotwords
        .iter()
        .map(|word| word.trim())
        .filter(|word| !word.is_empty() && seen.insert(*word))
        .collect()
}

/// Encodes the user dictionary as the JSON string the API expects in `request.corpus.context`:
/// `{"hotwords":[{"word":"..."}]}`. Returns `None` when there are no usable words.
pub fn hotwords_context(hotwords: &[String]) -> Option<String> {
    let words = normalized_hotwords(hotwords);
    if words.is_empty() {
        return None;
    }
    let entries: Vec<Value> = words.iter().map(|word| json!({ "word": word })).collect();
    Some(json!({ "hotwords": entries }).to_string())
}

/// Builds the full client request JSON.
pub fn build_request_payload(config: &CloudAsrConfig) -> Value {
    let mut audio = json!({
        "format": "pcm",
        "codec": "raw",
        "rate": SAMPLE_RATE,
        "bits": 16,
        "channel": 1,
    });
    if let (Some(code), Some(map)) = (
        language_code(&config.language, &config.endpoint),
        audio.as_object_mut(),
    ) {
        map.insert("language".to_owned(), Value::from(code));
    }

    let mut request = json!({
        "model_name": REQUEST_MODEL_NAME,
        "enable_itn": true,
        "enable_punc": true,
        "enable_ddc": false,
        "show_utterances": true,
        "result_type": "full",
    });
    if let (Some(context), Some(map)) =
        (hotwords_context(&config.hotwords), request.as_object_mut())
    {
        let chars: usize = normalized_hotwords(&config.hotwords)
            .iter()
            .map(|word| word.chars().count())
            .sum();
        if !endpoint_is_nostream(&config.endpoint) && chars > STREAMING_HOTWORDS_SOFT_LIMIT {
            tracing::warn!(
                chars,
                limit = STREAMING_HOTWORDS_SOFT_LIMIT,
                "热词总长度超过双向流式接口约 100 tokens 的上限，服务端可能截断"
            );
        }
        map.insert("corpus".to_owned(), json!({ "context": context }));
    }

    let mut payload = json!({ "audio": audio, "request": request });
    let uid = config.uid.trim();
    if let (false, Some(map)) = (uid.is_empty(), payload.as_object_mut()) {
        map.insert("user".to_owned(), json!({ "uid": uid }));
    }
    payload
}

fn header(message_type: u8, flags: u8, serialization: u8, compression: u8) -> [u8; 4] {
    [
        (PROTOCOL_VERSION << 4) | HEADER_SIZE_WORDS,
        (message_type << 4) | flags,
        (serialization << 4) | compression,
        0x00,
    ]
}

fn gzip(data: &[u8]) -> Result<Vec<u8>, CloudAsrError> {
    let mut encoder = GzEncoder::new(Vec::with_capacity(data.len() / 2 + 32), Compression::fast());
    encoder
        .write_all(data)
        .and_then(|()| encoder.finish())
        .map_err(|error| CloudAsrError::Protocol(format!("gzip 压缩失败：{error}")))
}

fn gunzip(data: &[u8]) -> Result<Vec<u8>, CloudAsrError> {
    let mut decoded = Vec::with_capacity(data.len() * 2);
    GzDecoder::new(data)
        .read_to_end(&mut decoded)
        .map_err(|error| CloudAsrError::Protocol(format!("gzip 解压失败：{error}")))?;
    Ok(decoded)
}

fn encode_frame(header: [u8; 4], sequence: i32, payload: &[u8]) -> Result<Vec<u8>, CloudAsrError> {
    let compressed = gzip(payload)?;
    let size = u32::try_from(compressed.len())
        .map_err(|_| CloudAsrError::Protocol("负载超过 4 GiB".to_owned()))?;
    let mut frame = Vec::with_capacity(12 + compressed.len());
    frame.extend_from_slice(&header);
    frame.extend_from_slice(&sequence.to_be_bytes());
    frame.extend_from_slice(&size.to_be_bytes());
    frame.extend_from_slice(&compressed);
    Ok(frame)
}

/// Encodes a full client request: header `11 11 11 00` (JSON, gzip, positive sequence),
/// `sequence`, payload size, gzip(JSON). The official demos use `sequence = 1`.
pub fn encode_full_client_request(
    payload: &Value,
    sequence: i32,
) -> Result<Vec<u8>, CloudAsrError> {
    let json = serde_json::to_vec(payload)
        .map_err(|error| CloudAsrError::Protocol(format!("请求 JSON 序列化失败：{error}")))?;
    encode_frame(
        header(
            MSG_FULL_CLIENT_REQUEST,
            FLAG_POSITIVE_SEQUENCE,
            SERIALIZATION_JSON,
            COMPRESSION_GZIP,
        ),
        sequence,
        &json,
    )
}

/// Encodes an audio-only request carrying s16le PCM bytes.
///
/// `sequence` is the positive packet number. When `is_last` is set, the frame uses flags
/// `0b0011` and puts `-sequence` on the wire, which tells the server the stream has ended.
///
/// Byte 2 is `0x11` (serialization JSON, gzip) as in both official demos and `doubao-speech`.
/// The doc's message-flow example shows `0x01` (no serialization) for audio; the vendor demos
/// are the path known to work against the live service, so this follows them.
pub fn encode_audio_packet(
    pcm_s16le: &[u8],
    sequence: i32,
    is_last: bool,
) -> Result<Vec<u8>, CloudAsrError> {
    if sequence <= 0 {
        return Err(CloudAsrError::Protocol(format!(
            "音频包序号必须为正数，实际为 {sequence}"
        )));
    }
    let (flags, wire_sequence) = if is_last {
        (FLAG_NEGATIVE_SEQUENCE, -sequence)
    } else {
        (FLAG_POSITIVE_SEQUENCE, sequence)
    };
    encode_frame(
        header(
            MSG_AUDIO_ONLY_REQUEST,
            flags,
            SERIALIZATION_JSON,
            COMPRESSION_GZIP,
        ),
        wire_sequence,
        pcm_s16le,
    )
}

struct FrameCursor<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> FrameCursor<'a> {
    fn take(&mut self, len: usize, field: &str) -> Result<&'a [u8], CloudAsrError> {
        let end = self.offset.checked_add(len);
        let bytes = end
            .and_then(|end| self.data.get(self.offset..end))
            .ok_or_else(|| {
                CloudAsrError::Protocol(format!(
                    "服务端帧被截断：读取 {field} 需要 {len} 字节，剩余 {} 字节",
                    self.remaining().len()
                ))
            })?;
        self.offset += len;
        Ok(bytes)
    }

    fn word(&mut self, field: &str) -> Result<[u8; 4], CloudAsrError> {
        let bytes = self.take(4, field)?;
        let mut word = [0_u8; 4];
        word.copy_from_slice(bytes);
        Ok(word)
    }

    fn remaining(&self) -> &'a [u8] {
        self.data.get(self.offset..).unwrap_or_default()
    }
}

#[derive(Deserialize, Default)]
struct ResponsePayload {
    #[serde(default)]
    result: Option<ResultField>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ResultField {
    One(ResultBody),
    Many(Vec<ResultBody>),
}

#[derive(Deserialize, Default)]
struct ResultBody {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    utterances: Option<Vec<RawUtterance>>,
}

#[derive(Deserialize)]
struct RawUtterance {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    definite: Option<bool>,
    #[serde(default)]
    start_time: Option<i64>,
    #[serde(default)]
    end_time: Option<i64>,
}

impl From<RawUtterance> for Utterance {
    fn from(raw: RawUtterance) -> Self {
        Self {
            text: raw.text.unwrap_or_default(),
            definite: raw.definite.unwrap_or(false),
            start_time_ms: raw.start_time,
            end_time_ms: raw.end_time,
        }
    }
}

fn decompress(payload: &[u8], compression: u8) -> Result<Vec<u8>, CloudAsrError> {
    if compression == COMPRESSION_GZIP && !payload.is_empty() {
        gunzip(payload)
    } else {
        Ok(payload.to_vec())
    }
}

fn parse_result(payload: &[u8]) -> Result<(Option<String>, Vec<Utterance>), CloudAsrError> {
    if payload.iter().all(u8::is_ascii_whitespace) {
        return Ok((None, Vec::new()));
    }
    let parsed: ResponsePayload = serde_json::from_slice(payload)
        .map_err(|error| CloudAsrError::Protocol(format!("识别结果不是有效 JSON：{error}")))?;
    let bodies = match parsed.result {
        None => Vec::new(),
        Some(ResultField::One(body)) => vec![body],
        Some(ResultField::Many(bodies)) => bodies,
    };
    let mut text: Option<String> = None;
    let mut utterances = Vec::new();
    for body in bodies {
        if let Some(part) = body.text {
            text.get_or_insert_with(String::new).push_str(&part);
        }
        utterances.extend(
            body.utterances
                .unwrap_or_default()
                .into_iter()
                .map(Utterance::from),
        );
    }
    Ok((text, utterances))
}

fn error_message_text(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw).trim().to_owned();
    if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&text) {
        for key in ["error", "message", "msg", "reason"] {
            if let Some(Value::String(message)) = map.get(key) {
                return message.clone();
            }
        }
    }
    text
}

/// Decodes one server frame. Malformed or truncated input returns `CloudAsrError::Protocol`
/// and never panics.
///
/// Field order follows the official demos: optional sequence (flags bit 0), optional event
/// (flags bit 2), then the type-specific body. A full server response carries
/// `payload size + payload`; an error frame carries `code (u32) + size (u32) + message`.
pub fn decode_server_message(frame: &[u8]) -> Result<ServerMessage, CloudAsrError> {
    let head = frame
        .get(..4)
        .ok_or_else(|| CloudAsrError::Protocol(format!("服务端帧过短：{} 字节", frame.len())))?;
    let header_len = usize::from(head[0] & 0x0f) * 4;
    if header_len < 4 {
        return Err(CloudAsrError::Protocol(format!(
            "服务端帧头长度无效：{header_len}"
        )));
    }
    let message_type = head[1] >> 4;
    let flags = head[1] & 0x0f;
    let serialization = head[2] >> 4;
    let compression = head[2] & 0x0f;
    let body = frame.get(header_len..).ok_or_else(|| {
        CloudAsrError::Protocol(format!(
            "服务端帧头声明 {header_len} 字节，实际只有 {} 字节",
            frame.len()
        ))
    })?;
    let mut cursor = FrameCursor {
        data: body,
        offset: 0,
    };

    let sequence = if flags & FLAG_POSITIVE_SEQUENCE != 0 {
        Some(i32::from_be_bytes(cursor.word("sequence")?))
    } else {
        None
    };
    if flags & FLAG_EVENT != 0 {
        cursor.word("event")?;
    }

    match message_type {
        MSG_FULL_SERVER_RESPONSE => {
            let size = u32::from_be_bytes(cursor.word("payload size")?);
            let size = usize::try_from(size)
                .map_err(|_| CloudAsrError::Protocol("负载长度超出平台范围".to_owned()))?;
            let payload = decompress(cursor.take(size, "payload")?, compression)?;
            let (text, utterances) = if serialization == SERIALIZATION_JSON {
                parse_result(&payload)?
            } else {
                (None, Vec::new())
            };
            Ok(ServerMessage::Response(ServerResponse {
                sequence,
                is_last: flags & FLAG_LAST_PACKET != 0,
                text,
                utterances,
            }))
        }
        MSG_SERVER_ERROR => {
            let code = u32::from_be_bytes(cursor.word("error code")?);
            // Be lenient with the size field: the error code is what matters.
            let declared = cursor
                .word("error size")
                .map(u32::from_be_bytes)
                .ok()
                .and_then(|size| usize::try_from(size).ok());
            let rest = cursor.remaining();
            let raw = declared.and_then(|size| rest.get(..size)).unwrap_or(rest);
            let raw = if compression == COMPRESSION_GZIP {
                gunzip(raw).unwrap_or_else(|_| raw.to_vec())
            } else {
                raw.to_vec()
            };
            Ok(ServerMessage::Error {
                code,
                message: error_message_text(&raw),
            })
        }
        other => Err(CloudAsrError::Protocol(format!(
            "未知的服务端消息类型 0b{other:04b}"
        ))),
    }
}

fn validate_config(config: &CloudAsrConfig) -> Result<(), CloudAsrError> {
    let endpoint = config.endpoint.trim().to_ascii_lowercase();
    if !(endpoint.starts_with("wss://") || endpoint.starts_with("ws://")) {
        return Err(CloudAsrError::InvalidConfig(
            "endpoint 必须以 wss:// 或 ws:// 开头".to_owned(),
        ));
    }
    if config.access_key.trim().is_empty() {
        return Err(CloudAsrError::InvalidConfig(
            "未配置火山引擎 Access Token（或新版控制台 API Key）".to_owned(),
        ));
    }
    if config.resource_id.trim().is_empty() {
        return Err(CloudAsrError::InvalidConfig(
            "未配置 X-Api-Resource-Id（例如 volc.bigasr.sauc.duration）".to_owned(),
        ));
    }
    if config.connect_timeout.is_zero() {
        return Err(CloudAsrError::InvalidConfig(
            "connect_timeout 必须大于 0".to_owned(),
        ));
    }
    Ok(())
}

/// Auth headers. Old console: `X-Api-App-Key` + `X-Api-Access-Key`. New console (empty
/// `app_key`): `X-Api-Key`. Both: `X-Api-Resource-Id`, `X-Api-Connect-Id` (doc example) and
/// `X-Api-Request-Id` (official demos).
fn handshake_headers(
    config: &CloudAsrConfig,
    connect_id: &str,
    request_id: &str,
) -> Vec<(&'static str, String, bool)> {
    let mut headers = Vec::with_capacity(5);
    let app_key = config.app_key.trim();
    let access_key = config.access_key.trim().to_owned();
    if app_key.is_empty() {
        headers.push(("x-api-key", access_key, true));
    } else {
        headers.push(("x-api-app-key", app_key.to_owned(), false));
        headers.push(("x-api-access-key", access_key, true));
    }
    headers.push((
        "x-api-resource-id",
        config.resource_id.trim().to_owned(),
        false,
    ));
    headers.push(("x-api-connect-id", connect_id.to_owned(), false));
    headers.push(("x-api-request-id", request_id.to_owned(), false));
    headers
}

fn build_handshake_request(
    config: &CloudAsrConfig,
    connect_id: &str,
    request_id: &str,
) -> Result<Request, CloudAsrError> {
    let mut request = config
        .endpoint
        .trim()
        .into_client_request()
        .map_err(|error| CloudAsrError::InvalidConfig(format!("endpoint 无效：{error}")))?;
    for (name, value, sensitive) in handshake_headers(config, connect_id, request_id) {
        let mut value = HeaderValue::from_str(&value)
            .map_err(|_| CloudAsrError::InvalidConfig(format!("{name} 含有非法字符")))?;
        value.set_sensitive(sensitive);
        request
            .headers_mut()
            .insert(HeaderName::from_static(name), value);
    }
    Ok(request)
}

fn tls_config() -> Result<Arc<rustls::ClientConfig>, CloudAsrError> {
    static TLS_CONFIG: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();
    if let Some(config) = TLS_CONFIG.get() {
        return Ok(Arc::clone(config));
    }
    // Pin the ring provider explicitly instead of relying on a process-wide default, which
    // panics when no provider (or more than one) is compiled in.
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|error| CloudAsrError::Connect(format!("初始化 TLS 失败：{error}")))?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(Arc::clone(TLS_CONFIG.get_or_init(|| Arc::new(config))))
}

#[derive(Default)]
struct SessionState {
    latest_text: Option<String>,
    final_text: Option<String>,
    error: Option<CloudAsrError>,
    closed: bool,
}

#[derive(Default)]
struct Shared {
    state: Mutex<SessionState>,
    changed: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, SessionState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn update(&self, apply: impl FnOnce(&mut SessionState)) {
        let mut state = self.lock();
        apply(&mut state);
        drop(state);
        self.changed.notify_all();
    }

    /// Marks the I/O thread as gone. Records `error` unless a final result already arrived.
    fn close(&self, error: Option<CloudAsrError>) {
        self.update(|state| {
            state.closed = true;
            if state.final_text.is_none() && state.error.is_none() {
                state.error = Some(error.unwrap_or_else(|| {
                    CloudAsrError::Io("服务端在返回最终结果前关闭了连接".to_owned())
                }));
            }
        });
    }
}

type Socket = WebSocket<MaybeTlsStream<TcpStream>>;

struct WorkerArgs {
    request: Request,
    full_request: Vec<u8>,
    deadline: Instant,
    timeout: Duration,
    ready: mpsc::SyncSender<Result<Option<String>, CloudAsrError>>,
    commands: mpsc::Receiver<Vec<u8>>,
    shared: Arc<Shared>,
}

/// One streaming recognition session over a single WebSocket connection.
///
/// Dropping the session (or calling [`CloudSession::finish`]) tells the I/O thread to send a
/// close frame and exit; nothing blocks on that.
pub struct CloudSession {
    commands: mpsc::Sender<Vec<u8>>,
    shared: Arc<Shared>,
    pending: Vec<u8>,
    next_sequence: i32,
    log_id: Option<String>,
    connect_id: String,
}

impl fmt::Debug for CloudSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CloudSession")
            .field("connect_id", &self.connect_id)
            .field("log_id", &self.log_id)
            .field("next_sequence", &self.next_sequence)
            .field("pending_bytes", &self.pending.len())
            .finish()
    }
}

impl CloudSession {
    /// TCP + TLS + WebSocket handshake bounded by `connect_timeout`, then sends the full client
    /// request. The work runs on a background I/O thread, so a stuck DNS lookup cannot hold
    /// this call past the timeout.
    pub fn connect(config: &CloudAsrConfig) -> Result<Self, CloudAsrError> {
        validate_config(config)?;
        let connect_id = uuid::Uuid::new_v4().to_string();
        let request_id = uuid::Uuid::new_v4().to_string();
        let request = build_handshake_request(config, &connect_id, &request_id)?;
        let full_request = encode_full_client_request(&build_request_payload(config), 1)?;

        let timeout = config.connect_timeout;
        let deadline = Instant::now() + timeout;
        let shared = Arc::new(Shared::default());
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let (command_tx, command_rx) = mpsc::channel();
        let args = WorkerArgs {
            request,
            full_request,
            deadline,
            timeout,
            ready: ready_tx,
            commands: command_rx,
            shared: Arc::clone(&shared),
        };
        thread::Builder::new()
            .name("yuda-cloud-asr".to_owned())
            .spawn(move || run_worker(args))
            .map_err(|error| CloudAsrError::Connect(format!("无法启动云端识别线程：{error}")))?;

        let log_id = match ready_rx.recv_timeout(timeout) {
            Ok(Ok(log_id)) => log_id,
            Ok(Err(error)) => return Err(error),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                return Err(CloudAsrError::Timeout {
                    stage: TimeoutStage::Connect,
                    timeout,
                })
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(CloudAsrError::Connect("云端识别线程意外退出".to_owned()))
            }
        };
        tracing::info!(
            connect_id = %connect_id,
            log_id = log_id.as_deref().unwrap_or("-"),
            "已连接火山引擎云端识别"
        );
        Ok(Self {
            commands: command_tx,
            shared,
            pending: Vec::with_capacity(PACKET_BYTES * 2),
            next_sequence: 2,
            log_id,
            connect_id,
        })
    }

    /// Queues 16 kHz mono s16le PCM. Full 100 ms packets (3200 bytes) are sent at once; the
    /// remainder (1..=3200 bytes) is held back so the last packet sent by `finish` is never
    /// empty. Never blocks on the network.
    pub fn send_audio(&mut self, samples: &[i16]) -> Result<(), CloudAsrError> {
        if let Some(error) = self.shared.lock().error.clone() {
            return Err(error);
        }
        self.pending.reserve(samples.len() * 2);
        for sample in samples {
            self.pending.extend_from_slice(&sample.to_le_bytes());
        }

        let sendable = (self.pending.len().saturating_sub(1) / PACKET_BYTES) * PACKET_BYTES;
        let mut queued_bytes = 0;
        let mut result = Ok(());
        for chunk in self.pending[..sendable].chunks_exact(PACKET_BYTES) {
            let sequence = self.next_sequence;
            let step = sequence
                .checked_add(1)
                .ok_or_else(|| CloudAsrError::Protocol("音频包序号溢出".to_owned()))
                .and_then(|next| {
                    let frame = encode_audio_packet(chunk, sequence, false)?;
                    self.commands
                        .send(frame)
                        .map_err(|_| terminal_error(&self.shared))?;
                    Ok(next)
                });
            match step {
                Ok(next) => {
                    self.next_sequence = next;
                    queued_bytes += PACKET_BYTES;
                }
                Err(error) => {
                    result = Err(error);
                    break;
                }
            }
        }
        self.pending.drain(..queued_bytes);
        result
    }

    /// Latest transcript received so far (non-blocking). `None` before the first non-empty result.
    pub fn latest_text(&self) -> Option<String> {
        self.shared.lock().latest_text.clone()
    }

    /// `X-Tt-Logid` returned by the handshake, for support tickets.
    pub fn log_id(&self) -> Option<&str> {
        self.log_id.as_deref()
    }

    /// `X-Api-Connect-Id` sent with the handshake.
    pub fn connect_id(&self) -> &str {
        &self.connect_id
    }

    /// Sends the held-back audio as the last packet (negative sequence) and waits up to
    /// `timeout` for the final result.
    pub fn finish(mut self, timeout: Duration) -> Result<String, CloudAsrError> {
        let deadline = Instant::now() + timeout;
        {
            let state = self.shared.lock();
            if let Some(text) = state.final_text.clone() {
                return Ok(text);
            }
            if let Some(error) = state.error.clone() {
                return Err(error);
            }
        }

        let last = std::mem::take(&mut self.pending);
        let frame = encode_audio_packet(&last, self.next_sequence, true)?;
        if self.commands.send(frame).is_err() {
            tracing::debug!("云端识别线程已退出，直接读取会话状态");
        }

        let mut state = self.shared.lock();
        loop {
            if let Some(text) = state.final_text.clone() {
                return Ok(text);
            }
            if let Some(error) = state.error.clone() {
                return Err(error);
            }
            if state.closed {
                return Err(CloudAsrError::Io(
                    "服务端在返回最终结果前关闭了连接".to_owned(),
                ));
            }
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .filter(|remaining| !remaining.is_zero());
            let Some(remaining) = remaining else {
                tracing::warn!(
                    connect_id = %self.connect_id,
                    log_id = self.log_id.as_deref().unwrap_or("-"),
                    "等待云端最终识别结果超时"
                );
                return Err(CloudAsrError::Timeout {
                    stage: TimeoutStage::FinalResult,
                    timeout,
                });
            };
            state = match self.shared.changed.wait_timeout(state, remaining) {
                Ok((guard, _)) => guard,
                Err(poisoned) => poisoned.into_inner().0,
            };
        }
    }
}

fn terminal_error(shared: &Shared) -> CloudAsrError {
    let state = shared.lock();
    if let Some(error) = state.error.clone() {
        return error;
    }
    if state.final_text.is_some() {
        return CloudAsrError::Protocol("云端识别会话已结束，不能继续发送音频".to_owned());
    }
    CloudAsrError::Io("云端识别连接已关闭".to_owned())
}

fn remaining_until(deadline: Instant) -> Option<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
}

fn connect_timeout_error(timeout: Duration) -> CloudAsrError {
    CloudAsrError::Timeout {
        stage: TimeoutStage::Connect,
        timeout,
    }
}

fn is_timeout_kind(kind: io::ErrorKind) -> bool {
    matches!(kind, io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut)
}

fn is_would_block(error: &tungstenite::Error) -> bool {
    matches!(error, tungstenite::Error::Io(io) if is_timeout_kind(io.kind()))
}

fn run_worker(args: WorkerArgs) {
    let WorkerArgs {
        request,
        full_request,
        deadline,
        timeout,
        ready,
        commands,
        shared,
    } = args;

    let (mut socket, control, log_id) = match open_socket(request, deadline, timeout) {
        Ok(opened) => opened,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    if let Err(error) = socket.send(Message::Binary(full_request.into())) {
        let error = if is_would_block(&error) {
            connect_timeout_error(timeout)
        } else {
            CloudAsrError::Connect(format!("发送识别参数失败：{error}"))
        };
        let _ = ready.send(Err(error));
        return;
    }
    let polling = control
        .set_read_timeout(Some(IO_POLL_INTERVAL))
        .and_then(|()| control.set_write_timeout(Some(IO_WRITE_TIMEOUT)));
    if let Err(error) = polling {
        let _ = ready.send(Err(CloudAsrError::Connect(format!(
            "设置套接字超时失败：{error}"
        ))));
        return;
    }
    if ready.send(Ok(log_id)).is_err() {
        // `connect` already gave up (timeout); do not leave the server waiting.
        close_gracefully(&mut socket);
        return;
    }
    io_loop(&mut socket, &commands, &shared);
}

fn open_socket(
    request: Request,
    deadline: Instant,
    timeout: Duration,
) -> Result<(Socket, TcpStream, Option<String>), CloudAsrError> {
    let uri = request.uri();
    let secure = uri.scheme_str() == Some("wss");
    let host = uri
        .host()
        .ok_or_else(|| CloudAsrError::InvalidConfig("endpoint 缺少主机名".to_owned()))?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_owned();
    let port = uri.port_u16().unwrap_or(if secure { 443 } else { 80 });

    let addrs: Vec<SocketAddr> = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|error| CloudAsrError::Connect(format!("无法解析 {host}：{error}")))?
        .collect();
    let tcp = connect_tcp(&addrs, &host, deadline, timeout)?;
    let remaining = remaining_until(deadline).ok_or_else(|| connect_timeout_error(timeout))?;
    tcp.set_nodelay(true)
        .and_then(|()| tcp.set_read_timeout(Some(remaining)))
        .and_then(|()| tcp.set_write_timeout(Some(remaining)))
        .map_err(|error| CloudAsrError::Connect(format!("设置套接字参数失败：{error}")))?;
    // A second handle to the same socket, used to change timeouts after tungstenite owns it.
    let control = tcp
        .try_clone()
        .map_err(|error| CloudAsrError::Connect(format!("复制套接字句柄失败：{error}")))?;

    let connector = Connector::Rustls(tls_config()?);
    let mut attempt = tungstenite::client_tls_with_config(request, tcp, None, Some(connector));
    loop {
        match attempt {
            Ok((socket, response)) => {
                let log_id = response
                    .headers()
                    .get("x-tt-logid")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned);
                return Ok((socket, control, log_id));
            }
            Err(HandshakeError::Interrupted(mid)) => {
                let remaining =
                    remaining_until(deadline).ok_or_else(|| connect_timeout_error(timeout))?;
                // Ignore failures here: the next blocking read reports them.
                let _ = control.set_read_timeout(Some(remaining));
                let _ = control.set_write_timeout(Some(remaining));
                attempt = mid.handshake();
            }
            Err(HandshakeError::Failure(error)) => {
                return Err(map_handshake_error(error, timeout));
            }
        }
    }
}

fn connect_tcp(
    addrs: &[SocketAddr],
    host: &str,
    deadline: Instant,
    timeout: Duration,
) -> Result<TcpStream, CloudAsrError> {
    let mut last_error = None;
    for addr in addrs {
        let remaining = remaining_until(deadline).ok_or_else(|| connect_timeout_error(timeout))?;
        match TcpStream::connect_timeout(addr, remaining) {
            Ok(stream) => return Ok(stream),
            Err(error) => last_error = Some(error),
        }
    }
    match last_error {
        Some(error) if is_timeout_kind(error.kind()) => Err(connect_timeout_error(timeout)),
        Some(error) => Err(CloudAsrError::Connect(format!("连接 {host} 失败：{error}"))),
        None => Err(CloudAsrError::Connect(format!("{host} 没有可用地址"))),
    }
}

fn map_handshake_error(error: tungstenite::Error, timeout: Duration) -> CloudAsrError {
    match error {
        tungstenite::Error::Http(response) => {
            let status = response.status().as_u16();
            let log_id = response
                .headers()
                .get("x-tt-logid")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("-");
            // The live service answers e.g. 401 `{"error":"load grant: requested grant not found in SaaS storage"}`.
            let body = error_message_text(response.body().as_deref().unwrap_or_default());
            let message: String = body.chars().take(MAX_ERROR_BODY_CHARS).collect();
            tracing::warn!(status, log_id, message = %message, "云端识别握手被拒绝");
            CloudAsrError::Rejected { status, message }
        }
        tungstenite::Error::Io(io) if is_timeout_kind(io.kind()) => connect_timeout_error(timeout),
        tungstenite::Error::Io(io) => CloudAsrError::Connect(format!("网络错误：{io}")),
        tungstenite::Error::Tls(tls) => CloudAsrError::Connect(format!("TLS 握手失败：{tls}")),
        tungstenite::Error::Url(url) => {
            CloudAsrError::InvalidConfig(format!("endpoint 无效：{url}"))
        }
        other => CloudAsrError::Connect(format!("WebSocket 握手失败：{other}")),
    }
}

/// Owns the socket: writes queued frames, then polls one read (bounded by
/// `IO_POLL_INTERVAL`). Exits after the final result, a server error, a transport failure,
/// or when the session is dropped.
fn io_loop(socket: &mut Socket, commands: &mpsc::Receiver<Vec<u8>>, shared: &Shared) {
    loop {
        let mut session_dropped = false;
        loop {
            match commands.try_recv() {
                Ok(frame) => {
                    if let Err(error) = socket.write(Message::Binary(frame.into())) {
                        if !is_would_block(&error) {
                            shared.close(Some(CloudAsrError::Io(format!("发送音频失败：{error}"))));
                            return;
                        }
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    session_dropped = true;
                    break;
                }
            }
        }
        if let Err(error) = socket.flush() {
            if !is_would_block(&error) {
                shared.close(Some(CloudAsrError::Io(format!("发送音频失败：{error}"))));
                return;
            }
        }
        if session_dropped {
            close_gracefully(socket);
            shared.close(Some(CloudAsrError::Io("云端识别会话已取消".to_owned())));
            return;
        }

        match socket.read() {
            Ok(Message::Binary(data)) => match decode_server_message(&data) {
                Ok(ServerMessage::Response(response)) => {
                    if apply_response(shared, response) {
                        close_gracefully(socket);
                        shared.close(None);
                        return;
                    }
                }
                Ok(ServerMessage::Error { code, message }) => {
                    tracing::warn!(code, message = %message, "云端识别服务返回错误帧");
                    shared.close(Some(CloudAsrError::Server { code, message }));
                    close_gracefully(socket);
                    return;
                }
                Err(error) => {
                    tracing::warn!(%error, "无法解析云端识别服务端帧");
                    shared.close(Some(error));
                    close_gracefully(socket);
                    return;
                }
            },
            Ok(Message::Close(frame)) => {
                let reason = frame
                    .map(|frame| format!("{}（{}）", frame.reason, u16::from(frame.code)))
                    .unwrap_or_default();
                shared.close(Some(CloudAsrError::Io(format!(
                    "服务端在返回最终结果前关闭了连接{reason}"
                ))));
                close_gracefully(socket);
                return;
            }
            Ok(Message::Text(text)) => {
                tracing::warn!(len = text.len(), "忽略云端识别服务端文本帧");
            }
            Ok(_) => {}
            Err(error) if is_would_block(&error) => {}
            Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
                shared.close(None);
                return;
            }
            Err(error) => {
                shared.close(Some(CloudAsrError::Io(error.to_string())));
                return;
            }
        }
    }
}

/// Applies one response to the shared state. Returns `true` on the final response.
fn apply_response(shared: &Shared, response: ServerResponse) -> bool {
    let definite = response.utterances.iter().filter(|u| u.definite).count();
    tracing::debug!(
        sequence = response.sequence,
        is_last = response.is_last,
        text_chars = response.text.as_ref().map(|text| text.chars().count()),
        utterances = response.utterances.len(),
        definite,
        "收到云端识别结果"
    );
    let is_last = response.is_last;
    shared.update(|state| {
        if let Some(text) = response.text.clone() {
            if !text.is_empty() || state.latest_text.is_some() {
                state.latest_text = Some(text);
            }
        }
        if is_last {
            state.final_text = Some(
                response
                    .text
                    .or_else(|| state.latest_text.clone())
                    .unwrap_or_default(),
            );
        }
    });
    is_last
}

fn close_gracefully(socket: &mut Socket) {
    let _ = socket.close(None);
    let deadline = Instant::now() + CLOSE_GRACE;
    while Instant::now() < deadline {
        match socket.read() {
            Ok(_) => {}
            Err(error) if is_would_block(&error) => {}
            Err(_) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(endpoint: &str) -> CloudAsrConfig {
        CloudAsrConfig {
            endpoint: endpoint.to_owned(),
            app_key: "app".to_owned(),
            access_key: "secret-token".to_owned(),
            resource_id: "volc.bigasr.sauc.duration".to_owned(),
            connect_timeout: Duration::from_secs(2),
            language: "zh".to_owned(),
            hotwords: Vec::new(),
            uid: "user-1".to_owned(),
        }
    }

    const ASYNC: &str = "wss://openspeech.bytedance.com/api/v3/sauc/bigmodel_async";
    const NOSTREAM: &str = "wss://openspeech.bytedance.com/api/v3/sauc/bigmodel_nostream";

    fn split_client_frame(frame: &[u8]) -> ([u8; 4], i32, Vec<u8>) {
        let mut head = [0_u8; 4];
        head.copy_from_slice(&frame[..4]);
        let sequence = i32::from_be_bytes(frame[4..8].try_into().unwrap());
        let size = u32::from_be_bytes(frame[8..12].try_into().unwrap()) as usize;
        assert_eq!(frame.len(), 12 + size);
        (head, sequence, gunzip(&frame[12..]).unwrap())
    }

    fn server_frame(flags: u8, sequence: Option<i32>, json: &str) -> Vec<u8> {
        let payload = gzip(json.as_bytes()).unwrap();
        let mut frame = vec![0x11, 0x90 | flags, 0x11, 0x00];
        if let Some(sequence) = sequence {
            frame.extend_from_slice(&sequence.to_be_bytes());
        }
        frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        frame.extend_from_slice(&payload);
        frame
    }

    #[test]
    fn full_client_request_has_documented_header_and_gzip_json() {
        let payload = build_request_payload(&config(ASYNC));
        let frame = encode_full_client_request(&payload, 1).unwrap();
        let (head, sequence, body) = split_client_frame(&frame);
        assert_eq!(head, [0x11, 0x11, 0x11, 0x00]);
        assert_eq!(sequence, 1);
        let decoded: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(decoded, payload);
    }

    #[test]
    fn audio_packets_use_positive_then_negative_sequence() {
        let pcm = vec![7_u8; PACKET_BYTES];
        let (head, sequence, body) =
            split_client_frame(&encode_audio_packet(&pcm, 2, false).unwrap());
        assert_eq!(head, [0x11, 0x21, 0x11, 0x00]);
        assert_eq!(sequence, 2);
        assert_eq!(body, pcm);

        let (head, sequence, body) =
            split_client_frame(&encode_audio_packet(&pcm[..10], 7, true).unwrap());
        assert_eq!(head, [0x11, 0x23, 0x11, 0x00]);
        assert_eq!(sequence, -7);
        assert_eq!(body, &pcm[..10]);

        let (_, _, body) = split_client_frame(&encode_audio_packet(&[], 3, true).unwrap());
        assert!(body.is_empty());
        assert!(matches!(
            encode_audio_packet(&pcm, 0, false),
            Err(CloudAsrError::Protocol(_))
        ));
    }

    #[test]
    fn request_payload_matches_documented_fields() {
        let mut cfg = config(ASYNC);
        cfg.hotwords = vec![
            " 语打 ".to_owned(),
            "Omarchy".to_owned(),
            "语打".to_owned(),
            String::new(),
        ];
        let payload = build_request_payload(&cfg);
        assert_eq!(payload["user"]["uid"], "user-1");
        assert_eq!(payload["audio"]["format"], "pcm");
        assert_eq!(payload["audio"]["codec"], "raw");
        assert_eq!(payload["audio"]["rate"], 16_000);
        assert_eq!(payload["audio"]["bits"], 16);
        assert_eq!(payload["audio"]["channel"], 1);
        assert!(payload["audio"].get("language").is_none());
        assert_eq!(payload["request"]["model_name"], "bigmodel");
        assert_eq!(payload["request"]["enable_itn"], true);
        assert_eq!(payload["request"]["enable_punc"], true);
        assert_eq!(payload["request"]["enable_ddc"], false);
        assert_eq!(payload["request"]["show_utterances"], true);
        assert_eq!(payload["request"]["result_type"], "full");
        let context = payload["request"]["corpus"]["context"].as_str().unwrap();
        let context: Value = serde_json::from_str(context).unwrap();
        assert_eq!(
            context,
            json!({"hotwords": [{"word": "语打"}, {"word": "Omarchy"}]})
        );
    }

    #[test]
    fn request_payload_omits_empty_optional_parts() {
        let mut cfg = config(ASYNC);
        cfg.uid = "  ".to_owned();
        cfg.hotwords = vec!["   ".to_owned()];
        let payload = build_request_payload(&cfg);
        assert!(payload.get("user").is_none());
        assert!(payload["request"].get("corpus").is_none());
    }

    #[test]
    fn language_is_only_sent_to_nostream_endpoint() {
        assert_eq!(language_code("en", ASYNC), None);
        assert_eq!(language_code("en", "wss://h/api/v3/sauc/bigmodel"), None);
        assert_eq!(language_code("en", NOSTREAM), Some("en-US"));
        assert_eq!(
            language_code("ja", &format!("{NOSTREAM}/?x=1")),
            Some("ja-JP")
        );
        assert_eq!(language_code("ko", NOSTREAM), Some("ko-KR"));
        assert_eq!(language_code("yue", NOSTREAM), Some("yue-CN"));
        assert_eq!(language_code("zh", NOSTREAM), None);
        assert_eq!(language_code("fr", NOSTREAM), None);

        let mut cfg = config(NOSTREAM);
        cfg.language = "en".to_owned();
        assert_eq!(build_request_payload(&cfg)["audio"]["language"], "en-US");
    }

    #[test]
    fn decodes_partial_and_final_responses() {
        let partial = server_frame(
            0b0001,
            Some(2),
            r#"{"audio_info":{"duration":200},"result":{"text":"你好","utterances":[{"text":"你好","definite":false,"start_time":0,"end_time":200}]}}"#,
        );
        let ServerMessage::Response(response) = decode_server_message(&partial).unwrap() else {
            panic!("expected response");
        };
        assert_eq!(response.sequence, Some(2));
        assert!(!response.is_last);
        assert_eq!(response.text.as_deref(), Some("你好"));
        assert_eq!(
            response.utterances,
            vec![Utterance {
                text: "你好".to_owned(),
                definite: false,
                start_time_ms: Some(0),
                end_time_ms: Some(200)
            }]
        );

        let last = server_frame(0b0011, Some(-3), r#"{"result":{"text":"你好。"}}"#);
        let ServerMessage::Response(response) = decode_server_message(&last).unwrap() else {
            panic!("expected response");
        };
        assert_eq!(response.sequence, Some(-3));
        assert!(response.is_last);
        assert_eq!(response.text.as_deref(), Some("你好。"));

        let last_without_sequence =
            server_frame(0b0010, None, r#"{"result":[{"text":"a"},{"text":"b"}]}"#);
        let ServerMessage::Response(response) =
            decode_server_message(&last_without_sequence).unwrap()
        else {
            panic!("expected response");
        };
        assert_eq!(response.sequence, None);
        assert!(response.is_last);
        assert_eq!(response.text.as_deref(), Some("ab"));
    }

    #[test]
    fn decodes_event_flag_and_empty_payload() {
        let mut frame = vec![0x11, 0x95, 0x11, 0x00];
        frame.extend_from_slice(&4_i32.to_be_bytes());
        frame.extend_from_slice(&451_i32.to_be_bytes());
        frame.extend_from_slice(&0_u32.to_be_bytes());
        let ServerMessage::Response(response) = decode_server_message(&frame).unwrap() else {
            panic!("expected response");
        };
        assert_eq!(response.sequence, Some(4));
        assert_eq!(response.text, None);
    }

    #[test]
    fn decodes_error_frames() {
        let json = br#"{"error":"requested resource not granted"}"#;
        let mut frame = vec![0x11, 0xF0, 0x10, 0x00];
        frame.extend_from_slice(&45_000_001_u32.to_be_bytes());
        frame.extend_from_slice(&(json.len() as u32).to_be_bytes());
        frame.extend_from_slice(json);
        assert_eq!(
            decode_server_message(&frame).unwrap(),
            ServerMessage::Error {
                code: 45_000_001,
                message: "requested resource not granted".to_owned()
            }
        );

        let text = gzip("server busy".as_bytes()).unwrap();
        let mut frame = vec![0x11, 0xF0, 0x01, 0x00];
        frame.extend_from_slice(&55_000_031_u32.to_be_bytes());
        frame.extend_from_slice(&(text.len() as u32).to_be_bytes());
        frame.extend_from_slice(&text);
        assert_eq!(
            decode_server_message(&frame).unwrap(),
            ServerMessage::Error {
                code: 55_000_031,
                message: "server busy".to_owned()
            }
        );
        let error = CloudAsrError::Server {
            code: 55_000_031,
            message: "server busy".to_owned(),
        };
        assert!(error.to_string().contains("服务器繁忙"));
    }

    #[test]
    fn truncated_or_unknown_frames_are_protocol_errors() {
        let frame = server_frame(0b0001, Some(2), r#"{"result":{"text":"你好"}}"#);
        for len in 0..frame.len() {
            assert!(
                matches!(
                    decode_server_message(&frame[..len]),
                    Err(CloudAsrError::Protocol(_))
                ),
                "prefix of {len} bytes should fail"
            );
        }
        assert!(matches!(
            decode_server_message(&[0x10, 0x90, 0x11, 0x00]),
            Err(CloudAsrError::Protocol(_))
        ));
        assert!(matches!(
            decode_server_message(&[0x11, 0x30, 0x11, 0x00, 0, 0, 0, 0]),
            Err(CloudAsrError::Protocol(_))
        ));
        let mut not_gzip = vec![0x11, 0x90, 0x11, 0x00];
        not_gzip.extend_from_slice(&3_u32.to_be_bytes());
        not_gzip.extend_from_slice(b"abc");
        assert!(matches!(
            decode_server_message(&not_gzip),
            Err(CloudAsrError::Protocol(_))
        ));
    }

    #[test]
    fn handshake_headers_follow_console_generation() {
        let old = handshake_headers(&config(ASYNC), "cid", "rid");
        let names: Vec<_> = old.iter().map(|(name, _, _)| *name).collect();
        assert_eq!(
            names,
            [
                "x-api-app-key",
                "x-api-access-key",
                "x-api-resource-id",
                "x-api-connect-id",
                "x-api-request-id"
            ]
        );

        let mut new_console = config(ASYNC);
        new_console.app_key.clear();
        let new = handshake_headers(&new_console, "cid", "rid");
        assert_eq!(new[0], ("x-api-key", "secret-token".to_owned(), true));
        assert!(new.iter().all(|(name, _, _)| *name != "x-api-app-key"));

        let request = build_handshake_request(&config(ASYNC), "cid", "rid").unwrap();
        assert_eq!(request.headers()["x-api-access-key"], "secret-token");
        assert!(request.headers()["x-api-access-key"].is_sensitive());
    }

    #[test]
    fn validates_config_before_connecting() {
        let mut cfg = config("https://example.com");
        assert!(matches!(
            validate_config(&cfg),
            Err(CloudAsrError::InvalidConfig(_))
        ));
        cfg.endpoint = ASYNC.to_owned();
        cfg.access_key.clear();
        assert!(matches!(
            validate_config(&cfg),
            Err(CloudAsrError::InvalidConfig(_))
        ));
        cfg.access_key = "k".to_owned();
        cfg.resource_id.clear();
        assert!(matches!(
            validate_config(&cfg),
            Err(CloudAsrError::InvalidConfig(_))
        ));
        cfg.resource_id = "r".to_owned();
        cfg.connect_timeout = Duration::ZERO;
        assert!(matches!(
            CloudSession::connect(&cfg),
            Err(CloudAsrError::InvalidConfig(_))
        ));
    }

    #[test]
    fn debug_output_redacts_access_key() {
        let rendered = format!("{:?}", config(ASYNC));
        assert!(!rendered.contains("secret-token"));
        assert!(rendered.contains("<redacted>"));
    }

    #[test]
    fn tls_config_builds_with_ring_and_webpki_roots() {
        let config = tls_config().unwrap();
        assert!(Arc::ptr_eq(&config, &tls_config().unwrap()));
    }
}
