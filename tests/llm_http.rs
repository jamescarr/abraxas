//! `OpenAiCompat` against a tiny hand-written HTTP/1.1 server.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use abraxas::config::ResponseFormatKind;
use abraxas::llm::{Llm, LlmError, LlmReply, LlmRequest, Message, OpenAiCompat, Usage};
use serde_json::{Value, json};

#[derive(Clone)]
enum Step {
    /// Send one HTTP chunk.
    Chunk(String),
    Sleep(u64),
    /// Drop the socket without the terminating chunk.
    Disconnect,
}

#[derive(Clone)]
enum Script {
    Status(u16, String),
    Sse(Vec<Step>),
}

struct Received {
    headers: Vec<(String, String)>,
    body: Value,
}

struct Mock {
    base_url: String,
    received: Arc<Mutex<Vec<Received>>>,
}

impl Mock {
    fn start(script: Script) -> Mock {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
        let received = Arc::new(Mutex::new(Vec::new()));
        let rec = Arc::clone(&received);
        thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(conn) = conn else { break };
                let script = script.clone();
                let rec = Arc::clone(&rec);
                thread::spawn(move || serve(conn, &script, &rec));
            }
        });
        Mock { base_url, received }
    }

    fn client(
        &self,
        key: Option<&str>,
        effort: Option<&str>,
        fmt: ResponseFormatKind,
    ) -> OpenAiCompat {
        OpenAiCompat::with_base_url(
            &self.base_url,
            key.map(str::to_owned),
            effort.map(str::to_owned),
            fmt,
            Duration::from_secs(10),
        )
    }

    fn last(&self) -> (Vec<(String, String)>, Value) {
        let r = self.received.lock().unwrap();
        let r = r.last().expect("no request received");
        (r.headers.clone(), r.body.clone())
    }
}

fn serve(mut s: TcpStream, script: &Script, rec: &Mutex<Vec<Received>>) {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        match s.read(&mut tmp) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let headers: Vec<(String, String)> = head
        .lines()
        .skip(1)
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    let len: usize = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    while buf.len() < head_end + len {
        match s.read(&mut tmp) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
        }
    }
    let body = serde_json::from_slice(&buf[head_end..head_end + len]).unwrap_or(Value::Null);
    rec.lock().unwrap().push(Received { headers, body });

    match script {
        Script::Status(code, text) => {
            let resp = format!(
                "HTTP/1.1 {code} X\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                text.len()
            );
            let _ = s.write_all(resp.as_bytes());
        }
        Script::Sse(steps) => {
            let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
            if s.write_all(head.as_bytes()).is_err() {
                return;
            }
            for step in steps {
                match step {
                    Step::Chunk(c) => {
                        let frame = format!("{:x}\r\n{c}\r\n", c.len());
                        if s.write_all(frame.as_bytes()).is_err() || s.flush().is_err() {
                            return;
                        }
                    }
                    Step::Sleep(ms) => thread::sleep(Duration::from_millis(*ms)),
                    Step::Disconnect => return,
                }
            }
            let _ = s.write_all(b"0\r\n\r\n");
        }
    }
}

fn delta(content: &str) -> String {
    format!(
        "data: {}\n\n",
        json!({"choices":[{"delta":{"content":content}}]})
    )
}

fn req() -> LlmRequest {
    LlmRequest {
        model: "m1".into(),
        messages: vec![Message::system("sys"), Message::user("hello")],
        temperature: 0.5,
        max_tokens: 77,
        seed: Some(42),
        json_schema: None,
    }
}

fn run(c: &OpenAiCompat, r: &LlmRequest) -> (Result<LlmReply, LlmError>, Vec<String>) {
    let mut deltas = Vec::new();
    let cancel = AtomicBool::new(false);
    let res = c.stream(r, &mut |d| deltas.push(d.to_owned()), &cancel);
    (res, deltas)
}

