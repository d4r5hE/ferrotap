//! G16 local REST API: bound to 127.0.0.1 on a random port, Bearer token required, every endpoint.
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};

use ferrotap::auto_api::{http_call, random_token, ApiCmd, ApiServer, Shared};
use ferrotap::model::SessionRecord;
use serde_json::Value;

fn rec(id: usize, method: &str, url: &str, status: u16) -> SessionRecord {
    SessionRecord {
        id,
        method: method.into(),
        url: url.into(),
        status,
        request_body: format!("req{id}"),
        response_body: format!("resp{id}"),
        start_time: "2026-10-16T10:00:00+00:00".into(),
        duration_ms: 10,
        ..Default::default()
    }
}

fn setup() -> (ApiServer, Arc<Mutex<Shared>>, Receiver<ApiCmd>) {
    let shared = Arc::new(Mutex::new(Shared {
        sessions: vec![
            rec(1, "GET", "https://api.example.com/a", 200),
            rec(2, "POST", "https://api.example.com/b", 404),
            rec(3, "GET", "https://cdn.other.net/c.js", 500),
            rec(4, "GET", "https://api.example.com/d", 201),
        ],
        capturing: true,
        proxy_port: 8897,
        mode: "test".into(),
        ..Default::default()
    }));
    let (tx, rx) = channel();
    let srv = ApiServer::start(0, "secret-token".into(), shared.clone(), tx).unwrap();
    (srv, shared, rx)
}

fn call(srv: &ApiServer, m: &str, p: &str, b: Option<&str>) -> (u16, Value) {
    let (st, body) = http_call(srv.port, &srv.token, m, p, b).unwrap();
    (st, serde_json::from_str(&body).unwrap_or(Value::Null))
}

#[test]
fn token_is_random_and_required() {
    let a = random_token();
    assert_eq!(a.len(), 32);
    assert_ne!(a, random_token());
    let (srv, _sh, _rx) = setup();
    assert_eq!(http_call(srv.port, "", "GET", "/status", None).unwrap().0, 401);
    assert_eq!(http_call(srv.port, "secret-tokeX", "GET", "/status", None).unwrap().0, 401);
    // No Authorization header at all.
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", srv.port)).unwrap();
    s.write_all(b"GET /sessions HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    assert!(out.starts_with("HTTP/1.1 401"), "{out}");
    assert!(!out.contains("resp1"));
}

#[test]
fn binds_localhost_only() {
    let (srv, _sh, _rx) = setup();
    // The listener is on 127.0.0.1; a non-loopback address of this machine must not reach it.
    let addrs: Vec<std::net::IpAddr> = std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|u| u.connect("10.255.255.255:9").map(|_| u))
        .and_then(|u| u.local_addr())
        .map(|a| vec![a.ip()])
        .unwrap_or_default();
    for ip in addrs.into_iter().filter(|ip| !ip.is_loopback() && !ip.is_unspecified()) {
        assert!(std::net::TcpStream::connect_timeout(&(ip, srv.port).into(), std::time::Duration::from_millis(500)).is_err());
    }
}

#[test]
fn status_and_capture() {
    let (srv, sh, rx) = setup();
    let (st, v) = call(&srv, "GET", "/status", None);
    assert_eq!(st, 200);
    assert_eq!(v["sessions"], 4);
    assert_eq!(v["capturing"], true);
    assert_eq!(v["mode"], "test");
    let (st, v) = call(&srv, "POST", "/capture/stop", None);
    assert_eq!((st, v["capturing"].clone()), (200, Value::Bool(false)));
    assert_eq!(rx.try_recv().unwrap(), ApiCmd::StopCapture);
    assert!(!sh.lock().unwrap().capturing);
    assert_eq!(call(&srv, "GET", "/status", None).1["capturing"], false);
    call(&srv, "POST", "/capture/start", None);
    assert_eq!(rx.try_recv().unwrap(), ApiCmd::StartCapture);
    assert!(sh.lock().unwrap().capturing);
    assert_eq!(call(&srv, "GET", "/capture/start", None).0, 404);
}

