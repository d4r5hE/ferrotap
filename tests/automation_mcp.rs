//! G11 MCP server: JSON-RPC 2.0 initialize / tools/list / tools/call against a stub API, plus one
//! end-to-end run of the real `ferrotap_mcp` binary over stdio against a REST API on a random port.
use std::cell::RefCell;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};

use ferrotap::auto_api::{ApiServer, Shared};
use ferrotap::auto_mcp::{handle_line, serve, tools, ApiClient};
use ferrotap::model::SessionRecord;
use serde_json::{json, Value};

/// Records every call and answers with canned JSON.
struct Stub {
    calls: RefCell<Vec<(String, String, Option<String>)>>,
}

impl ApiClient for Stub {
    fn call(&self, method: &str, path: &str, body: Option<&str>) -> Result<(u16, String), String> {
        self.calls.borrow_mut().push((method.into(), path.into(), body.map(String::from)));
        let r = match (method, path.split('?').next().unwrap()) {
            ("GET", "/status") => json!({"capturing": true, "sessions": 2}),
            ("GET", "/sessions") => json!({"total": 7, "returned": 2, "sessions": [{"id": 1}, {"id": 2}]}),
            ("GET", "/sessions/5") => json!({"id": 5, "url": "https://x.test/", "request_body": "q", "response_body": "r", "request_headers": {}}),
            ("GET", p) if p.starts_with("/sessions/") => return Ok((404, json!({"error": "no session"}).to_string())),
            ("DELETE", "/sessions") => json!({"cleared": 2}),
            ("POST", "/capture/start") => json!({"capturing": true}),
            ("POST", "/capture/stop") => json!({"capturing": false}),
            ("POST", "/sessions/export") => json!({"log": {"version": "1.2", "entries": []}}),
            ("GET", "/rules") => json!({"rules": []}),
            ("POST", "/rules") => json!({"rules": 1}),
            _ => return Err("unexpected".into()),
        };
        Ok((200, r.to_string()))
    }
}

fn stub() -> Stub {
    Stub { calls: RefCell::new(Vec::new()) }
}

fn rpc(api: &dyn ApiClient, v: Value) -> Value {
    serde_json::from_str(&handle_line(api, &v.to_string()).expect("a response")).unwrap()
}

fn call_tool(api: &dyn ApiClient, name: &str, args: Value) -> (bool, String) {
    let r = rpc(api, json!({"jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": {"name": name, "arguments": args}}));
    (r["result"]["isError"].as_bool().unwrap(), r["result"]["content"][0]["text"].as_str().unwrap().to_string())
}

