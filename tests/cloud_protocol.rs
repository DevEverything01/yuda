//! Protocol tests for the Volcano Engine bigmodel streaming ASR client against a local
//! plain `ws://` mock server. No network access.
//!
//! The mock decodes client frames by hand (flate2 + raw byte checks) instead of reusing the
//! crate's own codec, so a framing bug cannot pass on both sides.

// tungstenite's server `Callback` signature fixes the `Err` type to `http::Response`.
#![allow(clippy::result_large_err)]

use std::{
    collections::HashMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::mpsc,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use serde_json::{json, Value};
use tungstenite::{
    accept_hdr,
    handshake::server::{ErrorResponse, Request, Response},
    http::{HeaderValue, StatusCode},
    Message, WebSocket,
};
use yuda::cloud_asr::{CloudAsrConfig, CloudAsrError, CloudSession, TimeoutStage, PACKET_BYTES};

const LOG_ID: &str = "mock-logid-20261014";
const FINAL_TEXT: &str = "最终结果。";

#[derive(Debug, Clone)]
struct ClientFrame {
    message_type: u8,
    flags: u8,
    serialization: u8,
    sequence: i32,
    payload: Vec<u8>,
}

struct Handshake {
    path: String,
    headers: HashMap<String, String>,
}

type ServerHandle = JoinHandle<(Handshake, Vec<ClientFrame>)>;

fn gzip(data: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}

fn parse_client_frame(data: &[u8]) -> ClientFrame {
    assert!(
        data.len() >= 12,
        "客户端帧过短：{} 字节，期望 ≥ 12",
        data.len()
    );
    assert_eq!(data[0], 0x11, "期望协议版本 1、头长 1 个字（0x11）");
    assert_eq!(data[3], 0x00, "期望保留字节为 0x00");
    assert_eq!(data[2] & 0x0f, 0b0001, "期望客户端负载为 gzip 压缩");
    let flags = data[1] & 0x0f;
    assert_ne!(flags & 0b0001, 0, "期望客户端帧携带序号（flags bit0 = 1）");
    let sequence = i32::from_be_bytes(data[4..8].try_into().unwrap());
    let size = u32::from_be_bytes(data[8..12].try_into().unwrap()) as usize;
    assert_eq!(data.len(), 12 + size, "期望 payload size 字段与帧长度一致");
    let mut payload = Vec::new();
    GzDecoder::new(&data[12..])
        .read_to_end(&mut payload)
        .unwrap();
    ClientFrame {
        message_type: data[1] >> 4,
        flags,
        serialization: data[2] >> 4,
        sequence,
        payload,
    }
}

fn response_frame(flags: u8, sequence: i32, body: &Value) -> Vec<u8> {
    let payload = gzip(body.to_string().as_bytes());
    let mut frame = vec![0x11, 0x90 | flags, 0x11, 0x00];
    frame.extend_from_slice(&sequence.to_be_bytes());
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(&payload);
    frame
}

/// Error frame as in the doc example: type 0b1111, flags 0, JSON, no compression.
fn error_frame(code: u32, message: &str) -> Vec<u8> {
    let body = json!({ "error": message }).to_string();
    let mut frame = vec![0x11, 0xF0, 0x10, 0x00];
    frame.extend_from_slice(&code.to_be_bytes());
    frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
    frame.extend_from_slice(body.as_bytes());
    frame
}

fn read_frame(ws: &mut WebSocket<TcpStream>, frames: &mut Vec<ClientFrame>) -> Option<ClientFrame> {
    loop {
        match ws.read() {
            Ok(Message::Binary(data)) => {
                let frame = parse_client_frame(&data);
                frames.push(frame.clone());
                return Some(frame);
            }
            Ok(Message::Close(_)) | Err(_) => return None,
            Ok(_) => {}
        }
    }
}

fn drain_until_closed(ws: &mut WebSocket<TcpStream>, frames: &mut Vec<ClientFrame>) {
    while read_frame(ws, frames).is_some() {}
}

fn spawn_server<F>(path: &str, script: F) -> (String, ServerHandle)
where
    F: FnOnce(&mut WebSocket<TcpStream>, &mut Vec<ClientFrame>) + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut handshake = None;
        let mut ws = accept_hdr(stream, |request: &Request, mut response: Response| {
            handshake = Some(Handshake {
                path: request.uri().path().to_owned(),
                headers: request
                    .headers()
                    .iter()
                    .map(|(name, value)| {
                        (name.as_str().to_owned(), value.to_str().unwrap().to_owned())
                    })
                    .collect(),
            });
            response
                .headers_mut()
                .insert("X-Tt-Logid", HeaderValue::from_static(LOG_ID));
            Ok(response)
        })
        .unwrap();
        let mut frames = Vec::new();
        script(&mut ws, &mut frames);
        (handshake.unwrap(), frames)
    });
    (format!("ws://{addr}{path}"), handle)
}

