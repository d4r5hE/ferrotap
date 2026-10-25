//! G16 local REST API: a tiny HTTP/1.1 server on 127.0.0.1 only, protected by a random Bearer token.
//! It works on a shared snapshot (`Shared`) and also sends an `ApiCmd` for every change so the owner
//! (the egui app or the headless loop) can apply it to its own state.
//!
//! Endpoints (all need `Authorization: Bearer <token>`):
//!   GET /status, POST /capture/start, POST /capture/stop,
//!   GET /sessions?host=&status=&method=&limit=, GET /sessions/{id}, DELETE /sessions,
//!   POST /sessions/export (body: {"ids":[..]?, "path":"x.har"?}) -> HAR JSON,
//!   GET /rules, POST /rules (one rule object = append, an array = replace).
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use crate::model::{Rule, SessionRecord};

/// What the API asks the owner of the real state to do.
#[derive(Debug, Clone, PartialEq)]
pub enum ApiCmd {
    StartCapture,
    StopCapture,
    ClearSessions,
    SetRules(Vec<Rule>),
    /// Mark (or unmark with None) these sessions.
    Mark(Vec<usize>, Option<crate::model::SessionMark>),
    DeleteSessions(Vec<usize>),
    SelectSessions(Vec<usize>),
    /// "browser" or "terminal": start one with capture on.
    Launch(String),
    SystemProxy(bool),
    OpenHttpsSettings,
    /// Set the grid's quick filter text ("" clears it).
    SetFilter(String),
    /// The auto-save settings file changed: reload it.
    ReloadAutosave,
}

/// The state the API reads (and updates optimistically).
#[derive(Debug, Clone, Default)]
pub struct Shared {
    pub sessions: Vec<SessionRecord>,
    pub capturing: bool,
    pub rules: Vec<Rule>,
    /// The capture proxy's port (informational, shown by /status).
    pub proxy_port: u16,
    /// "gui" or "headless".
    pub mode: String,
    /// Mask session details returned by the API with the privacy settings.
    pub sanitize: bool,
}

pub type SharedState = Arc<Mutex<Shared>>;

/// A random 32-hex-character token (seeded from the OS-randomised std hasher, time and pid).
pub fn random_token() -> String {
    use sha2::{Digest, Sha256};
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let mut h = Sha256::new();
    for i in 0..4u64 {
        let mut x = RandomState::new().build_hasher();
        x.write_u64(i);
        h.update(x.finish().to_le_bytes());
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    h.update(nanos.to_le_bytes());
    h.update(std::process::id().to_le_bytes());
    let d = h.finalize();
    d[..16].iter().map(|b| format!("{b:02x}")).collect()
}

/// A running API server; dropping it (or calling `stop`) shuts the listener down.
pub struct ApiServer {
    pub port: u16,
    pub token: String,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ApiServer {
    /// Bind 127.0.0.1:`port` (0 = any free port) and serve in a background thread.
    pub fn start(port: u16, token: String, shared: SharedState, cmds: Sender<ApiCmd>) -> Result<ApiServer, String> {
        let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], port))).map_err(|e| format!("bind 127.0.0.1:{port}: {e}"))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let stop = Arc::new(AtomicBool::new(false));
        let st = stop.clone();
        let tok = token.clone();
        let cmds = Mutex::new(cmds);
        let cmds = Arc::new(cmds);
        let thread = std::thread::spawn(move || {
            while !st.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let (shared, tok, cmds) = (shared.clone(), tok.clone(), cmds.clone());
                        std::thread::spawn(move || {
                            let tx = cmds.lock().map(|t| t.clone());
                            if let Ok(tx) = tx {
                                handle_conn(stream, &tok, &shared, &tx);
                            }
                        });
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(15)),
                    Err(_) => std::thread::sleep(Duration::from_millis(50)),
                }
            }
        });
        Ok(ApiServer { port, token, stop, thread: Some(thread) })
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for ApiServer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// A parsed HTTP request.
#[derive(Debug, Default)]
pub struct HttpReq {
    pub method: String,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpReq {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
    pub fn q(&self, name: &str) -> Option<&str> {
        self.query.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str()).filter(|v| !v.is_empty())
    }
}

const MAX_BODY: usize = 64 * 1024 * 1024;

