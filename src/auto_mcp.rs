//! G11 local MCP server (Model Context Protocol, JSON-RPC 2.0 over stdio, newline-delimited).
//! It is a thin bridge: every tool call becomes a request to the running app's local REST API
//! (`auto_api`). The bin is `ferrotap_mcp`; it reads the port and token from
//! --api-port/--token or env FERROTAP_API_PORT/FERROTAP_API_TOKEN.
use serde_json::{json, Value};

pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// Something that can call the REST API: (method, path, body) -> (status, body).
pub trait ApiClient {
    fn call(&self, method: &str, path: &str, body: Option<&str>) -> Result<(u16, String), String>;
}

/// The real client: HTTP to 127.0.0.1:port with a Bearer token.
pub struct HttpApi {
    pub port: u16,
    pub token: String,
}

impl ApiClient for HttpApi {
    fn call(&self, method: &str, path: &str, body: Option<&str>) -> Result<(u16, String), String> {
        crate::auto_api::http_call(self.port, &self.token, method, path, body)
    }
}

fn tool(name: &str, desc: &str, props: Value, required: &[&str]) -> Value {
    json!({"name": name, "description": desc,
           "inputSchema": {"type": "object", "properties": props, "required": required}})
}

/// The tool list returned by tools/list.
pub fn tools() -> Vec<Value> {
    let filt = json!({
        "host": {"type": "string", "description": "host contains (case-insensitive)"},
        "status": {"type": "string", "description": "exact status (e.g. 404) or class (e.g. 5xx)"},
        "method": {"type": "string", "description": "HTTP method"},
    });
    let mut list_props = filt.clone();
    list_props["limit"] = json!({"type": "integer", "description": "max sessions to return (default 50)"});
    let id = json!({"id": {"type": "integer", "description": "session id"}});
    vec![
        tool("status", "Ferrotap status: capturing on/off, session and rule counts, proxy address.", json!({}), &[]),
        tool("list_sessions", "List captured sessions (id, method, url, status, time, size), optionally filtered.", list_props, &[]),
        tool("count_sessions", "Count captured sessions, optionally filtered.", filt, &[]),
        tool("get_session", "Full details of one session: headers, bodies, timings.", id.clone(), &[]),
        tool("get_bodies", "Request body and response body of one session.", id, &["id"]),
        tool("clear_sessions", "Delete all captured sessions.", json!({}), &[]),
        tool("start_capture", "Start (resume) capturing traffic.", json!({}), &[]),
        tool("stop_capture", "Stop (pause) capturing traffic.", json!({}), &[]),
        tool(
            "export_har",
            "Export sessions as HAR 1.2. With `path` the file is written there; otherwise the HAR JSON is returned.",
            json!({"ids": {"type": "array", "items": {"type": "integer"}, "description": "session ids (default: all)"},
                   "path": {"type": "string", "description": "file path to write (.har)"}}),
            &[],
        ),
        tool("list_rules", "List the Auto Responder rules.", json!({}), &[]),
        tool(
            "add_rule",
            "Add an Auto Responder rule that answers matching requests with a canned response.",
            json!({"pattern": {"type": "string"}, "match_kind": {"type": "string", "enum": ["contains", "regex"]},
                   "method": {"type": "string"}, "status": {"type": "integer"}, "body": {"type": "string"},
                   "name": {"type": "string"}}),
            &["pattern"],
        ),
        tool("clear_rules", "Delete every Auto Responder rule.", json!({}), &[]),
        tool(
            "mark_sessions",
            "Mark sessions with a color (red, blue, green, orange, purple, copper), strikeout, or none to unmark.",
            json!({"ids": {"type": "array", "items": {"type": "integer"}}, "mark": {"type": "string"}}),
            &["ids", "mark"],
        ),
        tool("delete_sessions", "Delete these sessions.", json!({"ids": {"type": "array", "items": {"type": "integer"}}}), &["ids"]),
        tool("select_sessions", "Select these sessions in the Ferrotap window.", json!({"ids": {"type": "array", "items": {"type": "integer"}}}), &["ids"]),
        tool(
            "replay_session",
            "Send a captured request again through the proxy (the replay is captured as a new session).",
            json!({"id": {"type": "integer"}, "count": {"type": "integer", "description": "times (1-50, default 1)"}}),
            &["id"],
        ),
        tool("list_snapshots", "List saved snapshots.", json!({}), &[]),
        tool(
            "save_snapshot",
            "Save sessions (default: all) as a named snapshot.",
            json!({"name": {"type": "string"}, "ids": {"type": "array", "items": {"type": "integer"}}, "folder": {"type": "string"}}),
            &["name"],
        ),
        tool("snapshot_details", "Sessions inside a snapshot (by the name list_snapshots shows).", json!({"name": {"type": "string"}}), &["name"]),
        tool(
            "start_capture_with",
            "Start a browser (fresh profile) or a terminal whose traffic goes through Ferrotap.",
            json!({"what": {"type": "string", "enum": ["browser", "terminal"]}}),
            &["what"],
        ),
        tool("set_system_proxy", "Turn the Windows system proxy (pointing at Ferrotap) on or off.", json!({"on": {"type": "boolean"}}), &["on"]),
        tool("apply_filter", "Filter the Ferrotap grid with quick-filter text (e.g. 'host:api status:500 method:POST'); empty clears it.", json!({"text": {"type": "string"}}), &["text"]),
        tool("get_autosave", "Auto-save settings (on/off, interval, how many files to keep, folder).", json!({}), &[]),
        tool(
            "set_autosave",
            "Change auto-save: turn it on/off and set the interval (minutes), files kept and folder.",
            json!({"enabled": {"type": "boolean"}, "interval_min": {"type": "integer"}, "keep": {"type": "integer"}, "folder": {"type": "string"}}),
            &[],
        ),
        tool("open_https_settings", "Open the HTTPS settings window (trust the Ferrotap CA there).", json!({}), &[]),
        tool(
            "set_sanitize",
            "Mask headers, cookies, query parameters and body patterns (Settings > Privacy) in session details returned to you.",
            json!({"on": {"type": "boolean"}}),
            &["on"],
        ),
    ]
}