/// Acks the full client request with an empty result, answers each audio packet with
/// "第N包", and answers the last packet with the final text.
fn streaming_script(ws: &mut WebSocket<TcpStream>, frames: &mut Vec<ClientFrame>) {
    let Some(full) = read_frame(ws, frames) else {
        return;
    };
    let _ = ws.send(Message::Binary(
        response_frame(0b0001, full.sequence, &json!({ "result": { "text": "" } })).into(),
    ));
    while let Some(frame) = read_frame(ws, frames) {
        if frame.flags == 0b0011 {
            let body = json!({
                "audio_info": { "duration": 500 },
                "result": {
                    "text": FINAL_TEXT,
                    "utterances": [{ "text": FINAL_TEXT, "definite": true, "start_time": 0, "end_time": 500 }]
                }
            });
            let _ = ws.send(Message::Binary(
                response_frame(0b0011, frame.sequence, &body).into(),
            ));
            break;
        }
        let text = format!("第{}包", frame.sequence - 1);
        let _ = ws.send(Message::Binary(
            response_frame(
                0b0001,
                frame.sequence,
                &json!({ "result": { "text": text } }),
            )
            .into(),
        ));
    }
    drain_until_closed(ws, frames);
}

fn config(endpoint: String) -> CloudAsrConfig {
    CloudAsrConfig {
        endpoint,
        app_key: "test-app-key".to_owned(),
        access_key: "test-access-key".to_owned(),
        resource_id: "volc.bigasr.sauc.duration".to_owned(),
        connect_timeout: Duration::from_secs(2),
        language: "zh".to_owned(),
        hotwords: Vec::new(),
        uid: "tester".to_owned(),
    }
}

fn pcm(samples: usize) -> Vec<i16> {
    (0..samples)
        .map(|index| ((index * 37) % 65_536) as u16 as i16)
        .collect()
}

#[test]
fn handshake_headers_and_full_client_request() {
    let (endpoint, server) = spawn_server("/api/v3/sauc/bigmodel_nostream", streaming_script);
    let mut cfg = config(endpoint);
    cfg.language = "en".to_owned();
    cfg.hotwords = vec!["语打".to_owned(), "Hyprland".to_owned()];

    let session = CloudSession::connect(&cfg).unwrap();
    assert_eq!(session.log_id(), Some(LOG_ID));
    let connect_id = session.connect_id().to_owned();
    assert_eq!(session.finish(Duration::from_secs(2)).unwrap(), FINAL_TEXT);

    let (handshake, frames) = server.join().unwrap();
    assert_eq!(handshake.path, "/api/v3/sauc/bigmodel_nostream");
    let header = |name: &str| handshake.headers.get(name).cloned().unwrap_or_default();
    assert_eq!(header("x-api-app-key"), "test-app-key");
    assert_eq!(header("x-api-access-key"), "test-access-key");
    assert_eq!(header("x-api-resource-id"), "volc.bigasr.sauc.duration");
    assert_eq!(header("x-api-connect-id"), connect_id);
    assert_eq!(connect_id.len(), 36, "期望 X-Api-Connect-Id 为 UUID");
    assert_eq!(
        header("x-api-request-id").len(),
        36,
        "期望 X-Api-Request-Id 为 UUID"
    );

    let full = &frames[0];
    assert_eq!(full.message_type, 0b0001, "期望首帧为 full client request");
    assert_eq!(full.flags, 0b0001);
    assert_eq!(full.serialization, 0b0001, "JSON");
    assert_eq!(full.sequence, 1);
    let request: Value = serde_json::from_slice(&full.payload).unwrap();
    assert_eq!(request["user"]["uid"], "tester");
    assert_eq!(
        request["audio"],
        json!({ "format": "pcm", "codec": "raw", "rate": 16000, "bits": 16, "channel": 1, "language": "en-US" })
    );
    assert_eq!(request["request"]["model_name"], "bigmodel");
    assert_eq!(request["request"]["enable_itn"], true);
    assert_eq!(request["request"]["enable_punc"], true);
    assert_eq!(request["request"]["show_utterances"], true);
    assert_eq!(request["request"]["result_type"], "full");
    let context: Value =
        serde_json::from_str(request["request"]["corpus"]["context"].as_str().unwrap()).unwrap();
    assert_eq!(
        context,
        json!({ "hotwords": [{ "word": "语打" }, { "word": "Hyprland" }] })
    );

    // Empty input: finish still sends an empty last packet with a negative sequence.
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[1].message_type, 0b0010);
    assert_eq!(frames[1].flags, 0b0011);
    assert_eq!(frames[1].sequence, -2);
    assert!(frames[1].payload.is_empty());
}

