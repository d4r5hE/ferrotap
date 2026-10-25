//! G11 assistant: talks to a (fake) Ollama; session details are masked before they are sent.
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use ferrotap::ai_assist::*;
use ferrotap::model::SessionRecord;

/// Fake Ollama: /api/tags lists two models, /api/chat echoes a fixed answer; requests are kept.
fn fake() -> (String, Arc<Mutex<Vec<String>>>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s2 = seen.clone();
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            let mut s = s;
            let mut buf = Vec::new();
            let mut tmp = [0u8; 65536];
            loop {
                let n = s.read(&mut tmp).unwrap_or(0);
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&tmp[..n]);
                let text = String::from_utf8_lossy(&buf);
                if let Some(i) = text.find("\r\n\r\n") {
                    let len = text.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))).unwrap_or(0);
                    if buf.len() >= i + 4 + len {
                        break;
                    }
                }
            }
            let req = String::from_utf8_lossy(&buf).to_string();
            s2.lock().unwrap().push(req.clone());
            let body = if req.contains("/api/tags") {
                r#"{"models":[{"name":"qwen3.5:4b"},{"name":"llama3:8b"}]}"#.to_string()
            } else {
                r##"{"message":{"role":"assistant","content":"Session #1 failed with 500 because the server errored."}}"##.to_string()
            };
            let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
        }
    });
    (url, seen)
}

#[test]
fn models_question_and_masking() {
    let (url, seen) = fake();
    assert_eq!(list_models(&url).unwrap(), vec!["qwen3.5:4b", "llama3:8b"]);
    let mut s = SessionRecord { id: 1, method: "GET".into(), url: "https://api.test/v1/orders?token=abc".into(), status: 500, duration_ms: 120, ..Default::default() };
    s.request_headers.insert("authorization".into(), "Bearer topsecret".into());
    s.set_response_body(b"{\"error\":\"boom\"}");
    let ctx = context(&[s], &ferrotap::qol::PrivacySettings::default());
    assert!(ctx.contains("### Session #1: GET") && ctx.contains("-> 500"), "{ctx}");
    assert!(!ctx.contains("topsecret") && !ctx.contains("token=abc"), "masked: {ctx}");
    assert!(ctx.contains("boom"));
    let a = ask(&url, "qwen3.5:4b", "Why did it fail?", &ctx).unwrap();
    assert!(a.contains("#1 failed"));
    let reqs = seen.lock().unwrap().clone();
    let chat = reqs.iter().find(|r| r.contains("/api/chat")).unwrap();
    assert!(chat.contains("\"stream\":false") && chat.contains("Why did it fail?") && !chat.contains("topsecret"));
    assert!(list_models("https://x").is_err(), "only plain http to the local server");
}
