//! G11 / G16 more REST API + MCP tools: mark / delete / select / replay sessions, clear rules,
//! snapshots, launch, system proxy, HTTPS settings, output sanitization.
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};

use ferrotap::auto_api::{http_call, ApiCmd, ApiServer, Shared};
use ferrotap::model::{SessionMark, SessionRecord};
use serde_json::{json, Value};

/// Tests that point FERROTAP_SNAPSHOTS somewhere take turns (it is process-wide).
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn rec(id: usize, url: &str) -> SessionRecord {
    let mut s = SessionRecord { id, method: "GET".into(), url: url.into(), status: 200, ..Default::default() };
    s.request_headers.insert("authorization".into(), "Bearer topsecret".into());
    s
}

fn setup(mode: &str, proxy_port: u16) -> (ApiServer, Arc<Mutex<Shared>>, Receiver<ApiCmd>) {
    let shared = Arc::new(Mutex::new(Shared {
        sessions: vec![rec(1, "https://a.test/1"), rec(2, "https://a.test/2"), rec(3, "https://b.test/3")],
        capturing: true,
        proxy_port,
        mode: mode.into(),
        ..Default::default()
    }));
    let (tx, rx) = channel();
    let srv = ApiServer::start(0, "tok".into(), shared.clone(), tx).unwrap();
    (srv, shared, rx)
}

fn call(srv: &ApiServer, m: &str, p: &str, b: Option<Value>) -> (u16, Value) {
    let body = b.map(|v| v.to_string());
    let (st, out) = http_call(srv.port, &srv.token, m, p, body.as_deref()).unwrap();
    (st, serde_json::from_str(&out).unwrap_or(Value::Null))
}

#[test]
fn mark_delete_select_and_rules() {
    let (srv, sh, rx) = setup("gui", 1);
    let (st, v) = call(&srv, "POST", "/sessions/mark", Some(json!({"ids": [1, 2, 99], "mark": "red"})));
    assert_eq!((st, v["marked"].clone()), (200, json!(2)));
    assert_eq!(rx.try_recv().unwrap(), ApiCmd::Mark(vec![1, 2], Some(SessionMark::Red)));
    assert_eq!(sh.lock().unwrap().sessions[0].mark, Some(SessionMark::Red));
    assert_eq!(call(&srv, "POST", "/sessions/mark", Some(json!({"ids": [1], "mark": "plaid"}))).0, 400);
    call(&srv, "POST", "/sessions/mark", Some(json!({"ids": [1], "mark": "none"})));
    assert_eq!(rx.try_recv().unwrap(), ApiCmd::Mark(vec![1], None));
    assert_eq!(call(&srv, "POST", "/sessions/delete", Some(json!({"ids": [3]}))).1["deleted"], 1);
    assert_eq!(rx.try_recv().unwrap(), ApiCmd::DeleteSessions(vec![3]));
    assert_eq!(sh.lock().unwrap().sessions.len(), 2);
    assert_eq!(call(&srv, "POST", "/sessions/select", Some(json!({"id": 2}))).0, 200);
    assert_eq!(rx.try_recv().unwrap(), ApiCmd::SelectSessions(vec![2]));
    assert_eq!(call(&srv, "POST", "/sessions/delete", Some(json!({"ids": [42]}))).0, 404);
    assert_eq!(call(&srv, "DELETE", "/rules", None).0, 200);
    assert_eq!(rx.try_recv().unwrap(), ApiCmd::SetRules(vec![]));
    assert_eq!(call(&srv, "POST", "/launch", Some(json!({"what": "terminal"}))).0, 200);
    assert_eq!(rx.try_recv().unwrap(), ApiCmd::Launch("terminal".into()));
    assert_eq!(call(&srv, "POST", "/launch", Some(json!({"what": "rm"}))).0, 400);
    assert_eq!(call(&srv, "POST", "/system-proxy", Some(json!({"on": true}))).0, 200);
    assert_eq!(rx.try_recv().unwrap(), ApiCmd::SystemProxy(true));
    assert_eq!(call(&srv, "POST", "/https/open", None).0, 200);
    assert_eq!(rx.try_recv().unwrap(), ApiCmd::OpenHttpsSettings);
}

#[test]
fn headless_refuses_window_actions_and_sanitize_masks() {
    let (srv, _sh, _rx) = setup("headless", 1);
    assert_eq!(call(&srv, "POST", "/launch", Some(json!({"what": "browser"}))).0, 409);
    assert_eq!(call(&srv, "POST", "/system-proxy", Some(json!({"on": true}))).0, 409);
    assert_eq!(call(&srv, "GET", "/sessions/1", None).1["request_headers"]["authorization"], "Bearer topsecret");
    assert_eq!(call(&srv, "POST", "/sanitize", Some(json!({"on": true}))).0, 200);
    let masked = call(&srv, "GET", "/sessions/1", None).1["request_headers"]["authorization"].clone();
    assert_ne!(masked, json!("Bearer topsecret"));
}