#[test]
fn sessions_list_filters_and_get() {
    let (srv, _sh, _rx) = setup();
    let (st, v) = call(&srv, "GET", "/sessions", None);
    assert_eq!(st, 200);
    assert_eq!(v["total"], 4);
    assert!(v["sessions"][0].get("response_body").is_none(), "list has no bodies");
    assert_eq!(call(&srv, "GET", "/sessions?host=API.example", None).1["total"], 3);
    assert_eq!(call(&srv, "GET", "/sessions?method=post", None).1["sessions"][0]["id"], 2);
    assert_eq!(call(&srv, "GET", "/sessions?status=5xx", None).1["sessions"][0]["id"], 3);
    assert_eq!(call(&srv, "GET", "/sessions?status=2xx", None).1["total"], 2);
    assert_eq!(call(&srv, "GET", "/sessions?status=404", None).1["total"], 1);
    let (_, v) = call(&srv, "GET", "/sessions?host=example.com&limit=2", None);
    assert_eq!((v["total"].clone(), v["returned"].clone()), (Value::from(3), Value::from(2)));
    let (st, v) = call(&srv, "GET", "/sessions/3", None);
    assert_eq!(st, 200);
    assert_eq!(v["url"], "https://cdn.other.net/c.js");
    assert_eq!(v["request_body"], "req3");
    assert_eq!(v["response_body"], "resp3");
    assert_eq!(call(&srv, "GET", "/sessions/99", None).0, 404);
    assert_eq!(call(&srv, "PUT", "/sessions/3", None).0, 405);
}

#[test]
fn export_and_delete() {
    let (srv, sh, rx) = setup();
    let (st, v) = call(&srv, "POST", "/sessions/export", None);
    assert_eq!(st, 200);
    assert_eq!(v["log"]["entries"].as_array().unwrap().len(), 4);
    let (_, v) = call(&srv, "POST", "/sessions/export", Some(r#"{"ids":[2,4]}"#));
    let entries = v["log"]["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["request"]["method"], "POST");
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("x.har");
    let body = serde_json::json!({"path": p.to_str().unwrap()}).to_string();
    let (st, v) = call(&srv, "POST", "/sessions/export", Some(&body));
    assert_eq!((st, v["written"].clone()), (200, Value::from(4)));
    assert_eq!(ferrotap::har::import_har(&p).unwrap().len(), 4);
    assert_eq!(call(&srv, "POST", "/sessions/export", Some("{bad")).0, 400);

    let (st, v) = call(&srv, "DELETE", "/sessions", None);
    assert_eq!((st, v["cleared"].clone()), (200, Value::from(4)));
    assert_eq!(rx.try_recv().unwrap(), ApiCmd::ClearSessions);
    assert!(sh.lock().unwrap().sessions.is_empty());
    assert_eq!(call(&srv, "GET", "/sessions", None).1["total"], 0);
}

#[test]
fn rules_get_add_replace() {
    let (srv, sh, rx) = setup();
    assert_eq!(call(&srv, "GET", "/rules", None).1["rules"].as_array().unwrap().len(), 0);
    let (st, v) = call(&srv, "POST", "/rules", Some(r#"{"name":"m","pattern":"/api","status":503,"body":"down"}"#));
    assert_eq!((st, v["rules"].clone()), (200, Value::from(1)));
    match rx.try_recv().unwrap() {
        ApiCmd::SetRules(r) => {
            assert_eq!(r.len(), 1);
            assert_eq!(r[0].status, 503);
            assert!(r[0].matches("GET", "https://x.test/api/1"));
        }
        other => panic!("{other:?}"),
    }
    call(&srv, "POST", "/rules", Some(r#"{"pattern":"/two"}"#));
    let (_, v) = call(&srv, "GET", "/rules", None);
    assert_eq!(v["rules"][1]["pattern"], "/two");
    assert_eq!(v["rules"][1]["status"], 200);
    call(&srv, "POST", "/rules", Some(r#"[{"pattern":"/only"}]"#));
    assert_eq!(sh.lock().unwrap().rules.len(), 1);
    assert_eq!(call(&srv, "POST", "/rules", Some("[1,2]")).0, 400);
    assert_eq!(call(&srv, "POST", "/rules", Some("nope")).0, 400);
}

#[test]
fn stop_releases_port() {
    let (mut srv, _sh, _rx) = setup();
    let port = srv.port;
    srv.stop();
    assert!(std::net::TcpListener::bind(("127.0.0.1", port)).is_ok());
}
