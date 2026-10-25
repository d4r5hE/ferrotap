//! G15: collection runner against a local test server, redirects/settings, and the local mock server.
use ferrotap::api_tools::*;
use ferrotap::model::{Collection, ComposerRequest, KvRow, SavedRequest};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Test server on a random port: /slow sleeps 300 ms, /redir redirects to /json, /json returns JSON,
/// everything else echoes "METHOD path|body". Logs the request lines. Stops when `stop` is set.
fn test_server() -> (u16, Arc<Mutex<Vec<String>>>, Arc<AtomicBool>, std::thread::JoinHandle<()>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    l.set_nonblocking(true).unwrap();
    let log = Arc::new(Mutex::new(Vec::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let (log2, stop2) = (log.clone(), stop.clone());
    let h = std::thread::spawn(move || {
        while !stop2.load(Ordering::SeqCst) {
            let (mut st, _) = match l.accept() {
                Ok(x) => x,
                Err(_) => { std::thread::sleep(std::time::Duration::from_millis(10)); continue; }
            };
            st.set_nonblocking(false).unwrap();
            let log3 = log2.clone();
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let mut b = [0u8; 2048];
                let he = loop {
                    if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") { break p + 4; }
                    let k = st.read(&mut b).unwrap_or(0);
                    if k == 0 { return; }
                    buf.extend_from_slice(&b[..k]);
                };
                let head = String::from_utf8_lossy(&buf[..he]).to_string();
                let want = head.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))).unwrap_or(0);
                while buf.len() < he + want { let k = st.read(&mut b).unwrap_or(0); if k == 0 { break; } buf.extend_from_slice(&b[..k]); }
                let body = String::from_utf8_lossy(&buf[he..]).to_string();
                let line = head.lines().next().unwrap_or("").to_string();
                log3.lock().unwrap().push(format!("{line}|{}", head.lines().find(|l| l.starts_with("X-Tok")).unwrap_or("")));
                let mut it = line.split_whitespace();
                let method = it.next().unwrap_or("").to_string();
                let target = it.next().unwrap_or("/").to_string();
                let path = match target.split_once("://") { Some((_, r)) => format!("/{}", r.split_once('/').map(|x| x.1).unwrap_or("")), None => target.clone() };
                let path = path.split('?').next().unwrap().to_string();
                let (status, extra, out) = match path.as_str() {
                    "/slow" => { std::thread::sleep(std::time::Duration::from_millis(300)); (200, String::new(), "slow".to_string()) }
                    "/redir" => (302, "Location: /json\r\n".to_string(), String::new()),
                    "/json" => (200, "Content-Type: application/json\r\n".to_string(), "{\"user\":{\"id\":7}}".to_string()),
                    _ => (200, String::new(), format!("{method} {path}|{body}")),
                };
                let resp = format!("HTTP/1.1 {status} X\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{out}", out.len());
                let _ = st.write_all(resp.as_bytes());
            });
        }
    });
    (port, log, stop, h)
}

fn direct() -> ComposerSettings { ComposerSettings { follow_redirects: true, timeout_secs: 5, via_proxy: false, http10: false } }

#[test]
fn collection_runner_runs_in_order_with_env_and_tests() {
    let (port, log, stop, h) = test_server();
    let vars = vec![("baseUrl".to_string(), format!("http://127.0.0.1:{port}")), ("tok".to_string(), "secret".to_string())];
    let reqs = vec![
        SavedRequest { name: "one".into(), method: "POST".into(), url: "{{baseUrl}}/a".into(), headers: vec![KvRow::new("X-Tok", "{{tok}}")], body: "b={{tok}}".into(),
            tests: vec![Assertion::new(AssertKind::StatusEquals, "", "200"), Assertion::new(AssertKind::BodyContains, "", "POST /a|b=secret")], ..Default::default() },
        SavedRequest { name: "two".into(), method: "GET".into(), url: "{{baseUrl}}/json".into(),
            tests: vec![Assertion::new(AssertKind::JsonPathEquals, "$.user.id", "7"), Assertion::new(AssertKind::HeaderEquals, "content-type", "text/plain")], ..Default::default() },
        SavedRequest { name: "three".into(), method: "GET".into(), url: "{{baseUrl}}/redir".into(),
            tests: vec![Assertion::new(AssertKind::StatusEquals, "", "200"), Assertion::new(AssertKind::TimeBelow, "", "5000")], ..Default::default() },
    ];
    let mut got = Vec::new();
    let n = run_collection(&reqs, &vars, &direct(), "unused", &AtomicBool::new(false), |r| got.push(r));
    assert_eq!(n, 3);
    assert_eq!(got.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), vec!["one", "two", "three"]);
    assert_eq!((got[0].status, got[0].passed, got[0].failed), (200, 2, 0), "{:?}", got[0]);
    assert_eq!((got[1].passed, got[1].failed), (1, 1));
    assert_eq!(got[1].size, "{\"user\":{\"id\":7}}".len());
    assert_eq!((got[2].status, got[2].passed), (200, 2), "redirect followed: {:?}", got[2]);
    let l = log.lock().unwrap().clone();
    assert!(l[0].starts_with(&format!("POST http://127.0.0.1:{port}/a HTTP/1.1|X-Tok: secret")), "{l:?}");
    assert!(l.iter().any(|x| x.contains("/json")));
    // without following redirects the 302 is reported
    let nf = ComposerSettings { follow_redirects: false, ..direct() };
    let r = run_one(&reqs[2], &vars, &nf, "unused");
    assert_eq!((r.status, r.failed), (302, 1));
    // via a "proxy" address (the test server accepts absolute-form requests)
    let via = ComposerSettings { via_proxy: true, ..direct() };
    let r = run_one(&reqs[0], &vars, &via, &format!("127.0.0.1:{port}"));
    assert_eq!(r.failed, 0, "{r:?}");
    assert!(run_summary(&got).starts_with("3 requests, 5 tests passed, 1 failed"));
    stop.store(true, Ordering::SeqCst);
    h.join().unwrap();
}