#[test]
fn audio_is_sent_as_100ms_packets_with_increasing_sequence() {
    let (endpoint, server) = spawn_server("/api/v3/sauc/bigmodel_async", streaming_script);
    let mut session = CloudSession::connect(&config(endpoint)).unwrap();
    let samples = pcm(8_000); // 500 ms
    for chunk in samples.chunks(700) {
        session.send_audio(chunk).unwrap();
    }
    assert_eq!(session.finish(Duration::from_secs(2)).unwrap(), FINAL_TEXT);

    let (_, frames) = server.join().unwrap();
    let request: Value = serde_json::from_slice(&frames[0].payload).unwrap();
    assert!(
        request["audio"].get("language").is_none(),
        "双向流式接口不支持 audio.language，期望省略"
    );
    assert!(request["request"].get("corpus").is_none());

    let audio = &frames[1..];
    assert_eq!(audio.len(), 5, "期望 4 个整包 + 1 个末包");
    for (index, frame) in audio.iter().enumerate() {
        assert_eq!(
            frame.message_type, 0b0010,
            "期望消息类型为 audio-only request"
        );
        assert_eq!(
            frame.payload.len(),
            PACKET_BYTES,
            "期望每包 100 ms（3200 字节）"
        );
        if index + 1 < audio.len() {
            assert_eq!(frame.flags, 0b0001);
            assert_eq!(frame.sequence, index as i32 + 2);
        } else {
            assert_eq!(frame.flags, 0b0011, "期望末包 flags = 0b0011");
            assert_eq!(frame.sequence, -(index as i32 + 2), "期望末包序号为负");
        }
    }
    let sent: Vec<u8> = audio
        .iter()
        .flat_map(|frame| frame.payload.clone())
        .collect();
    let expected: Vec<u8> = samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect();
    assert_eq!(sent, expected, "期望按顺序发送 s16le PCM");
}