#[test]
fn streams_text_filters_think_and_reasoning_and_reads_usage() {
    let raw_reasoning = format!(
        "data: {}\n\ndata: {}\n\n",
        json!({"choices":[{"delta":{"reasoning_content":"pondering"}}]}),
        json!({"choices":[{"delta":{"reasoning":"more","content":""}}]}),
    );
    let usage = format!(
        "data: {}\n\n",
        json!({"choices":[],"usage":{"prompt_tokens":9,"completion_tokens":4}})
    );
    // A line split across HTTP chunks, tags split across deltas.
    let first = delta("Hel");
    let (a, b) = first.split_at(10);
    let steps = vec![
        Step::Chunk(": ping\r\n\r\n".into()),
        Step::Chunk(a.into()),
        Step::Sleep(20),
        Step::Chunk(b.into()),
        Step::Chunk(raw_reasoning),
        Step::Chunk(delta("lo <thi")),
        Step::Chunk(delta("nk>secret</th")),
        Step::Chunk(delta("ink> world")),
        Step::Chunk(usage),
        Step::Chunk("data: [DONE]\n\n".into()),
    ];
    let mock = Mock::start(Script::Sse(steps));
    let c = mock.client(None, None, ResponseFormatKind::JsonSchema);
    let (res, deltas) = run(&c, &req());
    let reply = res.unwrap();
    assert_eq!(reply.text, "Hello  world");
    assert_eq!(deltas.concat(), "Hello  world");
    assert!(
        !deltas
            .iter()
            .any(|d| d.contains("pondering") || d.contains("secret"))
    );
    assert_eq!(
        reply.usage,
        Some(Usage {
            prompt_tokens: 9,
            completion_tokens: 4
        })
    );
}

fn ok_script() -> Script {
    Script::Sse(vec![
        Step::Chunk(delta("ok")),
        Step::Chunk("data: [DONE]\n\n".into()),
    ])
}

#[test]
fn request_body_plain_text_call() {
    let mock = Mock::start(ok_script());
    let c = mock.client(None, None, ResponseFormatKind::JsonSchema);
    run(&c, &req()).0.unwrap();
    let (headers, body) = mock.last();
    assert_eq!(body["model"], "m1");
    assert_eq!(body["stream"], true);
    assert_eq!(body["stream_options"], json!({"include_usage": true}));
    assert_eq!(body["max_tokens"], 77);
    assert_eq!(body["seed"], 42);
    assert_eq!(
        body["messages"][0],
        json!({"role":"system","content":"sys"})
    );
    assert_eq!(
        body["messages"][1],
        json!({"role":"user","content":"hello"})
    );
    assert!(body.get("reasoning_effort").is_none());
    assert!(body.get("response_format").is_none());
    assert!(!headers.iter().any(|(k, _)| k == "authorization"));
}

#[test]
fn request_body_json_schema_and_options() {
    let mock = Mock::start(ok_script());
    let c = mock.client(Some("sekrit"), Some("none"), ResponseFormatKind::JsonSchema);
    let mut r = req();
    r.seed = None;
    let schema = json!({"type":"object","properties":{"a":{"type":"string"}}});
    r.json_schema = Some(("thing".into(), schema.clone()));
    run(&c, &r).0.unwrap();
    let (headers, body) = mock.last();
    assert_eq!(
        body["response_format"],
        json!({"type":"json_schema","json_schema":{"name":"thing","schema":schema,"strict":false}})
    );
    assert_eq!(body["reasoning_effort"], "none");
    assert!(body.get("seed").is_none());
    assert!(headers.contains(&("authorization".into(), "Bearer sekrit".into())));
}

#[test]
fn request_body_json_object_format() {
    let mock = Mock::start(ok_script());
    let c = mock.client(None, None, ResponseFormatKind::JsonObject);
    let mut r = req();
    r.json_schema = Some(("thing".into(), json!({"type":"object"})));
    run(&c, &r).0.unwrap();
    assert_eq!(
        mock.last().1["response_format"],
        json!({"type":"json_object"})
    );
}