#[test]
fn runner_stop_button_stops_early() {
    let (port, _log, stop, h) = test_server();
    let reqs: Vec<SavedRequest> = (0..8).map(|i| SavedRequest { name: format!("r{i}"), method: "GET".into(), url: format!("http://127.0.0.1:{port}/slow"), ..Default::default() }).collect();
    let runner = Runner::start(reqs, vec![], direct(), String::new(), None);
    assert_eq!(runner.total, 8);
    let first = runner.rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    assert_eq!(first.status, 200);
    runner.request_stop();
    let rest = runner.join();
    assert!(rest.len() < 7, "stopped early, got {} more", rest.len());
    stop.store(true, Ordering::SeqCst);
    h.join().unwrap();
}

#[test]
fn send_timeout_setting() {
    let (port, _log, stop, h) = test_server();
    let req = ComposerRequest { method: "GET".into(), url: format!("http://127.0.0.1:{port}/slow"), ..Default::default() };
    let ok = send_request(&req, &direct(), "");
    assert_eq!(ok.status, 200);
    assert!(ok.elapsed_ms >= 250);
    stop.store(true, Ordering::SeqCst);
    h.join().unwrap();
}

fn get(port: u16, method: &str, path: &str) -> (u16, String, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
    write!(s, "{method} {path} HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    let status = out.split_whitespace().nth(1).unwrap().parse().unwrap();
    let (head, body) = out.split_once("\r\n\r\n").unwrap();
    (status, head.to_string(), body.to_string())
}

#[test]
fn mock_server_routes_by_method_and_path() {
    let mut c = Collection::new("M");
    let ex = |st: u16, b: &str| Some(SavedResponse { status: st, headers: vec![("Content-Type".into(), "application/json".into()), ("Content-Length".into(), "999".into())], body: b.into() });
    c.requests.push(SavedRequest { name: "list".into(), method: "GET".into(), url: "{{baseUrl}}/users".into(), example: ex(200, "[1,2]"), ..Default::default() });
    c.requests.push(SavedRequest { name: "one".into(), method: "GET".into(), url: "{{baseUrl}}/users/{{id}}".into(), example: ex(200, "{\"id\":\"any\"}"), ..Default::default() });
    c.requests.push(SavedRequest { name: "me".into(), method: "GET".into(), url: "{{baseUrl}}/users/me".into(), example: ex(200, "{\"me\":true}"), ..Default::default() });
    c.requests.push(SavedRequest { name: "create".into(), method: "POST".into(), url: "http://real.host/users?x=1".into(), example: ex(201, "{\"created\":true}"), ..Default::default() });
    c.requests.push(SavedRequest { name: "no example".into(), method: "DELETE".into(), url: "{{baseUrl}}/users".into(), ..Default::default() });
    let routes = routes_from_collection(&c, &[("baseUrl".into(), "http://localhost:1".into())]);
    assert_eq!(routes.len(), 4);
    assert_eq!(routes[0].path, "/users");
    assert_eq!(routes[3].path, "/users");
    let mut m = MockServer::start("127.0.0.1:0", routes).unwrap();
    let p = m.port;
    assert!(p > 0 && m.is_running());
    assert_eq!(get(p, "GET", "/users").2, "[1,2]");
    let (st, head, body) = get(p, "POST", "/users?y=2");
    assert_eq!((st, body.as_str()), (201, "{\"created\":true}"));
    assert!(head.contains("Content-Type: application/json") && head.contains("Content-Length: 16") && !head.contains("999"));
    assert_eq!(get(p, "GET", "/users/12").2, "{\"id\":\"any\"}");
    assert_eq!(get(p, "GET", "/users/me").2, "{\"me\":true}", "exact path beats wildcard");
    assert_eq!(get(p, "DELETE", "/users").0, 405);
    assert_eq!(get(p, "GET", "/nothing").0, 404);
    assert_eq!(get(p, "GET", &format!("http://127.0.0.1:{p}/users")).2, "[1,2]", "absolute-form target");
    let (st, _, body) = get(p, "HEAD", "/users");
    assert_eq!((st, body.as_str()), (200, ""));
    assert!(m.hits.load(Ordering::SeqCst) >= 8);
    m.stop();
    assert!(!m.is_running());
    assert!(TcpStream::connect(("127.0.0.1", p)).is_err() || get_fails(p));
    assert!(path_matches("/a/:id/b", "/a/5/b") && path_matches("/a/*", "/a/x") && !path_matches("/a", "/a/b"));
}

fn get_fails(port: u16) -> bool {
    let mut s = match TcpStream::connect(("127.0.0.1", port)) { Ok(s) => s, Err(_) => return true };
    let _ = s.set_read_timeout(Some(std::time::Duration::from_millis(500)));
    let _ = s.write_all(b"GET /users HTTP/1.1\r\n\r\n");
    let mut out = String::new();
    s.read_to_string(&mut out).is_err() || out.is_empty()
}