#[test]
fn replay_and_snapshots() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // a tiny upstream that the "proxy" port points at directly (composer_send speaks absolute-form HTTP)
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            use std::io::{Read, Write};
            let mut s = s;
            let mut b = [0u8; 2048];
            let _ = s.read(&mut b);
            let _ = s.write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        }
    });
    let (srv, _sh, _rx) = setup("gui", port);
    let (st, v) = call(&srv, "POST", "/sessions/replay", Some(json!({"id": 1, "count": 3})));
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["replayed"], 3);
    assert_eq!(v["results"][0]["status"], 204, "{v}");

    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("FERROTAP_SNAPSHOTS", dir.path());
    let (st, v) = call(&srv, "POST", "/snapshots", Some(json!({"name": "mcp one", "ids": [2]})));
    assert_eq!(st, 200, "{v}");
    let list = call(&srv, "GET", "/snapshots", None).1;
    assert_eq!(list["snapshots"][0]["name"], "mcp one");
    let d = call(&srv, "GET", "/snapshots/details?name=mcp%20one", None).1;
    assert_eq!(d["sessions"][0]["url"], "https://a.test/2");
    assert_eq!(call(&srv, "GET", "/snapshots/details?name=nope", None).0, 404);
    assert_eq!(call(&srv, "POST", "/snapshots", Some(json!({}))).0, 400);
}

#[test]
fn mcp_lists_and_dispatches_new_tools() {
    let names: Vec<String> = ferrotap::auto_mcp::tools().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
    for want in ["mark_sessions", "delete_sessions", "select_sessions", "replay_session", "clear_rules", "list_snapshots", "save_snapshot", "snapshot_details", "start_capture_with", "set_system_proxy", "open_https_settings", "set_sanitize"] {
        assert!(names.contains(&want.to_string()), "{want}");
    }
    struct Rec(Mutex<Vec<(String, String, Option<String>)>>);
    impl ferrotap::auto_mcp::ApiClient for Rec {
        fn call(&self, m: &str, p: &str, b: Option<&str>) -> Result<(u16, String), String> {
            self.0.lock().unwrap().push((m.into(), p.into(), b.map(str::to_string)));
            Ok((200, "{}".into()))
        }
    }
    let api = Rec(Mutex::new(Vec::new()));
    ferrotap::auto_mcp::call_tool(&api, "mark_sessions", &json!({"ids": [4], "mark": "blue"})).unwrap();
    ferrotap::auto_mcp::call_tool(&api, "snapshot_details", &json!({"name": "a b"})).unwrap();
    ferrotap::auto_mcp::call_tool(&api, "replay_session", &json!({"id": 7})).unwrap();
    assert!(ferrotap::auto_mcp::call_tool(&api, "replay_session", &json!({})).is_err());
    let calls = api.0.lock().unwrap();
    assert_eq!(calls[0].1, "/sessions/mark");
    assert!(calls[0].2.as_deref().unwrap().contains("\"blue\""));
    assert_eq!(calls[1].1, "/snapshots/details?name=a%20b");
    assert!(calls[2].2.as_deref().unwrap().contains("\"id\":7"));
}

#[test]
fn filter_and_autosave() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("FERROTAP_SNAPSHOTS", dir.path());
    let (srv, _sh, rx) = setup("gui", 1);
    assert_eq!(call(&srv, "POST", "/filter", Some(json!({"text": "status:500"}))).0, 200);
    assert_eq!(rx.try_recv().unwrap(), ApiCmd::SetFilter("status:500".into()));
    assert_eq!(call(&srv, "GET", "/autosave", None).1["enabled"], false);
    let (st, v) = call(&srv, "POST", "/autosave", Some(json!({"enabled": true, "interval_min": 0, "keep": 3})));
    assert_eq!((st, v["enabled"].clone(), v["interval_min"].clone(), v["keep"].clone()), (200, json!(true), json!(1), json!(3)));
    assert_eq!(rx.try_recv().unwrap(), ApiCmd::ReloadAutosave);
    assert!(ferrotap::data_io::AutoSaveCfg::load(dir.path()).enabled);
    let names: Vec<String> = ferrotap::auto_mcp::tools().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
    assert!(names.iter().any(|n| n == "apply_filter") && names.iter().any(|n| n == "set_autosave"));
}