fn read_request(stream: &mut TcpStream) -> Result<HttpReq, String> {
    let mut r = BufReader::new(stream);
    let mut line = String::new();
    r.read_line(&mut line).map_err(|e| e.to_string())?;
    let mut parts = line.split_whitespace();
    let method = parts.next().ok_or("empty request")?.to_uppercase();
    let target = parts.next().ok_or("no target")?.to_string();
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        let n = r.read_line(&mut h).map_err(|e| e.to_string())?;
        if n == 0 || h.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
        if headers.len() > 200 {
            return Err("too many headers".into());
        }
    }
    let len: usize = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-length")).and_then(|(_, v)| v.parse().ok()).unwrap_or(0);
    if len > MAX_BODY {
        return Err("body too large".into());
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).map_err(|e| e.to_string())?;
    let (path, q) = target.split_once('?').unwrap_or((target.as_str(), ""));
    let query = q
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (crate::model::pct_decode(k), crate::model::pct_decode(v))
        })
        .collect();
    Ok(HttpReq { method, path: path.trim_end_matches('/').to_string(), query, headers, body })
}

fn handle_conn(mut stream: TcpStream, token: &str, shared: &SharedState, cmds: &Sender<ApiCmd>) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let (status, body) = match read_request(&mut stream) {
        Ok(req) => route(&req, token, shared, cmds),
        Err(e) => (400, json!({"error": e})),
    };
    let text = serde_json::to_string_pretty(&body).unwrap_or_default();
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n",
        text.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(text.as_bytes());
    let _ = stream.flush();
}

/// Constant-time-ish token check.
fn token_ok(req: &HttpReq, token: &str) -> bool {
    let Some(v) = req.header("authorization") else { return false };
    let Some(given) = v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer ")) else { return false };
    let (a, b) = (given.trim().as_bytes(), token.as_bytes());
    if a.len() != b.len() || token.is_empty() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// One-line summary object of a session (no bodies).
pub fn session_summary(s: &SessionRecord) -> Value {
    json!({
        "id": s.id, "method": s.method, "url": s.url, "host": s.host(), "status": s.status,
        "duration_ms": s.duration_ms as u64, "response_size": s.response_body.len(),
        "content_type": s.content_type(), "start_time": s.start_time,
    })
}

/// Filter sessions the way GET /sessions does. `status` is an exact code or a class like "4xx".
pub fn filter_sessions<'a>(sessions: &'a [SessionRecord], host: Option<&str>, status: Option<&str>, method: Option<&str>) -> Vec<&'a SessionRecord> {
    sessions
        .iter()
        .filter(|s| host.is_none_or(|h| s.host().to_lowercase().contains(&h.to_lowercase())))
        .filter(|s| method.is_none_or(|m| s.method.eq_ignore_ascii_case(m)))
        .filter(|s| {
            status.is_none_or(|st| {
                let st = st.trim().to_lowercase();
                if st.len() == 3 && st.ends_with("xx") {
                    st.chars().next().and_then(|c| c.to_digit(10)).is_some_and(|d| s.status / 100 == d as u16)
                } else {
                    st.parse::<u16>() == Ok(s.status)
                }
            })
        })
        .collect()
}

