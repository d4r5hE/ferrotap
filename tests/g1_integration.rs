//! Integration tests for Group G1: session grid context menu actions and replay mechanics.
use ferrotap::g1::{copy_row, copy_summary, ClearKind};
use ferrotap::g1_replay::{
    copy_text_for, replay_sessions, replayable, session_to_request, CopyAction, ReplayMode, ReplayOptions,
};
use ferrotap::model::{SessionModel, SessionRecord};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

fn rec(id: usize, method: &str, url: &str) -> SessionRecord {
    let mut s = SessionRecord { id, method: method.into(), url: url.into(), status: 200, ..Default::default() };
    s.request_headers.insert("Accept".into(), "text/plain".into());
    s.request_headers.insert("X-Token".into(), "abc123".into());
    s.request_headers.insert("Content-Length".into(), "999".into());
    s
}

/// Fake proxy answering `n` connections with "ok"; records every request head.
fn fake_proxy(n: usize) -> (String, Arc<Mutex<Vec<String>>>, std::thread::JoinHandle<()>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let log = Arc::new(Mutex::new(Vec::new()));
    let log2 = log.clone();
    let h = std::thread::spawn(move || {
        let mut hs = Vec::new();
        for _ in 0..n {
            let (mut st, _) = l.accept().unwrap();
            let log = log2.clone();
            hs.push(std::thread::spawn(move || {
                let mut buf = Vec::new();
                let mut b = [0u8; 1024];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    let k = st.read(&mut b).unwrap_or(0);
                    if k == 0 {
                        break;
                    }
                    buf.extend_from_slice(&b[..k]);
                }
                log.lock().unwrap().push(String::from_utf8_lossy(&buf).to_string());
                let _ = st.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
            }));
        }
        for h in hs {
            let _ = h.join();
        }
    });
    (addr, log, h)
}

#[test]
fn copy_menu_actions_produce_expected_text() {
    let s = rec(1, "GET", "http://example.com/api?q=1");
    assert_eq!(copy_text_for(CopyAction::Url, &s), "http://example.com/api?q=1");
    assert_eq!(copy_text_for(CopyAction::Row, &s), copy_row(&s));
    assert_eq!(copy_text_for(CopyAction::Summary, &s), copy_summary(&s));
    assert!(copy_text_for(CopyAction::Curl, &s).contains("curl"));
    assert!(copy_text_for(CopyAction::Curl, &s).contains("http://example.com/api?q=1"));
    let ps = copy_text_for(CopyAction::PowerShell, &s);
    assert!(ps.contains("Invoke-WebRequest") || ps.contains("Invoke-RestMethod"));
    assert!(copy_text_for(CopyAction::Fetch, &s).contains("fetch("));
    assert!(copy_text_for(CopyAction::Python, &s).contains("requests"));
}

#[test]
fn clear_connects_and_remove_selected() {
    let mut m = SessionModel::default();
    m.rows = vec![
        rec(1, "CONNECT", "example.com:443"),
        rec(2, "GET", "http://example.com/a"),
        rec(3, "CONNECT", "other.com:443"),
        rec(4, "POST", "http://example.com/b"),
    ];
    assert_eq!(m.clear_kind(ClearKind::Connect), 2);
    assert_eq!(m.rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![2, 4]);
    assert_eq!(m.remove_ids(&[2]), 1);
    assert_eq!(m.rows.len(), 1);
    assert_eq!(m.rows[0].id, 4);
}

#[test]
fn replay_request_building_and_filtering() {
    let r = session_to_request(&rec(1, "GET", "http://example.com/"));
    assert!(r.headers.iter().any(|(k, v)| k == "X-Token" && v == "abc123"));
    assert!(!r.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-length")));
    assert!(!replayable(&rec(2, "CONNECT", "example.com:443")));
    assert!(replayable(&rec(3, "POST", "http://example.com/x")));
}

#[test]
fn replay_sequential_hits_proxy_and_skips_connect() {
    let (addr, log, h) = fake_proxy(2);
    let sessions = vec![
        rec(1, "GET", "http://example.com/one"),
        rec(2, "CONNECT", "example.com:443"),
        rec(3, "GET", "http://example.com/two"),
    ];
    let out = replay_sessions(&sessions, &addr, ReplayOptions::default());
    h.join().unwrap();
    assert_eq!(out.len(), 2);
    assert!(out.iter().all(|r| r.error.is_empty()), "{:?}", out.iter().map(|r| r.error.clone()).collect::<Vec<_>>());
    let log = log.lock().unwrap();
    assert_eq!(log.len(), 2);
    assert!(log.iter().any(|l| l.contains("/one")));
    assert!(log.iter().any(|l| l.contains("/two")));
}

#[test]
fn replay_advanced_count_and_parallel() {
    let (addr, log, h) = fake_proxy(6);
    let sessions = vec![rec(1, "GET", "http://example.com/x1"), rec(2, "GET", "http://example.com/x2")];
    let opt = ReplayOptions { count: 3, delay_ms: 5, mode: ReplayMode::Parallel };
    let out = replay_sessions(&sessions, &addr, opt);
    h.join().unwrap();
    assert_eq!(out.len(), 6);
    assert_eq!(log.lock().unwrap().len(), 6);
}

#[test]
fn replay_dead_proxy_reports_error() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    drop(l);
    let out = replay_sessions(&[rec(1, "GET", "http://example.com/")], &addr, ReplayOptions::default());
    assert_eq!(out.len(), 1);
    assert!(!out[0].error.is_empty());
}
