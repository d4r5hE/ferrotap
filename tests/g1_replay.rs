//! G1: replaying recorded sessions reproduces method, URL, headers and body, in order.
use ferrotap::g1_replay::{replay_sessions, replayable, session_to_request, ReplayMode, ReplayOptions};
use ferrotap::model::SessionRecord;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

fn rec(id: usize, method: &str, url: &str, body: &str) -> SessionRecord {
    let mut s = SessionRecord { id, method: method.into(), url: url.into(), status: 200, ..Default::default() };
    s.request_headers.insert("X-Trace".into(), format!("t{id}"));
    s.request_headers.insert("Content-Length".into(), "999".into());
    s.request_body = body.into();
    s
}

/// Fake proxy: serves `n` connections one after another; logs (head, body) of each.
fn fake_proxy(n: usize) -> (String, Arc<Mutex<Vec<(String, String)>>>, std::thread::JoinHandle<()>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let log = Arc::new(Mutex::new(Vec::new()));
    let log2 = log.clone();
    let h = std::thread::spawn(move || {
        for _ in 0..n {
            let (mut st, _) = l.accept().unwrap();
            let mut buf = Vec::new();
            let mut b = [0u8; 2048];
            let head_end = loop {
                if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") { break p + 4; }
                let k = st.read(&mut b).unwrap_or(0);
                if k == 0 { break buf.len(); }
                buf.extend_from_slice(&b[..k]);
            };
            let head = String::from_utf8_lossy(&buf[..head_end.min(buf.len())]).to_string();
            let want = head
                .lines()
                .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
                .unwrap_or(0);
            while buf.len() < head_end + want {
                let k = st.read(&mut b).unwrap_or(0);
                if k == 0 { break; }
                buf.extend_from_slice(&b[..k]);
            }
            let body = String::from_utf8_lossy(&buf[head_end.min(buf.len())..]).to_string();
            log2.lock().unwrap().push((head, body));
            let _ = st.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
        }
    });
    (addr, log, h)
}

#[test]
fn session_to_request_keeps_fields_and_drops_framing_headers() {
    let s = rec(1, "POST", "http://example.com/a?b=1", "hello");
    let r = session_to_request(&s);
    assert_eq!(r.method, "POST");
    assert_eq!(r.url, "http://example.com/a?b=1");
    assert_eq!(r.body, "hello");
    assert!(r.headers.iter().any(|(k, v)| k == "X-Trace" && v == "t1"));
    assert!(!r.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-length")));
}

#[test]
fn connect_and_empty_url_are_not_replayable() {
    assert!(!replayable(&rec(1, "CONNECT", "example.com:443", "")));
    assert!(!replayable(&rec(2, "GET", "", "")));
    assert!(replayable(&rec(3, "GET", "http://example.com/", "")));
}

#[test]
fn sequential_replay_reproduces_requests_in_order() {
    let sessions = vec![
        rec(1, "GET", "http://example.com/one", ""),
        rec(2, "POST", "http://example.com/two", "payload-two"),
        rec(3, "CONNECT", "example.com:443", ""),
        rec(4, "PUT", "http://example.com/four", "p4"),
    ];
    let (addr, log, h) = fake_proxy(3);
    let out = replay_sessions(&sessions, &addr, ReplayOptions::default());
    h.join().unwrap();
    assert_eq!(out.len(), 3);
    assert!(out.iter().all(|r| r.error.is_empty()), "errors: {:?}", out.iter().map(|r| r.error.clone()).collect::<Vec<_>>());
    let log = log.lock().unwrap();
    assert_eq!(log.len(), 3);
    let first = log[0].0.lines().next().unwrap();
    assert!(first.starts_with("GET ") && first.contains("/one"), "{first}");
    let second = log[1].0.lines().next().unwrap();
    assert!(second.starts_with("POST ") && second.contains("/two"), "{second}");
    assert_eq!(log[1].1, "payload-two");
    assert!(log[1].0.to_ascii_lowercase().contains("x-trace: t2"));
    let third = log[2].0.lines().next().unwrap();
    assert!(third.starts_with("PUT ") && third.contains("/four"), "{third}");
    assert_eq!(log[2].1, "p4");
    // stale Content-Length 999 must not leak through
    assert!(!log[1].0.contains("999"));
}

#[test]
fn count_repeats_whole_session_list_in_order() {
    let sessions = vec![rec(1, "GET", "http://example.com/a", ""), rec(2, "GET", "http://example.com/b", "")];
    let (addr, log, h) = fake_proxy(4);
    let opt = ReplayOptions { count: 2, delay_ms: 0, mode: ReplayMode::Sequential };
    let out = replay_sessions(&sessions, &addr, opt);
    h.join().unwrap();
    assert_eq!(out.len(), 4);
    let paths: Vec<String> = log.lock().unwrap().iter().map(|(h, _)| h.lines().next().unwrap().split(' ').nth(1).unwrap().to_string()).collect();
    let tails: Vec<&str> = paths.iter().map(|p| p.rsplit('/').next().unwrap()).collect();
    assert_eq!(tails, vec!["a", "b", "a", "b"]);
}

#[test]
fn parallel_replay_sends_everything_and_returns_in_job_order() {
    let sessions: Vec<_> = (1..=3).map(|i| rec(i, "GET", &format!("http://example.com/p{i}"), "")).collect();
    let (addr, log, h) = fake_proxy(3);
    let opt = ReplayOptions { count: 1, delay_ms: 0, mode: ReplayMode::Parallel };
    let out = replay_sessions(&sessions, &addr, opt);
    h.join().unwrap();
    assert_eq!(out.len(), 3);
    assert!(out.iter().all(|r| r.error.is_empty()));
    assert_eq!(log.lock().unwrap().len(), 3);
}

#[test]
fn unreachable_proxy_reports_error_per_request() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    drop(l);
    let out = replay_sessions(&[rec(1, "GET", "http://example.com/", "")], &addr, ReplayOptions::default());
    assert_eq!(out.len(), 1);
    assert!(!out[0].error.is_empty());
}