fn qs(args: &Value, keys: &[&str]) -> String {
    let parts: Vec<String> = keys
        .iter()
        .filter_map(|k| {
            let v = args.get(*k)?;
            let s = match v {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                _ => return None,
            };
            Some(format!("{k}={}", crate::model::pct_encode(&s)))
        })
        .collect();
    if parts.is_empty() { String::new() } else { format!("?{}", parts.join("&")) }
}

fn id_arg(args: &Value) -> Result<u64, String> {
    args.get("id").and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))).ok_or_else(|| "missing integer argument `id`".to_string())
}

/// Run one tool: Ok(text) or Err(text) (an error result, isError = true).
pub fn call_tool(api: &dyn ApiClient, name: &str, args: &Value) -> Result<String, String> {
    let req = |m: &str, p: &str, b: Option<String>| -> Result<Value, String> {
        let (st, body) = api.call(m, p, b.as_deref())?;
        let v: Value = serde_json::from_str(&body).unwrap_or(Value::String(body));
        if st != 200 {
            return Err(format!("API {m} {p} -> {st}: {v}"));
        }
        Ok(v)
    };
    let pretty = |v: &Value| serde_json::to_string_pretty(v).unwrap_or_default();
    match name {
        "status" => req("GET", "/status", None).map(|v| pretty(&v)),
        "list_sessions" => {
            let mut a = args.clone();
            if a.get("limit").is_none() {
                if let Some(o) = a.as_object_mut() {
                    o.insert("limit".into(), json!(50));
                } else {
                    a = json!({"limit": 50});
                }
            }
            req("GET", &format!("/sessions{}", qs(&a, &["host", "status", "method", "limit"])), None).map(|v| pretty(&v))
        }
        "count_sessions" => {
            let q = qs(args, &["host", "status", "method"]);
            let sep = if q.is_empty() { "?" } else { "&" };
            let v = req("GET", &format!("/sessions{q}{sep}limit=0"), None)?;
            Ok(json!({"count": v["total"]}).to_string())
        }
        "get_session" => req("GET", &format!("/sessions/{}", id_arg(args)?), None).map(|v| pretty(&v)),
        "get_bodies" => {
            let v = req("GET", &format!("/sessions/{}", id_arg(args)?), None)?;
            Ok(pretty(&json!({"id": v["id"], "url": v["url"], "request_body": v["request_body"], "response_body": v["response_body"]})))
        }
        "clear_sessions" => req("DELETE", "/sessions", None).map(|v| pretty(&v)),
        "start_capture" => req("POST", "/capture/start", None).map(|v| pretty(&v)),
        "stop_capture" => req("POST", "/capture/stop", None).map(|v| pretty(&v)),
        "export_har" => {
            let mut b = json!({});
            if let Some(ids) = args.get("ids") {
                b["ids"] = ids.clone();
            }
            if let Some(p) = args.get("path") {
                b["path"] = p.clone();
            }
            req("POST", "/sessions/export", Some(b.to_string())).map(|v| pretty(&v))
        }
        "list_rules" => req("GET", "/rules", None).map(|v| pretty(&v)),
        "add_rule" => {
            let pattern = args.get("pattern").and_then(|p| p.as_str()).filter(|p| !p.is_empty()).ok_or("missing `pattern`")?;
            let rule = json!({
                "name": args.get("name").and_then(|v| v.as_str()).unwrap_or(pattern),
                "enabled": true,
                "match_kind": args.get("match_kind").and_then(|v| v.as_str()).unwrap_or("contains"),
                "pattern": pattern,
                "method": args.get("method").and_then(|v| v.as_str()).unwrap_or(""),
                "action": "canned",
                "status": args.get("status").and_then(|v| v.as_u64()).unwrap_or(200),
                "body": args.get("body").and_then(|v| v.as_str()).unwrap_or(""),
            });
            req("POST", "/rules", Some(rule.to_string())).map(|v| pretty(&v))
        }
        "clear_rules" => req("DELETE", "/rules", None).map(|v| pretty(&v)),
        "mark_sessions" => req("POST", "/sessions/mark", Some(json!({"ids": args.get("ids").cloned().unwrap_or(json!([])), "mark": args.get("mark").cloned().unwrap_or(json!("none"))}).to_string())).map(|v| pretty(&v)),
        "delete_sessions" => req("POST", "/sessions/delete", Some(json!({"ids": args.get("ids").cloned().unwrap_or(json!([]))}).to_string())).map(|v| pretty(&v)),
        "select_sessions" => req("POST", "/sessions/select", Some(json!({"ids": args.get("ids").cloned().unwrap_or(json!([]))}).to_string())).map(|v| pretty(&v)),
        "replay_session" => req("POST", "/sessions/replay", Some(json!({"id": id_arg(args)?, "count": args.get("count").cloned().unwrap_or(json!(1))}).to_string())).map(|v| pretty(&v)),
        "list_snapshots" => req("GET", "/snapshots", None).map(|v| pretty(&v)),
        "save_snapshot" => req("POST", "/snapshots", Some(args.to_string())).map(|v| pretty(&v)),
        "snapshot_details" => {
            let n = args.get("name").and_then(|n| n.as_str()).ok_or("missing `name`")?;
            req("GET", &format!("/snapshots/details?name={}", crate::model::pct_encode(n)), None).map(|v| pretty(&v))
        }
        "start_capture_with" => req("POST", "/launch", Some(json!({"what": args.get("what").cloned().unwrap_or(json!("browser"))}).to_string())).map(|v| pretty(&v)),
        "set_system_proxy" => req("POST", "/system-proxy", Some(json!({"on": args.get("on").cloned().unwrap_or(json!(false))}).to_string())).map(|v| pretty(&v)),
        "open_https_settings" => req("POST", "/https/open", None).map(|v| pretty(&v)),
        "apply_filter" => req("POST", "/filter", Some(json!({"text": args.get("text").cloned().unwrap_or(json!(""))}).to_string())).map(|v| pretty(&v)),
        "get_autosave" => req("GET", "/autosave", None).map(|v| pretty(&v)),
        "set_autosave" => req("POST", "/autosave", Some(args.to_string())).map(|v| pretty(&v)),
        "set_sanitize" => req("POST", "/sanitize", Some(json!({"on": args.get("on").cloned().unwrap_or(json!(true))}).to_string())).map(|v| pretty(&v)),
        other => Err(format!("unknown tool: {other}")),
    }
}

