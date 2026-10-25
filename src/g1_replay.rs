//! R3 Group G1: replay of captured sessions (sequential / parallel / advanced) and grid menu action mapping.
use crate::model::{composer_send, ComposerRequest, ComposerResponse, SessionRecord};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayMode {
    Sequential,
    Parallel,
}

/// Advanced replay settings: repeat count, delay between requests, and execution mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplayOptions {
    pub count: usize,
    pub delay_ms: u64,
    pub mode: ReplayMode,
}

impl Default for ReplayOptions {
    fn default() -> Self {
        ReplayOptions { count: 1, delay_ms: 0, mode: ReplayMode::Sequential }
    }
}

/// Context menu copy actions of the session grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyAction {
    Url,
    Row,
    Summary,
    Curl,
    PowerShell,
    Fetch,
    Python,
}

/// Text placed on the clipboard for a copy action.
pub fn copy_text_for(action: CopyAction, s: &SessionRecord) -> String {
    match action {
        CopyAction::Url => s.url.clone(),
        CopyAction::Row => crate::g1::copy_row(s),
        CopyAction::Summary => crate::g1::copy_summary(s),
        CopyAction::Curl => crate::codegen::to_curl(s),
        CopyAction::PowerShell => crate::codegen::to_powershell(s),
        CopyAction::Fetch => crate::codegen::to_fetch(s),
        CopyAction::Python => crate::codegen::to_python(s),
    }
}

/// CONNECT tunnels carry no request of their own, so they are never replayed.
pub fn replayable(s: &SessionRecord) -> bool {
    !s.method.eq_ignore_ascii_case("CONNECT") && !s.url.is_empty()
}