#[test]
fn partial_results_are_visible_before_final() {
    let (endpoint, server) = spawn_server("/api/v3/sauc/bigmodel_async", streaming_script);
    let mut session = CloudSession::connect(&config(endpoint)).unwrap();
    thread::sleep(Duration::from_millis(100));
    assert_eq!(session.latest_text(), None, "空结果的应答不应产生识别文本");

    session.send_audio(&pcm(4_800)).unwrap(); // 2 packets sent, 1 held back
    let deadline = Instant::now() + Duration::from_secs(2);
    while session.latest_text().as_deref() != Some("第2包") {
        assert!(
            Instant::now() < deadline,
            "期望收到中间结果“第2包”，实际：{:?}",
            session.latest_text()
        );
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(session.finish(Duration::from_secs(2)).unwrap(), FINAL_TEXT);
    server.join().unwrap();
}

#[test]
fn server_error_frame_maps_to_server_error() {
    let (endpoint, server) = spawn_server("/api/v3/sauc/bigmodel_async", |ws, frames| {
        if read_frame(ws, frames).is_some() {
            let _ = ws.send(Message::Binary(
                error_frame(45_000_001, "invalid resource id").into(),
            ));
        }
        drain_until_closed(ws, frames);
    });
    let mut session = CloudSession::connect(&config(endpoint)).unwrap();
    let expected = CloudAsrError::Server {
        code: 45_000_001,
        message: "invalid resource id".to_owned(),
    };

    let deadline = Instant::now() + Duration::from_secs(2);
    let error = loop {
        match session.send_audio(&pcm(1_600)) {
            Err(error) => break error,
            Ok(()) => {
                assert!(
                    Instant::now() < deadline,
                    "期望 send_audio 在服务端错误帧后返回错误"
                );
                thread::sleep(Duration::from_millis(5));
            }
        }
    };
    assert_eq!(error, expected);
    assert!(error.to_string().contains("请求参数无效"));
    assert_eq!(session.finish(Duration::from_secs(1)), Err(expected));
    server.join().unwrap();
}

#[test]
fn connect_times_out_when_handshake_never_completes() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let silent = thread::spawn(move || {
        // Accept the TCP connection but never answer the HTTP upgrade, until released
        // (hold time is longer than the ceiling below).
        let (stream, _) = listener.accept().unwrap();
        let _ = release_rx.recv_timeout(Duration::from_secs(4));
        drop(stream);
    });

    let mut cfg = config(format!("ws://{addr}/api/v3/sauc/bigmodel_async"));
    cfg.connect_timeout = Duration::from_millis(300);
    let started = Instant::now();
    let result = CloudSession::connect(&cfg);
    let elapsed = started.elapsed();

    assert!(
        matches!(
            result,
            Err(CloudAsrError::Timeout {
                stage: TimeoutStage::Connect,
                ..
            })
        ),
        "期望超时错误，实际：{result:?}"
    );
    assert!(
        elapsed >= Duration::from_millis(250),
        "返回过早：{elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_millis(2_000),
        "超时未生效，返回过晚：{elapsed:?}"
    );
    release_tx.send(()).unwrap();
    silent.join().unwrap();
}

#[test]
fn finish_times_out_without_final_result() {
    let (endpoint, server) = spawn_server("/api/v3/sauc/bigmodel_async", |ws, frames| {
        drain_until_closed(ws, frames);
    });
    let mut session = CloudSession::connect(&config(endpoint)).unwrap();
    session.send_audio(&pcm(3_200)).unwrap();

    let started = Instant::now();
    let result = session.finish(Duration::from_millis(300));
    let elapsed = started.elapsed();
    assert!(
        matches!(
            result,
            Err(CloudAsrError::Timeout {
                stage: TimeoutStage::FinalResult,
                ..
            })
        ),
        "期望超时错误，实际：{result:?}"
    );
    assert!(
        elapsed >= Duration::from_millis(250),
        "返回过早：{elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_millis(2_000),
        "超时未生效，返回过晚：{elapsed:?}"
    );

    // Dropping the session closes the socket, so the mock sees every frame and exits.
    let (_, frames) = server.join().unwrap();
    assert_eq!(frames.len(), 3, "期望 full request + 1 个整包 + 1 个末包");
    assert_eq!((frames[1].flags, frames[1].sequence), (0b0001, 2));
    assert_eq!((frames[2].flags, frames[2].sequence), (0b0011, -3));
}

#[test]
fn handshake_rejection_maps_to_rejected() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let reject = |_: &Request, _: Response| -> Result<Response, ErrorResponse> {
            let mut response = ErrorResponse::new(Some("invalid access key".to_owned()));
            *response.status_mut() = StatusCode::UNAUTHORIZED;
            Err(response)
        };
        let _ = accept_hdr(stream, reject);
    });

    let result = CloudSession::connect(&config(format!("ws://{addr}/api/v3/sauc/bigmodel_async")));
    match result {
        Err(CloudAsrError::Rejected { status, message }) => {
            assert_eq!(status, 401);
            assert!(
                message.contains("invalid access key"),
                "期望错误信息包含服务端原因，实际：{message}"
            );
        }
        other => panic!("期望 Rejected(401)，实际：{other:?}"),
    }
    server.join().unwrap();
}