#[test]
fn http_error_carries_status_and_truncated_body() {
    let mock = Mock::start(Script::Status(500, "x".repeat(1000)));
    let c = mock.client(None, None, ResponseFormatKind::JsonSchema);
    match run(&c, &req()).0 {
        Err(LlmError::Status(500, body)) => assert_eq!(body.chars().count(), 300),
        other => panic!("unexpected {other:?}"),
    }
    let mock = Mock::start(Script::Status(400, "{\"error\":\"bad schema\"}".into()));
    let c = mock.client(None, None, ResponseFormatKind::JsonSchema);
    match run(&c, &req()).0 {
        Err(LlmError::Status(400, body)) => assert!(body.contains("bad schema")),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn connection_refused_is_transport() {
    let port = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let c = OpenAiCompat::with_base_url(
        &format!("http://127.0.0.1:{port}/v1"),
        None,
        None,
        ResponseFormatKind::JsonSchema,
        Duration::from_secs(5),
    );
    assert!(matches!(run(&c, &req()).0, Err(LlmError::Transport(_))));
}

#[test]
fn cancel_mid_stream_stops_early() {
    let mut steps = Vec::new();
    for i in 0..10 {
        steps.push(Step::Chunk(delta(&format!("w{i} "))));
        steps.push(Step::Sleep(30));
    }
    steps.push(Step::Chunk("data: [DONE]\n\n".into()));
    let mock = Mock::start(Script::Sse(steps));
    let c = mock.client(None, None, ResponseFormatKind::JsonSchema);
    let cancel = AtomicBool::new(false);
    let mut deltas = 0;
    let res = c.stream(
        &req(),
        &mut |_| {
            deltas += 1;
            if deltas == 2 {
                cancel.store(true, Ordering::Relaxed);
            }
        },
        &cancel,
    );
    assert!(matches!(res, Err(LlmError::Cancelled)), "{res:?}");
    assert!((2..10).contains(&deltas), "deltas {deltas}");
}

#[test]
fn mid_stream_disconnect_is_stream_error_with_partial_text() {
    let steps = vec![
        Step::Chunk(delta("partial ")),
        Step::Chunk(delta("text")),
        Step::Sleep(30),
        Step::Disconnect,
    ];
    let mock = Mock::start(Script::Sse(steps));
    let c = mock.client(None, None, ResponseFormatKind::JsonSchema);
    let (res, deltas) = run(&c, &req());
    assert!(matches!(res, Err(LlmError::Stream(_))), "{res:?}");
    assert_eq!(deltas.concat(), "partial text");
}

#[test]
fn error_object_in_stream_is_stream_error() {
    let steps = vec![
        Step::Chunk(delta("hi")),
        Step::Chunk("data: {\"error\":{\"message\":\"out of memory\"}}\n\n".into()),
    ];
    let mock = Mock::start(Script::Sse(steps));
    let c = mock.client(None, None, ResponseFormatKind::JsonSchema);
    match run(&c, &req()).0 {
        Err(LlmError::Stream(m)) => assert!(m.contains("out of memory")),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn hanging_body_times_out() {
    let steps = vec![Step::Chunk(delta("a")), Step::Sleep(3000)];
    let mock = Mock::start(Script::Sse(steps));
    let c = OpenAiCompat::with_base_url(
        &mock.base_url,
        None,
        None,
        ResponseFormatKind::JsonSchema,
        Duration::from_millis(300),
    );
    let (res, _) = run(&c, &req());
    assert!(matches!(res, Err(LlmError::Timeout)), "{res:?}");
}

#[test]
fn warmup_sends_one_token_request() {
    let mock = Mock::start(ok_script());
    let c = mock.client(None, None, ResponseFormatKind::JsonSchema);
    c.warmup("tiny");
    let (_, body) = mock.last();
    assert_eq!(body["max_tokens"], 1);
    assert_eq!(body["model"], "tiny");
}