#[test]
fn initialize_and_notifications() {
    let s = stub();
    let r = rpc(&s, json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}}));
    assert_eq!(r["jsonrpc"], "2.0");
    assert_eq!(r["id"], 1);
    assert_eq!(r["result"]["protocolVersion"], "2025-03-26");
    assert!(r["result"]["capabilities"]["tools"].is_object());
    assert_eq!(r["result"]["serverInfo"]["name"], "ferrotap");
    assert!(handle_line(&s, r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
    assert_eq!(rpc(&s, json!({"jsonrpc": "2.0", "id": "p", "method": "ping"}))["result"], json!({}));
    assert_eq!(rpc(&s, json!({"jsonrpc": "2.0", "id": 2, "method": "nope"}))["error"]["code"], -32601);
    let bad: Value = serde_json::from_str(&handle_line(&s, "{not json").unwrap()).unwrap();
    assert_eq!(bad["error"]["code"], -32700);
    assert!(handle_line(&s, "   ").is_none());
}

#[test]
fn tools_list_has_schemas() {
    let s = stub();
    let r = rpc(&s, json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list"}));
    let list = r["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = list.iter().map(|t| t["name"].as_str().unwrap()).collect();
    for n in ["status", "list_sessions", "count_sessions", "get_session", "get_bodies", "clear_sessions", "start_capture", "stop_capture", "export_har"] {
        assert!(names.contains(&n), "missing {n}");
    }
    for t in list {
        assert_eq!(t["inputSchema"]["type"], "object");
        assert!(!t["description"].as_str().unwrap().is_empty());
    }
    assert_eq!(list.len(), tools().len());
}

#[test]
fn tools_call_maps_to_api() {
    let s = stub();
    let (err, t) = call_tool(&s, "status", json!({}));
    assert!(!err && t.contains("\"capturing\": true"));
    let (_, _) = call_tool(&s, "list_sessions", json!({"host": "a b", "status": "4xx"}));
    assert_eq!(s.calls.borrow().last().unwrap().1, "/sessions?host=a%20b&status=4xx&limit=50");
    let (_, t) = call_tool(&s, "count_sessions", json!({"method": "GET"}));
    assert_eq!(t, r#"{"count":7}"#);
    assert_eq!(s.calls.borrow().last().unwrap().1, "/sessions?method=GET&limit=0");
    let (err, t) = call_tool(&s, "get_session", json!({"id": 5}));
    assert!(!err && t.contains("https://x.test/"));
    let (err, t) = call_tool(&s, "get_bodies", json!({"id": "5"}));
    assert!(!err);
    let v: Value = serde_json::from_str(&t).unwrap();
    assert_eq!((v["request_body"].as_str(), v["response_body"].as_str()), (Some("q"), Some("r")));
    let (err, t) = call_tool(&s, "get_session", json!({"id": 6}));
    assert!(err && t.contains("404"));
    assert!(call_tool(&s, "get_bodies", json!({})).0, "missing id is an error result");
    assert!(call_tool(&s, "clear_sessions", json!({})).1.contains("cleared"));
    assert!(call_tool(&s, "stop_capture", json!({})).1.contains("false"));
    assert!(call_tool(&s, "start_capture", json!({})).1.contains("true"));
    assert!(call_tool(&s, "export_har", json!({"ids": [1]})).1.contains("1.2"));
    assert_eq!(s.calls.borrow().last().unwrap().2.as_deref(), Some(r#"{"ids":[1]}"#));
    assert!(!call_tool(&s, "add_rule", json!({"pattern": "/x", "status": 418})).0);
    let body: Value = serde_json::from_str(s.calls.borrow().last().unwrap().2.as_deref().unwrap()).unwrap();
    assert_eq!((body["pattern"].as_str(), body["status"].as_u64()), (Some("/x"), Some(418)));
    assert!(!call_tool(&s, "list_rules", json!({})).0);
    let r = rpc(&s, json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "rm_rf"}}));
    assert_eq!(r["error"]["code"], -32602);
}

#[test]
fn serve_reads_lines() {
    let s = stub();
    let input = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n";
    let mut out = Vec::new();
    serve(&s, &input[..], &mut out).unwrap();
    let lines: Vec<&str> = std::str::from_utf8(&out).unwrap().lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(lines[1].contains("\"id\":2"));
}

#[test]
fn mcp_binary_against_real_api() {
    let shared = Arc::new(Mutex::new(Shared {
        sessions: vec![SessionRecord { id: 1, method: "GET".into(), url: "https://m.test/p".into(), status: 200, response_body: "BODY".into(), ..Default::default() }],
        capturing: true,
        ..Default::default()
    }));
    let (tx, _rx) = channel();
    let srv = ApiServer::start(0, "tok123".into(), shared.clone(), tx).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_ferrotap_mcp"))
        .args(["--api-port", &srv.port.to_string(), "--token", "tok123"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let mut stdin = child.stdin.take().unwrap();
        let msgs = [
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18"}}),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "get_bodies", "arguments": {"id": 1}}}),
            json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "stop_capture", "arguments": {}}}),
            json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "clear_sessions", "arguments": {}}}),
        ];
        for m in msgs {
            writeln!(stdin, "{m}").unwrap();
        }
    }
    let out = child.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines.len(), 4, "{text}");
    assert!(lines[1]["result"]["content"][0]["text"].as_str().unwrap().contains("BODY"));
    assert_eq!(lines[1]["result"]["isError"], false);
    let sh = shared.lock().unwrap();
    assert!(!sh.capturing);
    assert!(sh.sessions.is_empty());
}