/// Build a dispatchable request from a captured session (stale framing headers are dropped).
pub fn session_to_request(s: &SessionRecord) -> ComposerRequest {
    let headers = s
        .request_headers
        .iter()
        .filter(|(k, _)| {
            !["content-length", "connection", "proxy-connection", "transfer-encoding"]
                .contains(&k.to_ascii_lowercase().as_str())
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    ComposerRequest { method: s.method.clone(), url: s.url.clone(), headers, body: s.request_body.clone() }
}

/// Replay sessions through the capture proxy. Results are returned in job order.
pub fn replay_sessions(sessions: &[SessionRecord], proxy: &str, opt: ReplayOptions) -> Vec<ComposerResponse> {
    let reqs: Vec<ComposerRequest> = sessions.iter().filter(|s| replayable(s)).map(session_to_request).collect();
    let mut jobs: Vec<ComposerRequest> = Vec::new();
    for _ in 0..opt.count.max(1) {
        for r in &reqs {
            jobs.push(ComposerRequest { method: r.method.clone(), url: r.url.clone(), headers: r.headers.clone(), body: r.body.clone() });
        }
    }
    match opt.mode {
        ReplayMode::Sequential => {
            let mut out = Vec::new();
            for (i, r) in jobs.iter().enumerate() {
                if i > 0 && opt.delay_ms > 0 {
                    std::thread::sleep(Duration::from_millis(opt.delay_ms));
                }
                out.push(composer_send(r, proxy));
            }
            out
        }
        ReplayMode::Parallel => std::thread::scope(|sc| {
            let handles: Vec<_> = jobs
                .iter()
                .enumerate()
                .map(|(i, r)| {
                    sc.spawn(move || {
                        if opt.delay_ms > 0 {
                            std::thread::sleep(Duration::from_millis(opt.delay_ms * i as u64));
                        }
                        composer_send(r, proxy)
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| {
                    h.join().unwrap_or_else(|_| ComposerResponse { error: "replay thread panicked".into(), ..Default::default() })
                })
                .collect()
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::g1::ClearKind;
    use crate::model::SessionModel;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    fn rec(id: usize, method: &str, url: &str) -> SessionRecord {
        let mut s = SessionRecord { id, method: method.into(), url: url.into(), status: 200, ..Default::default() };
        s.request_headers.insert("Accept".into(), "text/plain".into());
        s.request_headers.insert("Content-Length".into(), "999".into());
        s
    }

    /// Fake proxy: answers `n` connections with "ok" and records each request head.
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
                        if k == 0 { break; }
                        buf.extend_from_slice(&b[..k]);
                    }
                    log.lock().unwrap().push(String::from_utf8_lossy(&buf).to_string());
                    let _ = st.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
                }));
            }
            for h in hs { let _ = h.join(); }
        });
        (addr, log, h)
    }

    #[test]
    fn copy_actions_match_generators() {
        let s = rec(7, "GET", "http://example.com/a?b=1");
        assert_eq!(copy_text_for(CopyAction::Url, &s), "http://example.com/a?b=1");
        assert_eq!(copy_text_for(CopyAction::Curl, &s), crate::codegen::to_curl(&s));
        assert_eq!(copy_text_for(CopyAction::PowerShell, &s), crate::codegen::to_powershell(&s));
        assert_eq!(copy_text_for(CopyAction::Fetch, &s), crate::codegen::to_fetch(&s));
        assert_eq!(copy_text_for(CopyAction::Python, &s), crate::codegen::to_python(&s));
        assert!(copy_text_for(CopyAction::Curl, &s).contains("example.com/a?b=1"));
        assert!(copy_text_for(CopyAction::Summary, &s).starts_with("#7 GET"));
        assert!(copy_text_for(CopyAction::Row, &s).starts_with("7\tGET"));
    }

    #[test]
    fn clear_connects_action() {
        let mut m = SessionModel::default();
        m.rows = vec![rec(1, "CONNECT", "example.com:443"), rec(2, "GET", "http://example.com/")];
        assert_eq!(m.clear_kind(ClearKind::Connect), 1);
        assert_eq!(m.rows.len(), 1);
        assert_eq!(m.rows[0].id, 2);
    }

    #[test]
    fn request_building_drops_stale_framing() {
        let r = session_to_request(&rec(1, "GET", "http://example.com/"));
        assert!(r.headers.iter().any(|(k, _)| k == "Accept"));
        assert!(!r.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-length")));
        assert!(!replayable(&rec(2, "CONNECT", "example.com:443")));
        assert!(replayable(&rec(3, "get", "http://example.com/")));
    }

    #[test]
    fn replay_sequential_skips_connect() {
        let (addr, log, h) = fake_proxy(1);
        let sessions = vec![rec(1, "CONNECT", "example.com:443"), rec(2, "GET", "http://example.com/x")];
        let out = replay_sessions(&sessions, &addr, ReplayOptions::default());
        h.join().unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].status, 200, "error: {}", out[0].error);
        assert_eq!(out[0].body, "ok");
        let l = log.lock().unwrap();
        assert!(l[0].starts_with("GET http://example.com/x HTTP/1.1"));
        assert!(l[0].contains("Accept: text/plain"));
    }

    #[test]
    fn replay_advanced_count_delay_parallel() {
        let (addr, log, h) = fake_proxy(6);
        let sessions = vec![rec(1, "GET", "http://example.com/a"), rec(2, "GET", "http://example.com/b")];
        let t = std::time::Instant::now();
        let seq = replay_sessions(&sessions, &addr, ReplayOptions { count: 2, delay_ms: 30, mode: ReplayMode::Sequential });
        assert_eq!(seq.len(), 4);
        assert!(t.elapsed() >= Duration::from_millis(90));
        let par = replay_sessions(&sessions, &addr, ReplayOptions { count: 1, delay_ms: 0, mode: ReplayMode::Parallel });
        h.join().unwrap();
        assert_eq!(par.len(), 2);
        assert!(seq.iter().chain(par.iter()).all(|r| r.status == 200 && r.error.is_empty()));
        assert_eq!(log.lock().unwrap().len(), 6);
    }

    #[test]
    fn replay_reports_error_for_dead_proxy() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap().to_string();
        drop(l);
        let out = replay_sessions(&[rec(1, "GET", "http://example.com/")], &addr, ReplayOptions::default());
        assert_eq!(out.len(), 1);
        assert!(!out[0].error.is_empty());
    }
}