/// Route one request. Public so it can be unit-tested without a socket.
pub fn route(req: &HttpReq, token: &str, shared: &SharedState, cmds: &Sender<ApiCmd>) -> (u16, Value) {
    if !token_ok(req, token) {
        return (401, json!({"error": "missing or wrong bearer token"}));
    }
    let mut sh = match shared.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    let m = req.method.as_str();
    let path = req.path.as_str();
    match (m, path) {
        ("GET", "/status") => (
            200,
            json!({
                "app": "ferrotap", "version": env!("CARGO_PKG_VERSION"), "mode": sh.mode,
                "capturing": sh.capturing, "sessions": sh.sessions.len(), "rules": sh.rules.len(),
                "proxy": format!("127.0.0.1:{}", sh.proxy_port),
            }),
        ),
        ("POST", "/capture/start") | ("POST", "/capture/stop") => {
            let on = path.ends_with("start");
            let _ = cmds.send(if on { ApiCmd::StartCapture } else { ApiCmd::StopCapture });
            sh.capturing = on;
            (200, json!({"capturing": on}))
        }
        ("GET", "/sessions") => {
            let list = filter_sessions(&sh.sessions, req.q("host"), req.q("status"), req.q("method"));
            let total = list.len();
            let limit = req.q("limit").and_then(|l| l.parse::<usize>().ok()).unwrap_or(usize::MAX);
            let items: Vec<Value> = list.into_iter().take(limit).map(session_summary).collect();
            (200, json!({"total": total, "returned": items.len(), "sessions": items}))
        }
        ("DELETE", "/sessions") => {
            let n = sh.sessions.len();
            let _ = cmds.send(ApiCmd::ClearSessions);
            sh.sessions.clear();
            (200, json!({"cleared": n}))
        }
        ("POST", "/sessions/export") => {
            let opts: Value = if req.body.is_empty() { json!({}) } else {
                match serde_json::from_slice(&req.body) {
                    Ok(v) => v,
                    Err(e) => return (400, json!({"error": format!("bad JSON: {e}")})),
                }
            };
            let ids: Option<std::collections::HashSet<usize>> = opts.get("ids").and_then(|v| serde_json::from_value(v.clone()).ok());
            let chosen: Vec<SessionRecord> = sh.sessions.iter().filter(|s| ids.as_ref().is_none_or(|ids| ids.contains(&s.id))).cloned().collect();
            if let Some(p) = opts.get("path").and_then(|p| p.as_str()).filter(|p| !p.is_empty()) {
                return match crate::har::export_har(std::path::Path::new(p), &chosen) {
                    Ok(n) => (200, json!({"written": n, "path": p})),
                    Err(e) => (500, json!({"error": e})),
                };
            }
            (200, crate::har::to_har(&chosen))
        }
        ("GET", "/rules") => (200, json!({"rules": sh.rules})),
        ("DELETE", "/rules") => {
            let n = sh.rules.len();
            let _ = cmds.send(ApiCmd::SetRules(Vec::new()));
            sh.rules.clear();
            (200, json!({"cleared": n}))
        }
        ("POST", "/sessions/mark") | ("POST", "/sessions/delete") | ("POST", "/sessions/select") | ("POST", "/sessions/replay") => {
            let v: Value = match serde_json::from_slice(if req.body.is_empty() { b"{}" } else { &req.body }) {
                Ok(v) => v,
                Err(e) => return (400, json!({"error": format!("bad JSON: {e}")})),
            };
            let mut ids: Vec<usize> = v.get("ids").and_then(|x| serde_json::from_value(x.clone()).ok()).unwrap_or_default();
            if let Some(id) = v.get("id").and_then(|x| x.as_u64()) {
                ids.push(id as usize);
            }
            let known: Vec<usize> = ids.iter().copied().filter(|i| sh.sessions.iter().any(|s| s.id == *i)).collect();
            if known.is_empty() {
                return (404, json!({"error": "no such session ids"}));
            }
            match path {
                "/sessions/mark" => {
                    let name = v.get("mark").and_then(|m| m.as_str()).unwrap_or("");
                    let mark = crate::model::SessionMark::parse(name);
                    if mark.is_none() && !name.is_empty() && name != "none" {
                        return (400, json!({"error": "mark is red, blue, green, orange, purple, copper, strikeout or none"}));
                    }
                    for s in sh.sessions.iter_mut().filter(|s| known.contains(&s.id)) {
                        s.mark = mark;
                    }
                    let _ = cmds.send(ApiCmd::Mark(known.clone(), mark));
                    (200, json!({"marked": known.len()}))
                }
                "/sessions/delete" => {
                    let gone: std::collections::HashSet<usize> = known.iter().copied().collect();
                    sh.sessions.retain(|s| !gone.contains(&s.id));
                    let _ = cmds.send(ApiCmd::DeleteSessions(known.clone()));
                    (200, json!({"deleted": known.len()}))
                }
                "/sessions/select" => {
                    let _ = cmds.send(ApiCmd::SelectSessions(known.clone()));
                    (200, json!({"selected": known.len()}))
                }
                _ => {
                    // Replay through the capture proxy; the replays show up as new sessions.
                    let count = v.get("count").and_then(|c| c.as_u64()).unwrap_or(1).clamp(1, 50) as usize;
                    let chosen: Vec<SessionRecord> = sh.sessions.iter().filter(|s| known.contains(&s.id)).cloned().collect();
                    let proxy = format!("127.0.0.1:{}", sh.proxy_port);
                    drop(sh);
                    let opt = crate::g1_replay::ReplayOptions { count, ..Default::default() };
                    let res = crate::g1_replay::replay_sessions(&chosen, &proxy, opt);
                    let out: Vec<Value> = res.iter().map(|r| json!({"status": r.status, "ms": r.elapsed_ms as u64, "bytes": r.body.len(), "error": r.error})).collect();
                    (200, json!({"replayed": out.len(), "results": out}))
                }
            }
        }
        ("GET", "/snapshots") => {
            let list: Vec<Value> = crate::data_io::list_snapshots(&crate::data_io::snapshots_root())
                .iter()
                .map(|s| json!({"name": s.label(), "bytes": s.bytes, "modified": s.modified, "locked": s.locked}))
                .collect();
            (200, json!({"snapshots": list}))
        }
        ("POST", "/snapshots") => {
            let v: Value = serde_json::from_slice(if req.body.is_empty() { b"{}" } else { &req.body }).unwrap_or(json!({}));
            let name = v.get("name").and_then(|n| n.as_str()).unwrap_or("").trim().to_string();
            if name.is_empty() {
                return (400, json!({"error": "missing `name`"}));
            }
            let ids: Option<std::collections::HashSet<usize>> = v.get("ids").and_then(|x| serde_json::from_value(x.clone()).ok());
            let chosen: Vec<SessionRecord> = sh.sessions.iter().filter(|s| ids.as_ref().is_none_or(|ids| ids.contains(&s.id))).cloned().collect();
            let folder = v.get("folder").and_then(|f| f.as_str()).unwrap_or("");
            match crate::data_io::save_snapshot(&crate::data_io::snapshots_root(), folder, &name, &chosen) {
                Ok(p) => (200, json!({"saved": chosen.len(), "path": p.display().to_string()})),
                Err(e) => (400, json!({"error": e})),
            }
        }
        ("GET", "/snapshots/details") => {
            let want = req.q("name").unwrap_or("").to_string();
            let found = crate::data_io::list_snapshots(&crate::data_io::snapshots_root()).into_iter().find(|s| s.label() == want);
            match found {
                None => (404, json!({"error": format!("no snapshot {want}")})),
                Some(s) if s.locked => (403, json!({"error": "this snapshot is password protected"})),
                Some(s) => match crate::data_io::load_snapshot(&s.path) {
                    Ok(list) => (200, json!({"name": want, "sessions": list.iter().map(session_summary).collect::<Vec<_>>()})),
                    Err(e) => (500, json!({"error": e})),
                },
            }
        }
        ("POST", "/launch") => {
            let v: Value = serde_json::from_slice(if req.body.is_empty() { b"{}" } else { &req.body }).unwrap_or(json!({}));
            let what = v.get("what").and_then(|w| w.as_str()).unwrap_or("browser");
            if !matches!(what, "browser" | "terminal") {
                return (400, json!({"error": "what is browser or terminal"}));
            }
            if sh.mode != "gui" {
                return (409, json!({"error": "only the app with a window can start programs"}));
            }
            let _ = cmds.send(ApiCmd::Launch(what.to_string()));
            (200, json!({"starting": what}))
        }
        ("POST", "/system-proxy") => {
            let v: Value = serde_json::from_slice(if req.body.is_empty() { b"{}" } else { &req.body }).unwrap_or(json!({}));
            let Some(on) = v.get("on").and_then(|o| o.as_bool()) else { return (400, json!({"error": "missing boolean `on`"})) };
            if sh.mode != "gui" {
                return (409, json!({"error": "the system proxy is switched by the app with a window"}));
            }
            let _ = cmds.send(ApiCmd::SystemProxy(on));
            (200, json!({"system_proxy": on}))
        }
        ("POST", "/filter") => {
            let v: Value = serde_json::from_slice(if req.body.is_empty() { b"{}" } else { &req.body }).unwrap_or(json!({}));
            let text = v.get("text").and_then(|t| t.as_str()).unwrap_or("").to_string();
            let _ = cmds.send(ApiCmd::SetFilter(text.clone()));
            (200, json!({"filter": text}))
        }
        ("GET", "/autosave") => {
            let root = crate::data_io::snapshots_root();
            let cfg = crate::data_io::AutoSaveCfg::load(&root);
            (200, json!({"enabled": cfg.enabled, "interval_min": cfg.interval_min, "keep": cfg.keep, "folder": cfg.dir(&root).display().to_string()}))
        }
        ("POST", "/autosave") => {
            let v: Value = serde_json::from_slice(if req.body.is_empty() { b"{}" } else { &req.body }).unwrap_or(json!({}));
            let root = crate::data_io::snapshots_root();
            let mut cfg = crate::data_io::AutoSaveCfg::load(&root);
            if let Some(b) = v.get("enabled").and_then(|x| x.as_bool()) {
                cfg.enabled = b;
            }
            if let Some(n) = v.get("interval_min").and_then(|x| x.as_u64()) {
                cfg.interval_min = n.clamp(1, 24 * 60) as u32;
            }
            if let Some(n) = v.get("keep").and_then(|x| x.as_u64()) {
                cfg.keep = n.clamp(1, 1000) as usize;
            }
            if let Some(f) = v.get("folder").and_then(|x| x.as_str()) {
                cfg.folder = f.to_string();
            }
            if let Err(e) = cfg.save(&root) {
                return (500, json!({"error": e}));
            }
            let _ = cmds.send(ApiCmd::ReloadAutosave);
            (200, json!({"enabled": cfg.enabled, "interval_min": cfg.interval_min, "keep": cfg.keep}))
        }
        ("POST", "/https/open") => {
            let _ = cmds.send(ApiCmd::OpenHttpsSettings);
            (200, json!({"opened": true}))
        }
        ("POST", "/sanitize") => {
            let v: Value = serde_json::from_slice(if req.body.is_empty() { b"{}" } else { &req.body }).unwrap_or(json!({}));
            let Some(on) = v.get("on").and_then(|o| o.as_bool()) else { return (400, json!({"error": "missing boolean `on`"})) };
            sh.sanitize = on;
            (200, json!({"sanitize": on}))
        }
        ("POST", "/rules") => {
            let v: Value = match serde_json::from_slice(&req.body) {
                Ok(v) => v,
                Err(e) => return (400, json!({"error": format!("bad JSON: {e}")})),
            };
            let new_rules = if v.is_array() {
                match serde_json::from_value::<Vec<Rule>>(v) {
                    Ok(r) => r,
                    Err(e) => return (400, json!({"error": format!("bad rules: {e}")})),
                }
            } else {
                match serde_json::from_value::<Rule>(v) {
                    Ok(r) => {
                        let mut all = sh.rules.clone();
                        all.push(r);
                        all
                    }
                    Err(e) => return (400, json!({"error": format!("bad rule: {e}")})),
                }
            };
            let _ = cmds.send(ApiCmd::SetRules(new_rules.clone()));
            sh.rules = new_rules;
            (200, json!({"rules": sh.rules.len()}))
        }
        _ => {
            if let Some(id) = path.strip_prefix("/sessions/").and_then(|s| s.parse::<usize>().ok()) {
                if m != "GET" {
                    return (405, json!({"error": "use GET"}));
                }
                return match sh.sessions.iter().find(|s| s.id == id) {
                    Some(s) if sh.sanitize => (200, serde_json::to_value(crate::qol::sanitize(s, &crate::qol::privacy_settings())).unwrap_or(Value::Null)),
                    Some(s) => (200, serde_json::to_value(s).unwrap_or(Value::Null)),
                    None => (404, json!({"error": format!("no session {id}")})),
                };
            }
            (404, json!({"error": format!("no route {m} {path}")}))
        }
    }
}

/// Minimal blocking HTTP client for the API (used by the MCP bridge and tests).
pub fn http_call(port: u16, token: &str, method: &str, path: &str, body: Option<&str>) -> Result<(u16, String), String> {
    let mut s = TcpStream::connect(SocketAddr::from(([127, 0, 0, 1], port))).map_err(|e| format!("connect 127.0.0.1:{port}: {e}"))?;
    let _ = s.set_read_timeout(Some(Duration::from_secs(30)));
    let b = body.unwrap_or("");
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}",
        b.len()
    );
    s.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&raw).to_string();
    let (head, body) = text.split_once("\r\n\r\n").ok_or("bad response")?;
    let status = head.split_whitespace().nth(1).and_then(|c| c.parse().ok()).ok_or("bad status line")?;
    Ok((status, body.to_string()))
}