fn rpc_error(id: Value, code: i64, msg: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": msg}})
}

/// Handle one JSON-RPC message (one line). Returns the response line, or None for notifications.
pub fn handle_line(api: &dyn ApiClient, line: &str) -> Option<String> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let msg: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return Some(rpc_error(Value::Null, -32700, &format!("parse error: {e}")).to_string()),
    };
    if msg.is_array() {
        // Batches are not part of current MCP; answer each element.
        let out: Vec<Value> = msg
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|m| handle_line(api, &m.to_string()).and_then(|s| serde_json::from_str(&s).ok()))
            .collect();
        return if out.is_empty() { None } else { Some(Value::Array(out).to_string()) };
    }
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let Some(id) = id else {
        return None; // notification (e.g. notifications/initialized)
    };
    if method.is_empty() {
        return Some(rpc_error(id, -32600, "invalid request").to_string());
    }
    let params = msg.get("params").cloned().unwrap_or(json!({}));
    let result = match method {
        "initialize" => {
            let pv = params.get("protocolVersion").and_then(|v| v.as_str()).unwrap_or(PROTOCOL_VERSION);
            json!({
                "protocolVersion": pv,
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": "ferrotap", "version": env!("CARGO_PKG_VERSION")},
                "instructions": "Tools to inspect and control the Ferrotap debugging proxy (captured HTTP sessions, capture on/off, HAR export, Auto Responder rules)."
            })
        }
        "ping" => json!({}),
        "tools/list" => json!({"tools": tools()}),
        "tools/call" => {
            let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
            if !tools().iter().any(|t| t["name"] == name) {
                return Some(rpc_error(id, -32602, &format!("unknown tool: {name}")).to_string());
            }
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            match call_tool(api, name, &args) {
                Ok(t) => json!({"content": [{"type": "text", "text": t}], "isError": false}),
                Err(e) => json!({"content": [{"type": "text", "text": e}], "isError": true}),
            }
        }
        _ => return Some(rpc_error(id, -32601, &format!("method not found: {method}")).to_string()),
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string())
}

/// Serve MCP over the given reader/writer until EOF.
pub fn serve<R: std::io::BufRead, W: std::io::Write>(api: &dyn ApiClient, input: R, mut output: W) -> std::io::Result<()> {
    for line in input.lines() {
        let line = line?;
        if let Some(resp) = handle_line(api, &line) {
            writeln!(output, "{resp}")?;
            output.flush()?;
        }
    }
    Ok(())
}
