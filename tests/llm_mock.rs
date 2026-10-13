#![cfg(feature = "llm")]
//! LLM 契约：进程内 mock 的 OpenAI 兼容 chat/completions 端点（127.0.0.1 随机端口）。

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    sync::mpsc,
    thread,
    time::Duration,
};

use yuda::{
    config::LlmConfig,
    refine::{refine, rewrite, EDIT_PROMPT, SYSTEM_PROMPT},
};

struct Captured {
    request_line: String,
    authorization: Option<String>,
    body: serde_json::Value,
}

/// 启动只服务一个请求的 mock 端点，返回 api_base 与收到的请求。
fn serve_once(reply: &'static str, delay: Duration) -> (String, mpsc::Receiver<Captured>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("mock 端点应能监听");
    let address = listener.local_addr().expect("本地地址");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let Ok((stream, _)) = listener.accept() else {
            return;
        };
        let mut reader = BufReader::new(stream.try_clone().expect("clone"));
        let mut request_line = String::new();
        reader.read_line(&mut request_line).expect("请求行");
        let mut length = 0;
        let mut authorization = None;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).expect("请求头");
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            let lower = line.to_ascii_lowercase();
            if let Some(value) = lower.strip_prefix("content-length:") {
                length = value.trim().parse().expect("长度");
            }
            if lower.starts_with("authorization:") {
                authorization = Some(line["authorization:".len()..].trim().to_owned());
            }
        }
        let mut body = vec![0; length];
        reader.read_exact(&mut body).expect("请求体");
        let _ = tx.send(Captured {
            request_line: request_line.trim_end().to_owned(),
            authorization,
            body: serde_json::from_slice(&body).expect("请求体应为 JSON"),
        });
        thread::sleep(delay);
        let payload = format!(
            r#"{{"choices":[{{"message":{{"role":"assistant","content":{}}}}}]}}"#,
            serde_json::to_string(reply).expect("json")
        );
        let mut stream = stream;
        let _ = write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            payload.len(),
            payload
        );
    });
    (format!("http://{address}/v1"), rx)
}

fn config(api_base: String, api_key: &str, timeout_ms: u64) -> LlmConfig {
    LlmConfig {
        enabled: true,
        api_base,
        api_key: api_key.to_owned(),
        model: "test-model".to_owned(),
        timeout_ms,
    }
}

#[test]
fn refine_sends_the_verbatim_prompt_and_accepts_small_fixes() {
    let (base, rx) = serve_once("用 Python 解析 JSON", Duration::ZERO);
    let refined = refine(&config(base, "sk-test", 3_000), "用配森解析杰森").expect("纠错应成功");
    assert_eq!(refined, "用 Python 解析 JSON");
    let request = rx.recv().expect("应收到请求");
    assert_eq!(request.request_line, "POST /v1/chat/completions HTTP/1.1");
    assert_eq!(request.authorization.as_deref(), Some("Bearer sk-test"));
    assert_eq!(request.body["model"], "test-model");
    assert_eq!(request.body["messages"][0]["content"], SYSTEM_PROMPT);
    assert_eq!(request.body["messages"][1]["content"], "用配森解析杰森");
}

#[test]
fn empty_api_key_sends_no_authorization_header() {
    let (base, rx) = serve_once("好的。", Duration::ZERO);
    refine(&config(base, "", 3_000), "好的。").expect("纠错应成功");
    assert_eq!(rx.recv().expect("应收到请求").authorization, None);
}

#[test]
fn refine_rejects_rewrites_that_drift_from_the_original() {
    let (base, _rx) = serve_once(
        "这是一段完全不同而且长得多的回答，解释了我为什么修改了原文。",
        Duration::ZERO,
    );
    let error = refine(&config(base, "", 3_000), "今天开会").expect_err("偏离过大应被拒绝");
    assert!(format!("{error:#}").contains("偏离原文"));
}

#[test]
fn rewrite_applies_freeform_instructions() {
    let (base, rx) = serve_once("Meeting at 3 pm today.", Duration::ZERO);
    let text =
        rewrite(&config(base, "", 3_000), "今天下午3点开会。", "翻译成英文").expect("改写应成功");
    assert_eq!(text, "Meeting at 3 pm today.");
    let request = rx.recv().expect("应收到请求");
    assert_eq!(request.body["messages"][0]["content"], EDIT_PROMPT);
    let user = request.body["messages"][1]["content"]
        .as_str()
        .expect("用户消息");
    assert!(user.contains("今天下午3点开会。") && user.contains("翻译成英文"));
}

#[test]
fn slow_endpoint_times_out() {
    let (base, _rx) = serve_once("好的。", Duration::from_secs(3));
    let started = std::time::Instant::now();
    assert!(
        refine(&config(base, "", 300), "好的。").is_err(),
        "超时应返回错误"
    );
    assert!(started.elapsed() < Duration::from_secs(2), "超时上限应生效");
}
