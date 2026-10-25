//! G15 API development tools: environments/variables, response tests, collection runner, code generation, OpenAPI/Postman import-export, GraphQL view, local mock server.
//! Owned by one builder; the app calls `api_tools_menu` from the toolbar and `api_tools_windows` every frame.
//! The Composer (src/ui.rs) calls into this module for variable substitution, tests, tabs, settings and the Code / GraphQL views.
use crate::model::{Collection, ComposerRequest, ComposerResponse, KvRow, SavedRequest, SessionRecord};
use crate::ui::GridApp;
use egui::{Color32, RichText};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

fn yes() -> bool { true }

// ===================== Environments and variables =====================

/// A named set of variables (`{{name}}` in requests is replaced by the value).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Environment {
    pub name: String,
    #[serde(default)]
    pub vars: Vec<KvRow>,
}

/// All environments plus the active one, saved as `_environments.json` in the collections folder.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct EnvStore {
    #[serde(default)]
    pub envs: Vec<Environment>,
    /// Name of the active environment ("" = none).
    #[serde(default)]
    pub active: String,
}

impl EnvStore {
    /// Enabled variables of the active environment.
    pub fn active_vars(&self) -> Vec<(String, String)> {
        self.envs.iter().find(|e| e.name == self.active).map(env_vars).unwrap_or_default()
    }
    pub fn get_mut(&mut self, name: &str) -> Option<&mut Environment> { self.envs.iter_mut().find(|e| e.name == name) }
    /// Add (or replace) an environment.
    pub fn upsert(&mut self, e: Environment) {
        if let Some(x) = self.get_mut(&e.name) { *x = e; } else { self.envs.push(e); }
    }
}

pub fn env_vars(e: &Environment) -> Vec<(String, String)> {
    e.vars.iter().filter(|r| r.enabled && !r.key.trim().is_empty()).map(|r| (r.key.trim().to_string(), r.value.clone())).collect()
}

pub fn envs_file(dir: &Path) -> PathBuf { dir.join("_environments.json") }

pub fn load_envs(dir: &Path) -> EnvStore {
    std::fs::read_to_string(envs_file(dir)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn save_envs(dir: &Path, s: &EnvStore) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let t = serde_json::to_string_pretty(s).map_err(|e| e.to_string())?;
    std::fs::write(envs_file(dir), t).map_err(|e| e.to_string())
}

/// Replace `{{name}}` (spaces inside the braces allowed) with variable values. Unknown names are left as they are.
/// Values may themselves contain variables (resolved up to 5 levels deep).
pub fn substitute(text: &str, vars: &[(String, String)]) -> String {
    let mut cur = text.to_string();
    for _ in 0..5 {
        if !cur.contains("{{") { break; }
        let mut out = String::with_capacity(cur.len());
        let mut rest = cur.as_str();
        let mut changed = false;
        while let Some(i) = rest.find("{{") {
            out.push_str(&rest[..i]);
            let after = &rest[i + 2..];
            match after.find("}}") {
                Some(j) => {
                    let name = after[..j].trim();
                    match vars.iter().rev().find(|(k, _)| k == name) {
                        Some((_, v)) => { out.push_str(v); changed = true; }
                        None => { out.push_str("{{"); out.push_str(&after[..j]); out.push_str("}}"); }
                    }
                    rest = &after[j + 2..];
                }
                None => { out.push_str(&rest[i..]); rest = ""; }
            }
        }
        out.push_str(rest);
        cur = out;
        if !changed { break; }
    }
    cur
}

/// Names of `{{variables}}` used in a text.
pub fn variables_in(text: &str) -> Vec<String> {
    let mut v = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("{{") {
        let after = &rest[i + 2..];
        let Some(j) = after.find("}}") else { break };
        let n = after[..j].trim().to_string();
        if !n.is_empty() && !v.contains(&n) { v.push(n); }
        rest = &after[j + 2..];
    }
    v
}

/// The request with variables substituted in URL, params, headers and body.
pub fn apply_env(r: &SavedRequest, vars: &[(String, String)]) -> SavedRequest {
    let mut o = r.clone();
    o.url = substitute(&r.url, vars);
    for row in o.params.iter_mut().chain(o.headers.iter_mut()) {
        row.key = substitute(&row.key, vars);
        row.value = substitute(&row.value, vars);
    }
    o.body = substitute(&r.body, vars);
    o
}

/// The request that is sent: variables substituted, params appended, JSON content type for GraphQL bodies.
pub fn prepare(r: &SavedRequest, vars: &[(String, String)]) -> ComposerRequest {
    let mut c = apply_env(r, vars).to_composer();
    if r.body_mode == "graphql" && !c.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type")) {
        c.headers.push(("Content-Type".into(), "application/json".into()));
    }
    c
}

// ===================== Saved responses and Composer settings =====================

/// A response saved with a request (served by the mock server, exported as an example).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct SavedResponse {
    pub status: u16,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    #[serde(default)]
    pub body: String,
}

impl From<&ComposerResponse> for SavedResponse {
    fn from(r: &ComposerResponse) -> Self {
        let headers = r.headers.iter().filter(|(k, _)| !is_framing_header(k)).cloned().collect();
        SavedResponse { status: r.status, headers, body: r.body.clone() }
    }
}

fn is_framing_header(k: &str) -> bool {
    let k = k.to_ascii_lowercase();
    matches!(k.as_str(), "content-length" | "transfer-encoding" | "connection" | "keep-alive" | "proxy-connection" | "host" | "content-encoding")
        || k.starts_with("proxy-")
}

/// Composer settings (saved as `_composer_settings.json`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ComposerSettings {
    #[serde(default = "yes")]
    pub follow_redirects: bool,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// Send through the capture proxy (so the request shows in the session list) instead of directly.
    #[serde(default = "yes")]
    pub via_proxy: bool,
    /// Send HTTP/1.0 instead of HTTP/1.1 (the proxy still negotiates HTTP/2 with the server when it can).
    #[serde(default)]
    pub http10: bool,
}

fn default_timeout() -> u64 { 30 }

impl Default for ComposerSettings {
    fn default() -> Self { ComposerSettings { follow_redirects: true, timeout_secs: 30, via_proxy: true, http10: false } }
}

pub fn load_settings(dir: &Path) -> ComposerSettings {
    std::fs::read_to_string(dir.join("_composer_settings.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn save_settings(dir: &Path, s: &ComposerSettings) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("_composer_settings.json"), serde_json::to_string_pretty(s).unwrap_or_default()).map_err(|e| e.to_string())
}

/// host:port of an http:// URL (port 80 by default).
pub fn url_authority(url: &str) -> Option<String> {
    let rest = url.trim().strip_prefix("http://")?;
    let a = rest.split(['/', '?', '#']).next().unwrap_or("");
    if a.is_empty() { return None; }
    Some(if a.rsplit_once(':').map(|(_, p)| p.parse::<u16>().is_ok()).unwrap_or(false) { a.to_string() } else { format!("{a}:80") })
}

/// Resolve a Location header against the URL it came from.
pub fn resolve_location(base: &str, loc: &str) -> String {
    let loc = loc.trim();
    if loc.starts_with("http://") || loc.starts_with("https://") { return loc.to_string(); }
    let (scheme, rest) = base.split_once("://").unwrap_or(("http", base));
    let auth = rest.split(['/', '?', '#']).next().unwrap_or("");
    if let Some(r) = loc.strip_prefix("//") { return format!("{scheme}://{r}"); }
    if loc.starts_with('/') { return format!("{scheme}://{auth}{loc}"); }
    let path = rest[auth.len()..].split(['?', '#']).next().unwrap_or("");
    let dir = match path.rfind('/') { Some(i) => &path[..=i], None => "/" };
    format!("{scheme}://{auth}{dir}{loc}")
}

/// Send a request with the Composer settings: via `proxy` (host:port) or directly, following redirects if asked.
pub fn send_request(req: &ComposerRequest, settings: &ComposerSettings, proxy: &str) -> ComposerResponse {
    let t0 = std::time::Instant::now();
    let mut cur = req.clone();
    let mut hops = 0;
    loop {
        let addr = if settings.via_proxy {
            proxy.to_string()
        } else {
            let Some(a) = url_authority(&cur.url) else {
                return ComposerResponse { error: "only http:// URLs can be sent directly".into(), ..Default::default() };
            };
            use std::net::ToSocketAddrs;
            match a.to_socket_addrs().ok().and_then(|mut it| it.next()) {
                Some(sa) => sa.to_string(),
                None => return ComposerResponse { error: format!("cannot resolve {a}"), ..Default::default() },
            }
        };
        let mut r = crate::model::composer_send_opts(&cur, &addr, settings.timeout_secs, settings.http10);
        let loc = r.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("location")).map(|(_, v)| v.clone());
        if settings.follow_redirects && r.error.is_empty() && matches!(r.status, 301 | 302 | 303 | 307 | 308) && hops < 10 {
            if let Some(loc) = loc {
                let next = resolve_location(&cur.url, &loc);
                if !(r.status == 307 || r.status == 308) && cur.method.to_uppercase() != "HEAD" {
                    cur.method = "GET".into();
                    cur.body.clear();
                    cur.headers.retain(|(k, _)| !k.eq_ignore_ascii_case("content-type") && !k.eq_ignore_ascii_case("content-length"));
                }
                cur.headers.retain(|(k, _)| !k.eq_ignore_ascii_case("host"));
                cur.url = next;
                hops += 1;
                continue;
            }
        }
        r.elapsed_ms = t0.elapsed().as_millis();
        return r;
    }
}

// ===================== Tests / assertions =====================

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AssertKind {
    #[default]
    StatusEquals,
    HeaderExists,
    HeaderEquals,
    JsonPathEquals,
    JsonPathExists,
    BodyContains,
    BodyMatches,
    TimeBelow,
}

impl AssertKind {
    pub const ALL: [AssertKind; 8] = [AssertKind::StatusEquals, AssertKind::HeaderExists, AssertKind::HeaderEquals, AssertKind::JsonPathEquals, AssertKind::JsonPathExists, AssertKind::BodyContains, AssertKind::BodyMatches, AssertKind::TimeBelow];
    pub fn label(self) -> &'static str {
        match self {
            AssertKind::StatusEquals => "Status equals",
            AssertKind::HeaderExists => "Header exists",
            AssertKind::HeaderEquals => "Header equals",
            AssertKind::JsonPathEquals => "JSON path equals",
            AssertKind::JsonPathExists => "JSON path exists",
            AssertKind::BodyContains => "Body contains",
            AssertKind::BodyMatches => "Body matches (regex)",
            AssertKind::TimeBelow => "Time below (ms)",
        }
    }
    /// Uses the `target` field (header name or JSON path).
    pub fn has_target(self) -> bool { matches!(self, AssertKind::HeaderExists | AssertKind::HeaderEquals | AssertKind::JsonPathEquals | AssertKind::JsonPathExists) }
    /// Uses the `expected` field.
    pub fn has_expected(self) -> bool { !matches!(self, AssertKind::HeaderExists | AssertKind::JsonPathExists) }
}

/// One test on a Composer response.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Assertion {
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub kind: AssertKind,
    /// Header name or JSON path ($.a.b[0] or a.b.0).
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub expected: String,
}

impl Assertion {
    pub fn new(kind: AssertKind, target: &str, expected: &str) -> Self {
        Assertion { enabled: true, kind, target: target.to_string(), expected: expected.to_string() }
    }
    pub fn describe(&self) -> String {
        let mut s = self.kind.label().to_string();
        if self.kind.has_target() { s.push_str(&format!(" [{}]", self.target)); }
        if self.kind.has_expected() { s.push_str(&format!(" {}", self.expected)); }
        s
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct AssertResult {
    pub label: String,
    pub passed: bool,
    pub detail: String,
}

/// Look up a JSON path: `$.a.b[0].c`, `a.b.0.c` or `$['a']`. "$" alone is the root.
pub fn json_path<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    let p = path.trim();
    let p = p.strip_prefix('$').unwrap_or(p);
    let mut toks: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut chars = p.chars();
    while let Some(c) = chars.next() {
        match c {
            '.' => { if !cur.is_empty() { toks.push(std::mem::take(&mut cur)); } }
            '[' => {
                if !cur.is_empty() { toks.push(std::mem::take(&mut cur)); }
                let mut inner = String::new();
                for c2 in chars.by_ref() { if c2 == ']' { break; } inner.push(c2); }
                toks.push(inner.trim().trim_matches(|c| c == '\'' || c == '"').to_string());
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() { toks.push(cur); }
    let mut node = v;
    for t in &toks {
        node = match node {
            Value::Object(m) => m.get(t)?,
            Value::Array(a) => a.get(t.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(node)
}

fn json_equals(v: &Value, expected: &str) -> bool {
    let e = expected.trim();
    if let Value::String(s) = v { if s == expected || s == e { return true; } }
    match serde_json::from_str::<Value>(e) {
        Ok(ev) => &ev == v || (v.is_number() && ev.is_number() && v.as_f64() == ev.as_f64()),
        Err(_) => false,
    }
}

/// Evaluate one assertion against a response.
pub fn check(a: &Assertion, r: &ComposerResponse) -> AssertResult {
    let label = a.describe();
    let res = |passed: bool, detail: String| AssertResult { label: label.clone(), passed, detail };
    if !r.error.is_empty() {
        return res(false, format!("request failed: {}", r.error));
    }
    let header = |name: &str| r.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name.trim())).map(|(_, v)| v.clone());
    match a.kind {
        // Exact code, class (2xx), range (200-299) or list (201,204).
        AssertKind::StatusEquals => {
            let spec = a.expected.trim();
            res(!spec.is_empty() && crate::model::bp_status_matches(spec, r.status), format!("status {}", r.status))
        }
        AssertKind::HeaderExists => match header(&a.target) {
            Some(v) => res(true, format!("{}: {v}", a.target.trim())),
            None => res(false, "header missing".into()),
        },
        AssertKind::HeaderEquals => match header(&a.target) {
            Some(v) => res(v.trim() == a.expected.trim(), format!("got '{v}'")),
            None => res(false, "header missing".into()),
        },
        AssertKind::JsonPathEquals | AssertKind::JsonPathExists => {
            let Ok(v) = serde_json::from_str::<Value>(&r.body) else { return res(false, "body is not JSON".into()) };
            match json_path(&v, &a.target) {
                None => res(false, "path not found".into()),
                Some(x) if a.kind == AssertKind::JsonPathExists => res(true, format!("= {x}")),
                Some(x) => res(json_equals(x, &a.expected), format!("got {x}")),
            }
        }
        AssertKind::BodyContains => res(r.body.contains(&a.expected), format!("{} body bytes", r.body.len())),
        AssertKind::BodyMatches => match regex::Regex::new(&a.expected) {
            Ok(re) => res(re.is_match(&r.body), format!("{} body bytes", r.body.len())),
            Err(e) => res(false, format!("bad regex: {e}")),
        },
        AssertKind::TimeBelow => {
            let lim = a.expected.trim().parse::<u128>().unwrap_or(0);
            res(r.elapsed_ms < lim, format!("{} ms", r.elapsed_ms))
        }
    }
}

/// Run the enabled assertions.
pub fn run_tests(tests: &[Assertion], r: &ComposerResponse) -> Vec<AssertResult> {
    tests.iter().filter(|a| a.enabled).map(|a| check(a, r)).collect()
}

// ===================== Collection runner =====================

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct RunResult {
    pub name: String,
    pub method: String,
    pub url: String,
    pub status: u16,
    pub elapsed_ms: u128,
    pub size: usize,
    pub error: String,
    pub passed: usize,
    pub failed: usize,
    pub results: Vec<AssertResult>,
}

/// Send one saved request (with variables) and run its tests.
pub fn run_one(r: &SavedRequest, vars: &[(String, String)], settings: &ComposerSettings, proxy: &str) -> RunResult {
    let req = prepare(r, vars);
    let resp = send_request(&req, settings, proxy);
    let results = run_tests(&r.tests, &resp);
    let passed = results.iter().filter(|x| x.passed).count();
    RunResult {
        name: r.name.clone(),
        method: req.method.clone(),
        url: req.url.clone(),
        status: resp.status,
        elapsed_ms: resp.elapsed_ms,
        size: resp.body.len(),
        error: resp.error.clone(),
        passed,
        failed: results.len() - passed,
        results,
    }
}

/// Run the requests in order, calling `on` after each; stops early when `stop` is set. Returns how many ran.
pub fn run_collection(reqs: &[SavedRequest], vars: &[(String, String)], settings: &ComposerSettings, proxy: &str, stop: &AtomicBool, mut on: impl FnMut(RunResult)) -> usize {
    let mut n = 0;
    for r in reqs {
        if stop.load(Ordering::SeqCst) { break; }
        on(run_one(r, vars, settings, proxy));
        n += 1;
    }
    n
}

/// A collection run on a worker thread.
pub struct Runner {
    pub rx: Receiver<RunResult>,
    pub stop: Arc<AtomicBool>,
    pub done: Arc<AtomicBool>,
    pub total: usize,
    pub handle: Option<std::thread::JoinHandle<()>>,
}

impl Runner {
    pub fn start(reqs: Vec<SavedRequest>, vars: Vec<(String, String)>, settings: ComposerSettings, proxy: String, repaint: Option<egui::Context>) -> Runner {
        let (tx, rx): (Sender<RunResult>, Receiver<RunResult>) = std::sync::mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let (s2, d2) = (stop.clone(), done.clone());
        let total = reqs.len();
        let handle = std::thread::spawn(move || {
            run_collection(&reqs, &vars, &settings, &proxy, &s2, |r| {
                let _ = tx.send(r);
                if let Some(c) = &repaint { c.request_repaint(); }
            });
            d2.store(true, Ordering::SeqCst);
            if let Some(c) = &repaint { c.request_repaint(); }
        });
        Runner { rx, stop, done, total, handle: Some(handle) }
    }
    pub fn request_stop(&self) { self.stop.store(true, Ordering::SeqCst); }
    pub fn is_done(&self) -> bool { self.done.load(Ordering::SeqCst) }
    /// Wait for the worker and collect what is left in the channel.
    pub fn join(mut self) -> Vec<RunResult> {
        if let Some(h) = self.handle.take() { let _ = h.join(); }
        self.rx.try_iter().collect()
    }
}

/// One-line summary of a run.
pub fn run_summary(rs: &[RunResult]) -> String {
    let p: usize = rs.iter().map(|r| r.passed).sum();
    let f: usize = rs.iter().map(|r| r.failed).sum();
    let e = rs.iter().filter(|r| !r.error.is_empty()).count();
    format!("{} requests, {p} tests passed, {f} failed, {e} errors", rs.len())
}

// ===================== GraphQL =====================

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GraphqlParts {
    pub query: String,
    /// Variables as pretty JSON ("" when none).
    pub variables: String,
    pub operation_name: String,
}

fn parts_from_obj(m: &serde_json::Map<String, Value>) -> Option<GraphqlParts> {
    let q = m.get("query")?.as_str()?;
    let variables = match m.get("variables") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(v) => serde_json::to_string_pretty(v).unwrap_or_default(),
    };
    let operation_name = m.get("operationName").and_then(|v| v.as_str()).unwrap_or("").to_string();
    Some(GraphqlParts { query: q.to_string(), variables, operation_name })
}

/// Detect a GraphQL request: JSON body with a "query" string (or a batch of them), `application/graphql` body,
/// or a GET with a `query` parameter on a URL mentioning graphql.
pub fn detect_graphql(method: &str, url: &str, headers: &[(String, String)], body: &str) -> Option<GraphqlParts> {
    let b = body.trim();
    if b.starts_with('{') || b.starts_with('[') {
        if let Ok(v) = serde_json::from_str::<Value>(b) {
            match &v {
                Value::Object(m) => return parts_from_obj(m),
                Value::Array(a) => return a.first().and_then(|x| x.as_object()).and_then(parts_from_obj),
                _ => {}
            }
        }
    }
    let ct = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-type")).map(|(_, v)| v.to_lowercase()).unwrap_or_default();
    if ct.contains("application/graphql") && !b.is_empty() {
        return Some(GraphqlParts { query: body.to_string(), ..Default::default() });
    }
    if method.eq_ignore_ascii_case("GET") && url.to_lowercase().contains("graphql") {
        let (_, rows) = crate::model::parse_query(url);
        let get = |n: &str| rows.iter().find(|r| r.key == n).map(|r| r.value.clone()).unwrap_or_default();
        let q = get("query");
        if !q.is_empty() {
            let vars = get("variables");
            let variables = serde_json::from_str::<Value>(&vars).ok().map(|v| serde_json::to_string_pretty(&v).unwrap_or_default()).unwrap_or(vars);
            return Some(GraphqlParts { query: q, variables, operation_name: get("operationName") });
        }
    }
    None
}

/// Build the JSON body {"query", "variables"?, "operationName"?}. Variables must be a JSON object (or empty).
pub fn build_graphql_body(query: &str, variables: &str, operation_name: &str) -> Result<String, String> {
    let mut m = serde_json::Map::new();
    m.insert("query".into(), Value::String(query.to_string()));
    if !variables.trim().is_empty() {
        let v: Value = serde_json::from_str(variables.trim()).map_err(|e| format!("variables are not valid JSON: {e}"))?;
        if !v.is_object() { return Err("variables must be a JSON object".into()); }
        m.insert("variables".into(), v);
    }
    if !operation_name.trim().is_empty() { m.insert("operationName".into(), Value::String(operation_name.trim().to_string())); }
    serde_json::to_string_pretty(&Value::Object(m)).map_err(|e| e.to_string())
}

/// Operation type and name from the query text, e.g. ("query", "GetUser"); anonymous "{ ... }" is ("query", "").
pub fn graphql_operation(query: &str) -> (String, String) {
    let t = query.trim_start();
    for kw in ["query", "mutation", "subscription"] {
        if let Some(rest) = t.strip_prefix(kw) {
            let name: String = rest.trim_start().chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
            return (kw.to_string(), name);
        }
    }
    ("query".into(), String::new())
}

// ===================== Postman collection v2.1 =====================

pub const POSTMAN_SCHEMA: &str = "https://schema.getpostman.com/json/collection/v2.1.0/collection.json";

fn split_url(url: &str) -> (String, Vec<String>, Vec<String>) {
    // (protocol, host parts, path parts) for the Postman url object
    let (proto, rest) = match url.split_once("://") { Some((p, r)) => (p.to_string(), r), None => (String::new(), url) };
    let rest = rest.split(['?', '#']).next().unwrap_or("");
    let mut segs = rest.split('/');
    let host = segs.next().unwrap_or("");
    let host_parts = if host.starts_with("{{") { vec![host.to_string()] } else { host.split('.').map(|s| s.to_string()).collect() };
    let path: Vec<String> = segs.map(|s| s.to_string()).collect();
    (proto, host_parts, path)
}

fn status_text(s: u16) -> &'static str {
    match s { 200 => "OK", 201 => "Created", 204 => "No Content", 301 => "Moved Permanently", 302 => "Found", 304 => "Not Modified", 400 => "Bad Request", 401 => "Unauthorized", 403 => "Forbidden", 404 => "Not Found", 405 => "Method Not Allowed", 500 => "Internal Server Error", _ => "" }
}

/// Export a collection as a Postman v2.1 collection (our tests are kept under "ferrotapTests").
pub fn to_postman(c: &Collection, variables: &[(String, String)]) -> Value {
    let items: Vec<Value> = c.requests.iter().map(|r| {
        let raw = crate::model::build_url(&r.url, &r.params);
        let (proto, host, path) = split_url(&r.url);
        let mut url = json!({"raw": raw, "host": host, "path": path});
        if !proto.is_empty() { url["protocol"] = json!(proto); }
        if !r.params.is_empty() {
            url["query"] = Value::Array(r.params.iter().map(|p| { let mut q = json!({"key": p.key, "value": p.value}); if !p.enabled { q["disabled"] = json!(true); } q }).collect());
        }
        let header: Vec<Value> = r.headers.iter().map(|h| { let mut o = json!({"key": h.key, "value": h.value}); if !h.enabled { o["disabled"] = json!(true); } o }).collect();
        let mut req = json!({"method": if r.method.is_empty() { "GET" } else { &r.method }, "header": header, "url": url});
        if !r.body.is_empty() {
            let gql = if r.body_mode == "graphql" { detect_graphql("POST", &r.url, &[], &r.body) } else { None };
            req["body"] = match gql {
                Some(g) => json!({"mode": "graphql", "graphql": {"query": g.query, "variables": g.variables}}),
                None => {
                    let lang = if serde_json::from_str::<Value>(&r.body).is_ok() { "json" } else { "text" };
                    json!({"mode": "raw", "raw": r.body, "options": {"raw": {"language": lang}}})
                }
            };
        }
        let mut item = json!({"name": r.name, "request": req, "response": []});
        if let Some(ex) = &r.example {
            item["response"] = json!([{ "name": format!("{} example", r.name), "originalRequest": req, "code": ex.status,
                "status": status_text(ex.status), "header": ex.headers.iter().map(|(k, v)| json!({"key": k, "value": v})).collect::<Vec<_>>(), "body": ex.body }]);
        }
        if !r.tests.is_empty() { item["ferrotapTests"] = serde_json::to_value(&r.tests).unwrap_or(Value::Null); }
        item
    }).collect();
    let mut v = json!({"info": {"name": c.name, "schema": POSTMAN_SCHEMA}, "item": items});
    if !variables.is_empty() {
        v["variable"] = Value::Array(variables.iter().map(|(k, val)| json!({"key": k, "value": val})).collect());
    }
    v
}

fn str_of(v: &Value) -> String {
    match v { Value::String(s) => s.clone(), Value::Null => String::new(), other => other.to_string() }
}

fn kv_from(v: &Value) -> KvRow {
    KvRow { enabled: !v.get("disabled").and_then(|d| d.as_bool()).unwrap_or(false), key: v.get("key").map(str_of).unwrap_or_default(), value: v.get("value").map(str_of).unwrap_or_default() }
}

fn postman_url(u: &Value) -> (String, Vec<KvRow>) {
    match u {
        Value::String(s) => crate::model::parse_query(s),
        Value::Object(m) => {
            let raw = m.get("raw").map(str_of).unwrap_or_else(|| {
                let proto = m.get("protocol").map(str_of).unwrap_or_default();
                let host = match m.get("host") { Some(Value::Array(a)) => a.iter().map(str_of).collect::<Vec<_>>().join("."), Some(v) => str_of(v), None => String::new() };
                let path = match m.get("path") { Some(Value::Array(a)) => a.iter().map(str_of).collect::<Vec<_>>().join("/"), Some(v) => str_of(v), None => String::new() };
                format!("{}{}{}{}", if proto.is_empty() { String::new() } else { format!("{proto}://") }, host, if path.is_empty() { "" } else { "/" }, path)
            });
            let (base, parsed) = crate::model::parse_query(&raw);
            let rows = match m.get("query") {
                Some(Value::Array(a)) => a.iter().map(kv_from).collect(),
                _ => parsed,
            };
            (base, rows)
        }
        _ => (String::new(), Vec::new()),
    }
}

fn postman_items(items: &[Value], prefix: &str, out: &mut Vec<SavedRequest>) {
    for it in items {
        let name = it.get("name").map(str_of).unwrap_or_default();
        if let Some(Value::Array(sub)) = it.get("item") {
            let p = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
            postman_items(sub, &p, out);
            continue;
        }
        let Some(req) = it.get("request") else { continue };
        let mut r = SavedRequest { name: if prefix.is_empty() { name } else { format!("{prefix}/{name}") }, ..Default::default() };
        if let Value::String(u) = req {
            r.method = "GET".into();
            let (b, p) = crate::model::parse_query(u);
            r.url = b;
            r.params = p;
        } else {
            r.method = req.get("method").map(str_of).unwrap_or_else(|| "GET".into()).to_uppercase();
            if let Some(u) = req.get("url") { let (b, p) = postman_url(u); r.url = b; r.params = p; }
            if let Some(Value::Array(hs)) = req.get("header") { r.headers = hs.iter().map(kv_from).collect(); }
            if let Some(b) = req.get("body") {
                let mode = b.get("mode").and_then(|m| m.as_str()).unwrap_or("raw").to_string();
                match mode.as_str() {
                    "graphql" => {
                        let g = b.get("graphql").cloned().unwrap_or(Value::Null);
                        let q = g.get("query").map(str_of).unwrap_or_default();
                        let vars = g.get("variables").map(|v| if let Value::String(s) = v { s.clone() } else { v.to_string() }).unwrap_or_default();
                        r.body = build_graphql_body(&q, &vars, "").or_else(|_| build_graphql_body(&q, "", "")).unwrap_or_default();
                        r.body_mode = "graphql".into();
                    }
                    "urlencoded" | "formdata" => {
                        if let Some(Value::Array(a)) = b.get(&mode) {
                            r.body = a.iter().map(kv_from).filter(|p| p.enabled)
                                .map(|p| format!("{}={}", crate::model::pct_encode(&p.key), crate::model::pct_encode(&p.value)))
                                .collect::<Vec<_>>().join("&");
                        }
                    }
                    _ => r.body = b.get("raw").map(str_of).unwrap_or_default(),
                }
            }
        }
        if let Some(Value::Array(rs)) = it.get("response") {
            if let Some(x) = rs.first() {
                let headers = match x.get("header") { Some(Value::Array(a)) => a.iter().map(|h| (h.get("key").map(str_of).unwrap_or_default(), h.get("value").map(str_of).unwrap_or_default())).collect(), _ => Vec::new() };
                r.example = Some(SavedResponse { status: x.get("code").and_then(|c| c.as_u64()).unwrap_or(200) as u16, headers, body: x.get("body").map(str_of).unwrap_or_default() });
            }
        }
        if let Some(t) = it.get("ferrotapTests") { r.tests = serde_json::from_value(t.clone()).unwrap_or_default(); }
        out.push(r);
    }
}

/// Import a Postman v2.0/v2.1 collection: (collection, collection variables). Folders are flattened as "folder/name".
pub fn from_postman(text: &str) -> Result<(Collection, Vec<KvRow>), String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
    let info = v.get("info").ok_or("not a Postman collection (no info)")?;
    let name = info.get("name").map(str_of).unwrap_or_else(|| "Imported".into());
    let mut reqs = Vec::new();
    match v.get("item") {
        Some(Value::Array(items)) => postman_items(items, "", &mut reqs),
        _ => return Err("not a Postman collection (no item list)".into()),
    }
    let vars = match v.get("variable") { Some(Value::Array(a)) => a.iter().map(kv_from).collect(), _ => Vec::new() };
    Ok((Collection { name, requests: reqs, folders: Vec::new() }, vars))
}

// ===================== OpenAPI 3 =====================

const METHODS: [&str; 7] = ["get", "post", "put", "patch", "delete", "head", "options"];

fn example_of(v: &Value) -> Option<Value> {
    if let Some(e) = v.get("example") { return Some(e.clone()); }
    if let Some(Value::Object(m)) = v.get("examples") {
        if let Some(first) = m.values().next() { return Some(first.get("value").cloned().unwrap_or_else(|| first.clone())); }
    }
    let s = v.get("schema")?;
    s.get("example").or_else(|| s.get("default")).cloned()
}

fn example_text(v: &Value) -> String {
    match v { Value::String(s) => s.clone(), other => serde_json::to_string_pretty(other).unwrap_or_default() }
}

/// `{id}` path templates become `{{id}}` variables.
fn braces_to_vars(path: &str) -> String {
    let mut o = String::new();
    let mut rest = path;
    while let Some(i) = rest.find('{') {
        o.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find('}') {
            Some(j) => { o.push_str("{{"); o.push_str(&after[..j]); o.push_str("}}"); rest = &after[j + 1..]; }
            None => { o.push_str(&rest[i..]); rest = ""; }
        }
    }
    o.push_str(rest);
    o
}

/// Import an OpenAPI 3 document (JSON): one request per path + method.
pub fn from_openapi(text: &str) -> Result<Collection, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("not JSON (YAML is not supported, convert it to JSON first): {e}"))?;
    if v.get("openapi").is_none() && v.get("swagger").is_none() { return Err("not an OpenAPI document (no 'openapi' field)".into()); }
    let title = v.pointer("/info/title").map(str_of).unwrap_or_else(|| "OpenAPI".into());
    let mut base = v.pointer("/servers/0/url").map(str_of).unwrap_or_default();
    if let Some(Value::Object(sv)) = v.pointer("/servers/0/variables") {
        for (k, d) in sv { base = base.replace(&format!("{{{k}}}"), &d.get("default").map(str_of).unwrap_or_default()); }
    }
    if base.is_empty() { base = "{{baseUrl}}".into(); }
    let base = base.trim_end_matches('/').to_string();
    let mut c = Collection::new(&title);
    let Some(Value::Object(paths)) = v.get("paths") else { return Err("no paths".into()) };
    for (path, item) in paths {
        let common: Vec<Value> = item.get("parameters").and_then(|p| p.as_array()).cloned().unwrap_or_default();
        for m in METHODS {
            let Some(op) = item.get(m) else { continue };
            let mut r = SavedRequest { method: m.to_uppercase(), url: format!("{base}{}", braces_to_vars(path)), ..Default::default() };
            r.name = op.get("operationId").or_else(|| op.get("summary")).map(str_of).filter(|s| !s.is_empty()).unwrap_or_else(|| format!("{} {}", m.to_uppercase(), path));
            let mut params = common.clone();
            if let Some(Value::Array(p)) = op.get("parameters") { params.extend(p.iter().cloned()); }
            for p in &params {
                let name = p.get("name").map(str_of).unwrap_or_default();
                let val = example_of(p).map(|e| example_text(&e)).unwrap_or_default();
                match p.get("in").and_then(|x| x.as_str()).unwrap_or("") {
                    "query" => r.params.push(KvRow::new(&name, &val)),
                    "header" => r.headers.push(KvRow::new(&name, &val)),
                    _ => {}
                }
            }
            if let Some(Value::Object(content)) = op.pointer("/requestBody/content") {
                if let Some((ct, media)) = content.iter().find(|(k, _)| k.contains("json")).or_else(|| content.iter().next()) {
                    r.headers.push(KvRow::new("Content-Type", ct));
                    if let Some(e) = example_of(media) { r.body = example_text(&e); }
                }
            }
            if let Some(Value::Object(resps)) = op.get("responses") {
                if let Some((code, resp)) = resps.iter().find(|(k, _)| k.starts_with('2')).or_else(|| resps.iter().next()) {
                    let mut ex = SavedResponse { status: code.parse::<u16>().unwrap_or(200), ..Default::default() };
                    if let Some(Value::Object(content)) = resp.get("content") {
                        if let Some((ct, media)) = content.iter().next() {
                            ex.headers.push(("Content-Type".into(), ct.clone()));
                            if let Some(e) = example_of(media) { ex.body = example_text(&e); }
                        }
                    }
                    r.example = Some(ex);
                }
            }
            if c.find(&r.name).is_some() { r.name = format!("{} ({} {})", r.name, m.to_uppercase(), path); }
            c.requests.push(r);
        }
    }
    Ok(c)
}

/// (origin, path) of a URL: origin is scheme://authority or a leading {{variable}}.
pub fn split_origin(url: &str) -> (String, String) {
    let u = url.trim();
    if u.starts_with("{{") {
        if let Some(j) = u.find("}}") { let (a, b) = u.split_at(j + 2); return (a.to_string(), b.to_string()); }
    }
    if let Some((scheme, rest)) = u.split_once("://") {
        let auth_len = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        return (format!("{scheme}://{}", &rest[..auth_len]), rest[auth_len..].to_string());
    }
    (String::new(), u.to_string())
}

/// URL path without origin and query ("/" when empty).
pub fn url_path(url: &str) -> String {
    let (_, p) = split_origin(url);
    let p = p.split(['?', '#']).next().unwrap_or("").to_string();
    if p.is_empty() { "/".into() } else { p }
}

fn op_id(name: &str) -> String {
    let mut o = String::new();
    let mut up = false;
    for ch in name.chars() {
        if ch.is_alphanumeric() {
            if up && !o.is_empty() { o.extend(ch.to_uppercase()); } else { o.push(ch); }
            up = false;
        } else { up = true; }
    }
    if o.is_empty() { "operation".into() } else { o }
}

/// Export a minimal OpenAPI 3.0 document for a collection.
pub fn to_openapi(c: &Collection) -> Value {
    let mut servers: Vec<String> = Vec::new();
    let mut paths = serde_json::Map::new();
    for r in &c.requests {
        let (origin, _) = split_origin(&r.url);
        if !origin.is_empty() && !servers.contains(&origin) { servers.push(origin); }
        let mut p = url_path(&r.url);
        let path_vars = variables_in(&p);
        for n in &path_vars { p = p.replace(&format!("{{{{{n}}}}}"), &format!("{{{n}}}")); }
        let mut params: Vec<Value> = path_vars.iter().map(|n| json!({"name": n, "in": "path", "required": true, "schema": {"type": "string"}})).collect();
        for q in r.params.iter().filter(|q| !q.key.is_empty()) { params.push(json!({"name": q.key, "in": "query", "required": false, "schema": {"type": "string"}, "example": q.value})); }
        let ct = r.headers.iter().find(|h| h.key.eq_ignore_ascii_case("content-type")).map(|h| h.value.clone());
        for h in r.headers.iter().filter(|h| !h.key.is_empty() && !h.key.eq_ignore_ascii_case("content-type")) { params.push(json!({"name": h.key, "in": "header", "required": false, "schema": {"type": "string"}, "example": h.value})); }
        let method = if r.method.is_empty() { "get".to_string() } else { r.method.to_lowercase() };
        let mut op = json!({"summary": r.name, "operationId": op_id(&r.name)});
        if !params.is_empty() { op["parameters"] = Value::Array(params); }
        if !r.body.is_empty() {
            let ct = ct.clone().unwrap_or_else(|| if serde_json::from_str::<Value>(&r.body).is_ok() { "application/json".into() } else { "text/plain".into() });
            let ex = serde_json::from_str::<Value>(&r.body).unwrap_or_else(|_| Value::String(r.body.clone()));
            op["requestBody"] = json!({"content": {ct: {"example": ex}}});
        }
        op["responses"] = match &r.example {
            Some(ex) => {
                let mut resp = json!({"description": if status_text(ex.status).is_empty() { "Response" } else { status_text(ex.status) }});
                if !ex.body.is_empty() {
                    let ct = ex.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-type")).map(|(_, v)| v.split(';').next().unwrap_or("").trim().to_string()).unwrap_or_else(|| "application/json".into());
                    let body = serde_json::from_str::<Value>(&ex.body).unwrap_or_else(|_| Value::String(ex.body.clone()));
                    resp["content"] = json!({ct: {"example": body}});
                }
                json!({ex.status.to_string(): resp})
            }
            None => json!({"200": {"description": "OK"}}),
        };
        let entry = paths.entry(p).or_insert_with(|| json!({}));
        entry[method] = op;
    }
    let servers: Vec<Value> = servers.iter().map(|s| {
        if s.starts_with("{{") {
            let n = s.trim_matches(|c| c == '{' || c == '}').trim().to_string();
            json!({"url": format!("{{{n}}}"), "variables": {n: {"default": "http://localhost"}}})
        } else { json!({"url": s}) }
    }).collect();
    json!({"openapi": "3.0.3", "info": {"title": c.name, "version": "1.0.0"}, "servers": servers, "paths": paths})
}

// ===================== Build a collection from captured traffic =====================

fn skip_captured_header(k: &str) -> bool {
    let k = k.to_ascii_lowercase();
    matches!(k.as_str(), "host" | "content-length" | "connection" | "proxy-connection" | "keep-alive" | "transfer-encoding" | "te" | "upgrade" | "accept-encoding")
        || k.starts_with("proxy-") || k.starts_with(':')
}

/// One Composer request (with the captured response as example) per session; names are unique.
pub fn collection_from_sessions(name: &str, sessions: &[&SessionRecord]) -> Collection {
    let mut c = Collection::new(name);
    for s in sessions {
        if s.method.eq_ignore_ascii_case("CONNECT") { continue; }
        let (base, params) = crate::model::parse_query(&s.url);
        let mut nm = format!("{} {}", s.method, url_path(&base));
        if c.find(&nm).is_some() {
            let mut i = 2;
            while c.find(&format!("{nm} ({i})")).is_some() { i += 1; }
            nm = format!("{nm} ({i})");
        }
        let headers = s.request_headers.iter().filter(|(k, _)| !skip_captured_header(k)).map(|(k, v)| KvRow::new(k, v)).collect();
        let example = if s.status > 0 {
            Some(SavedResponse { status: s.status, headers: s.response_headers.iter().filter(|(k, _)| !is_framing_header(k)).map(|(k, v)| (k.clone(), v.clone())).collect(), body: s.response_body.clone() })
        } else { None };
        let body_mode = if !s.request_body.is_empty() && detect_graphql(&s.method, &s.url, &[], &s.request_body).is_some() { "graphql".to_string() } else { String::new() };
        c.requests.push(SavedRequest { name: nm, method: s.method.clone(), url: base, params, headers, body: s.request_body.clone(), body_mode, example, ..Default::default() });
    }
    c
}

// ===================== Local mock server =====================

#[derive(Clone, Debug, PartialEq)]
pub struct MockRoute {
    pub method: String,
    /// Path pattern: `{{x}}`, `{x}`, `:x` and `*` segments match any segment.
    pub path: String,
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

/// Routes for every request with a saved response (variables substituted first; unknown ones stay wildcards).
pub fn routes_from_collection(c: &Collection, vars: &[(String, String)]) -> Vec<MockRoute> {
    c.requests.iter().filter_map(|r| {
        let ex = r.example.as_ref()?;
        let url = substitute(&r.url, vars);
        Some(MockRoute { method: if r.method.is_empty() { "GET".into() } else { r.method.to_uppercase() }, path: url_path(&url), status: ex.status, headers: ex.headers.clone(), body: ex.body.clone() })
    }).collect()
}

fn is_wild(seg: &str) -> bool {
    seg == "*" || seg.starts_with(':') || (seg.starts_with('{') && seg.ends_with('}'))
}

pub fn path_matches(pattern: &str, path: &str) -> bool {
    let a: Vec<&str> = pattern.trim_end_matches('/').split('/').collect();
    let b: Vec<&str> = path.trim_end_matches('/').split('/').collect();
    a.len() == b.len() && a.iter().zip(b.iter()).all(|(p, s)| is_wild(p) || p == s)
}

/// (status, headers, body) answered for `method path`: an exact-path route wins over wildcard routes;
/// 405 if only the method differs, 404 otherwise. HEAD falls back to GET routes.
pub fn mock_answer(routes: &[MockRoute], method: &str, path: &str) -> (u16, Vec<(String, String)>, String) {
    let path = path.split(['?', '#']).next().unwrap_or("/");
    let m = method.to_uppercase();
    let same_method = |r: &&&MockRoute| r.method == m || (m == "HEAD" && r.method == "GET");
    let cands: Vec<&MockRoute> = routes.iter().filter(|r| path_matches(&r.path, path)).collect();
    let pick = cands.iter().filter(same_method).find(|r| r.path.trim_end_matches('/') == path.trim_end_matches('/'))
        .or_else(|| cands.iter().find(same_method));
    let json_ct = || vec![("Content-Type".to_string(), "application/json".to_string())];
    match pick {
        Some(r) => (r.status, r.headers.clone(), r.body.clone()),
        None if !cands.is_empty() => (405, json_ct(), json!({"error": "method not allowed", "method": m, "path": path}).to_string()),
        None => (404, json_ct(), json!({"error": "no mock route", "method": m, "path": path}).to_string()),
    }
}

/// A small HTTP/1.1 server answering from `MockRoute`s on a thread; stop() or drop it to shut down.
pub struct MockServer {
    pub port: u16,
    pub hits: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

fn handle_mock_conn(mut st: std::net::TcpStream, routes: &[MockRoute]) {
    use std::io::{Read, Write};
    let _ = st.set_nonblocking(false);
    let _ = st.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    let mut buf = Vec::new();
    let mut b = [0u8; 4096];
    let head_end = loop {
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") { break p + 4; }
        match st.read(&mut b) { Ok(0) | Err(_) => return, Ok(k) => buf.extend_from_slice(&b[..k]) }
        if buf.len() > 1 << 20 { return; }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let want = head.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))).unwrap_or(0);
    while buf.len() < head_end + want.min(10 << 20) {
        match st.read(&mut b) { Ok(0) | Err(_) => break, Ok(k) => buf.extend_from_slice(&b[..k]) }
    }
    let mut parts = head.lines().next().unwrap_or("").split_whitespace();
    let method = parts.next().unwrap_or("GET").to_string();
    let target = parts.next().unwrap_or("/");
    let path = if target.contains("://") { url_path(target) } else { target.split(['?', '#']).next().unwrap_or("/").to_string() };
    let (status, headers, body) = mock_answer(routes, &method, &path);
    let mut out = format!("HTTP/1.1 {} {}\r\n", status, if status_text(status).is_empty() { "Mock" } else { status_text(status) });
    for (k, v) in headers.iter().filter(|(k, _)| !is_framing_header(k)) { out.push_str(&format!("{k}: {v}\r\n")); }
    out.push_str(&format!("Content-Length: {}\r\nX-Ferrotap-Mock: 1\r\nConnection: close\r\n\r\n", body.len()));
    let mut bytes = out.into_bytes();
    if !method.eq_ignore_ascii_case("HEAD") { bytes.extend_from_slice(body.as_bytes()); }
    let _ = st.write_all(&bytes);
    let _ = st.flush();
}

impl MockServer {
    /// Bind `addr` (e.g. "127.0.0.1:0" for a random port) and serve `routes`.
    pub fn start(addr: &str, routes: Vec<MockRoute>) -> Result<MockServer, String> {
        let l = std::net::TcpListener::bind(addr).map_err(|e| format!("bind {addr}: {e}"))?;
        let port = l.local_addr().map_err(|e| e.to_string())?.port();
        l.set_nonblocking(true).map_err(|e| e.to_string())?;
        let stop = Arc::new(AtomicBool::new(false));
        let hits = Arc::new(AtomicUsize::new(0));
        let (s2, h2) = (stop.clone(), hits.clone());
        let routes = Arc::new(routes);
        let handle = std::thread::spawn(move || {
            while !s2.load(Ordering::SeqCst) {
                match l.accept() {
                    Ok((st, _)) => {
                        h2.fetch_add(1, Ordering::SeqCst);
                        let r = routes.clone();
                        std::thread::spawn(move || handle_mock_conn(st, &r));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(std::time::Duration::from_millis(15)),
                    Err(_) => std::thread::sleep(std::time::Duration::from_millis(50)),
                }
            }
        });
        Ok(MockServer { port, hits, stop, handle: Some(handle) })
    }
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() { let _ = h.join(); }
    }
    pub fn is_running(&self) -> bool { self.handle.is_some() }
}

impl Drop for MockServer {
    fn drop(&mut self) { self.stop(); }
}

// ===================== Raw headers =====================

/// Header rows as "Name: value" lines; disabled rows are prefixed with "# ".
pub fn rows_to_raw(rows: &[KvRow]) -> String {
    rows.iter().map(|r| format!("{}{}: {}", if r.enabled { "" } else { "# " }, r.key, r.value)).collect::<Vec<_>>().join("\n")
}

pub fn raw_to_rows(text: &str) -> Vec<KvRow> {
    text.lines().filter_map(|l| {
        let t = l.trim();
        let (enabled, t) = match t.strip_prefix('#') { Some(r) => (false, r.trim()), None => (true, t) };
        let (k, v) = t.split_once(':')?;
        if k.trim().is_empty() { return None; }
        Some(KvRow { enabled, key: k.trim().to_string(), value: v.trim().to_string() })
    }).collect()
}

// ===================== UI state =====================

/// One Composer tab (the active tab's live data is in GridApp's comp_* fields).
#[derive(Clone, Default)]
pub struct ComposerTab {
    pub req: SavedRequest,
    pub resp: Option<ComposerResponse>,
    pub results: Vec<AssertResult>,
    pub coll: String,
    pub name: String,
}

/// This module's UI state (kept in `GridApp::ext.api`).
#[derive(Default)]
pub struct State {
    pub loaded: bool,
    pub envs: EnvStore,
    pub settings: ComposerSettings,
    // Composer additions
    pub comp_tests: Vec<Assertion>,
    pub last_results: Vec<AssertResult>,
    pub graphql_mode: bool,
    pub gql_query: String,
    pub gql_vars: String,
    pub gql_op: String,
    pub code_lang: usize,
    pub headers_raw: bool,
    pub headers_raw_text: String,
    /// Do not store the last response as the request's example when saving.
    pub no_example: bool,
    pub tabs: Vec<ComposerTab>,
    pub tab_active: usize,
    // Environments window
    pub show_envs: bool,
    pub env_edit: String,
    pub env_new: String,
    // Runner
    pub show_runner: bool,
    pub runner_coll: String,
    pub runner: Option<Runner>,
    pub run_results: Vec<RunResult>,
    pub run_msg: String,
    // Import / export
    pub show_io: bool,
    pub io_path: String,
    pub io_coll: String,
    pub io_msg: String,
    pub traffic_name: String,
    // GraphQL session view
    pub show_gql: bool,
    // Mock server
    pub show_mock: bool,
    pub mock_coll: String,
    pub mock_port: String,
    pub mock: Option<MockServer>,
    pub mock_routes: Vec<MockRoute>,
    pub mock_msg: String,
    pub msg: String,
}

impl State {
    pub fn ensure_loaded(&mut self, dir: &Path) {
        if self.loaded { return; }
        self.envs = load_envs(dir);
        self.settings = load_settings(dir);
        self.loaded = true;
    }
    pub fn active_vars(&self) -> Vec<(String, String)> { self.envs.active_vars() }
    /// Body to send/save: in GraphQL mode built from the query/variables/operation editors.
    pub fn effective_body(&self, raw: &str) -> String {
        if self.graphql_mode { build_graphql_body(&self.gql_query, &self.gql_vars, &self.gql_op).unwrap_or_else(|_| raw.to_string()) } else { raw.to_string() }
    }
    /// Load the tests, GraphQL editors and body mode from a saved request.
    pub fn load_from(&mut self, r: &SavedRequest) {
        self.comp_tests = r.tests.clone();
        self.last_results.clear();
        self.graphql_mode = r.body_mode == "graphql";
        let g = detect_graphql(&r.method, &r.url, &crate::model::rows_to_headers(&r.headers), &r.body).unwrap_or_default();
        self.gql_query = g.query;
        self.gql_vars = g.variables;
        self.gql_op = g.operation_name;
        self.headers_raw_text = rows_to_raw(&r.headers);
    }
    pub fn body_mode(&self) -> String { if self.graphql_mode { "graphql".into() } else { String::new() } }
}

fn proxy_addr() -> String { format!("127.0.0.1:{}", crate::conn::active_port()) }

fn short(url: &str) -> String {
    let p = url_path(url);
    if p.chars().count() > 24 { format!("{}...", p.chars().take(24).collect::<String>()) } else { p }
}

/// Read-only Query / Operation / Variables view of a GraphQL request.
pub fn api_gql_view(ui: &mut egui::Ui, g: &GraphqlParts, id: &str) {
    let (kind, name) = graphql_operation(&g.query);
    let op = if !g.operation_name.is_empty() { g.operation_name.clone() } else { name };
    ui.label(RichText::new(format!("Operation: {kind} {}", if op.is_empty() { "(anonymous)" } else { &op })).strong());
    ui.label("Query");
    egui::ScrollArea::vertical().max_height(160.0).id_salt(format!("{id}_q")).show(ui, |ui| {
        ui.add(egui::TextEdit::multiline(&mut g.query.as_str()).code_editor().desired_width(f32::INFINITY));
    });
    ui.label("Variables");
    let vars = if g.variables.is_empty() { "(none)".to_string() } else { g.variables.clone() };
    egui::ScrollArea::vertical().max_height(100.0).id_salt(format!("{id}_v")).show(ui, |ui| {
        ui.add(egui::TextEdit::multiline(&mut vars.as_str()).code_editor().desired_width(f32::INFINITY));
    });
}

impl GridApp {
    /// Toolbar entry: the "API" menu.
    pub fn api_tools_menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("API", |ui| {
            if ui.button("Composer").clicked() { self.show_composer = true; ui.close(); }
            if ui.button("Environments...").clicked() { self.ext.api.show_envs = true; ui.close(); }
            if ui.button("Collection runner...").clicked() { self.ext.api.show_runner = true; ui.close(); }
            if ui.button("Import / export...").clicked() { self.ext.api.show_io = true; ui.close(); }
            if ui.button("Mock server...").clicked() { self.ext.api.show_mock = true; ui.close(); }
            if ui.button("GraphQL view of selected session").clicked() { self.ext.api.show_gql = true; ui.close(); }
        });
    }

    /// Windows this module draws each frame.
    pub fn api_tools_windows(&mut self, ctx: &egui::Context) {
        let dir = self.comp_dir.clone();
        self.ext.api.ensure_loaded(&dir);
        self.api_envs_window(ctx);
        self.api_runner_window(ctx);
        self.api_io_window(ctx);
        self.api_mock_window(ctx);
        self.api_gql_window(ctx);
    }

    fn api_coll_combo(ui: &mut egui::Ui, id: &str, colls: &[Collection], sel: &mut String) {
        if sel.is_empty() || !colls.iter().any(|c| &c.name == sel) { if let Some(c) = colls.first() { *sel = c.name.clone(); } }
        egui::ComboBox::from_id_salt(id.to_string()).selected_text(if sel.is_empty() { "(no collections)".to_string() } else { sel.clone() }).show_ui(ui, |ui| {
            for c in colls { ui.selectable_value(sel, c.name.clone(), format!("{} ({})", c.name, c.requests.len())); }
        });
    }

    pub fn api_save_envs(&mut self) {
        self.ext.api.msg = match save_envs(&self.comp_dir, &self.ext.api.envs) { Ok(()) => "environments saved".into(), Err(e) => e };
    }

    // ---------- Composer pieces (called from composer_window) ----------

    /// Tab strip above the Composer request.
    pub fn api_composer_tabs(&mut self, ui: &mut egui::Ui) {
        if self.ext.api.tabs.is_empty() { self.ext.api.tabs.push(ComposerTab::default()); self.ext.api.tab_active = 0; }
        let mut switch: Option<usize> = None;
        let mut close: Option<usize> = None;
        let mut add = false;
        ui.horizontal_wrapped(|ui| {
            for i in 0..self.ext.api.tabs.len() {
                let t = &self.ext.api.tabs[i];
                let label = if i == self.ext.api.tab_active {
                    if !self.comp_save_name.trim().is_empty() { self.comp_save_name.clone() } else if self.comp_url.is_empty() { "New request".into() } else { format!("{} {}", self.comp_method, short(&self.comp_url)) }
                } else if !t.name.trim().is_empty() { t.name.clone() } else if t.req.url.is_empty() { "New request".into() } else { format!("{} {}", t.req.method, short(&t.req.url)) };
                if ui.selectable_label(i == self.ext.api.tab_active, label).clicked() && i != self.ext.api.tab_active { switch = Some(i); }
                if self.ext.api.tabs.len() > 1 && ui.small_button("x").on_hover_text("Close tab").clicked() { close = Some(i); }
            }
            if ui.small_button("+").on_hover_text("New Composer tab").clicked() { add = true; }
        });
        if add { self.api_tab_new(); } else if let Some(i) = switch { self.api_tab_switch(i); } else if let Some(i) = close { self.api_tab_close(i); }
    }

    fn api_tab_store(&mut self) {
        let cur = ComposerTab { req: self.current_saved(""), resp: self.comp_resp.clone(), results: self.ext.api.last_results.clone(), coll: self.comp_save_coll.clone(), name: self.comp_save_name.clone() };
        let a = self.ext.api.tab_active;
        if a < self.ext.api.tabs.len() { self.ext.api.tabs[a] = cur; }
    }

    fn api_tab_load(&mut self, i: usize) {
        let t = self.ext.api.tabs[i].clone();
        self.apply_saved(&t.req);
        self.comp_resp = t.resp;
        self.ext.api.last_results = t.results;
        if !t.coll.is_empty() { self.comp_save_coll = t.coll; }
        self.comp_save_name = t.name;
        self.ext.api.tab_active = i;
    }

    /// Open a new empty Composer tab (the current one is kept).
    pub fn api_tab_new(&mut self) {
        if self.ext.api.tabs.is_empty() { self.ext.api.tabs.push(ComposerTab::default()); }
        self.api_tab_store();
        self.ext.api.tabs.push(ComposerTab { req: SavedRequest { method: "GET".into(), ..Default::default() }, coll: self.comp_save_coll.clone(), ..Default::default() });
        let n = self.ext.api.tabs.len() - 1;
        self.api_tab_load(n);
    }

    pub fn api_tab_switch(&mut self, i: usize) {
        if i >= self.ext.api.tabs.len() || i == self.ext.api.tab_active { return; }
        self.api_tab_store();
        self.api_tab_load(i);
    }

    pub fn api_tab_close(&mut self, i: usize) {
        if self.ext.api.tabs.len() <= 1 || i >= self.ext.api.tabs.len() { return; }
        self.api_tab_store();
        self.ext.api.tabs.remove(i);
        let a = self.ext.api.tab_active;
        let new_a = if i < a { a - 1 } else if i == a { a.min(self.ext.api.tabs.len() - 1) } else { a };
        self.api_tab_load(new_a);
    }

    /// Environment selector + settings row in the Composer.
    pub fn api_composer_env_row(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        let mut settings_changed = false;
        ui.horizontal(|ui| {
            ui.label("Environment:");
            let st = &mut self.ext.api;
            let sel = if st.envs.active.is_empty() { "(none)".to_string() } else { st.envs.active.clone() };
            let names: Vec<String> = st.envs.envs.iter().map(|e| e.name.clone()).collect();
            egui::ComboBox::from_id_salt("comp_env").selected_text(sel).show_ui(ui, |ui| {
                changed |= ui.selectable_value(&mut st.envs.active, String::new(), "(none)").changed();
                for n in names { changed |= ui.selectable_value(&mut st.envs.active, n.clone(), n).changed(); }
            });
            if ui.small_button("Manage...").clicked() { st.show_envs = true; }
            ui.separator();
            settings_changed |= ui.checkbox(&mut st.settings.follow_redirects, "Follow redirects").changed();
            settings_changed |= ui.checkbox(&mut st.settings.http10, "HTTP/1.0").on_hover_text("Send the request as HTTP/1.0 instead of HTTP/1.1").changed();
            ui.label("Timeout s:");
            settings_changed |= ui.add(egui::DragValue::new(&mut st.settings.timeout_secs).range(1..=600)).changed();
            settings_changed |= ui.checkbox(&mut st.settings.via_proxy, "Via capture proxy").on_hover_text("Off: send directly to the server (not captured)").changed();
        });
        let vars = self.ext.api.active_vars();
        let mut used = variables_in(&self.comp_url);
        for r in self.comp_params.iter().chain(self.comp_hdr_rows.iter()) { used.extend(variables_in(&r.key)); used.extend(variables_in(&r.value)); }
        used.extend(variables_in(&self.comp_body));
        let mut missing: Vec<String> = used.into_iter().filter(|n| !vars.iter().any(|(k, _)| k == n)).collect();
        missing.sort();
        missing.dedup();
        if !missing.is_empty() {
            ui.label(RichText::new(format!("Undefined variables: {}", missing.join(", "))).small().color(Color32::from_rgb(220, 160, 60)));
        } else if !vars.is_empty() && self.comp_url.contains("{{") {
            ui.label(RichText::new(format!("-> {}", substitute(&crate::model::build_url(&self.comp_url, &self.comp_params), &vars))).small().color(Color32::GRAY));
        }
        if changed { self.api_save_envs(); }
        if settings_changed { let _ = save_settings(&self.comp_dir, &self.ext.api.settings); }
    }

    /// Headers tab with a Table / Raw toggle; returns true when the raw editor was drawn.
    pub fn api_composer_headers(&mut self, ui: &mut egui::Ui) -> bool {
        ui.horizontal(|ui| {
            let was = self.ext.api.headers_raw;
            ui.radio_value(&mut self.ext.api.headers_raw, false, "Table");
            ui.radio_value(&mut self.ext.api.headers_raw, true, "Raw");
            if !was && self.ext.api.headers_raw { self.ext.api.headers_raw_text = rows_to_raw(&self.comp_hdr_rows); }
        });
        if self.ext.api.headers_raw {
            ui.label(RichText::new("One 'Name: value' per line; a leading '#' disables the line.").small().color(Color32::GRAY));
            if ui.add(egui::TextEdit::multiline(&mut self.ext.api.headers_raw_text).code_editor().desired_rows(5).desired_width(f32::INFINITY)).changed() {
                self.comp_hdr_rows = raw_to_rows(&self.ext.api.headers_raw_text);
            }
            true
        } else { false }
    }

    /// Body tab: Raw / GraphQL mode.
    pub fn api_composer_body(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let was = self.ext.api.graphql_mode;
            ui.radio_value(&mut self.ext.api.graphql_mode, false, "Raw");
            ui.radio_value(&mut self.ext.api.graphql_mode, true, "GraphQL");
            if !was && self.ext.api.graphql_mode {
                let g = detect_graphql(&self.comp_method, &self.comp_url, &crate::model::rows_to_headers(&self.comp_hdr_rows), &self.comp_body).unwrap_or_default();
                let st = &mut self.ext.api;
                st.gql_query = if g.query.is_empty() { "query {\n  \n}".into() } else { g.query };
                st.gql_vars = g.variables;
                st.gql_op = g.operation_name;
                if self.comp_method == "GET" { self.comp_method = "POST".into(); }
            }
        });
        if self.ext.api.graphql_mode {
            let st = &mut self.ext.api;
            ui.label("Query");
            ui.add(egui::TextEdit::multiline(&mut st.gql_query).code_editor().desired_rows(6).desired_width(f32::INFINITY));
            ui.horizontal(|ui| { ui.label("Operation name"); ui.add(egui::TextEdit::singleline(&mut st.gql_op).desired_width(180.0)); });
            ui.label("Variables (JSON object)");
            ui.add(egui::TextEdit::multiline(&mut st.gql_vars).code_editor().desired_rows(3).desired_width(f32::INFINITY));
            match build_graphql_body(&st.gql_query, &st.gql_vars, &st.gql_op) {
                Ok(b) => { self.comp_body = b; }
                Err(e) => { ui.label(RichText::new(e).color(Color32::LIGHT_RED).small()); }
            }
        } else {
            ui.add(egui::TextEdit::multiline(&mut self.comp_body).desired_rows(5).desired_width(f32::INFINITY));
            if let Some(g) = detect_graphql(&self.comp_method, &self.comp_url, &crate::model::rows_to_headers(&self.comp_hdr_rows), &self.comp_body) {
                ui.collapsing("GraphQL request detected", |ui| api_gql_view(ui, &g, "comp_gql_detect"));
            }
        }
    }

    /// Tests tab: editable assertion list.
    pub fn api_composer_tests(&mut self, ui: &mut egui::Ui) {
        let mut del = None;
        egui::Grid::new("comp_tests_grid").num_columns(5).striped(true).show(ui, |ui| {
            for (i, a) in self.ext.api.comp_tests.iter_mut().enumerate() {
                ui.checkbox(&mut a.enabled, "");
                egui::ComboBox::from_id_salt(format!("comp_test_kind{i}")).selected_text(a.kind.label()).width(140.0).show_ui(ui, |ui| {
                    for k in AssertKind::ALL { ui.selectable_value(&mut a.kind, k, k.label()); }
                });
                if a.kind.has_target() {
                    let hint = if matches!(a.kind, AssertKind::HeaderExists | AssertKind::HeaderEquals) { "header name" } else { "$.data.id" };
                    ui.add(egui::TextEdit::singleline(&mut a.target).hint_text(hint).desired_width(150.0));
                } else { ui.label(""); }
                if a.kind.has_expected() {
                    let hint = match a.kind { AssertKind::StatusEquals => "200, 2xx, 200-299", AssertKind::TimeBelow => "500", AssertKind::BodyMatches => "regex", _ => "expected" };
                    ui.add(egui::TextEdit::singleline(&mut a.expected).hint_text(hint).desired_width(160.0));
                } else { ui.label(""); }
                if ui.small_button("x").clicked() { del = Some(i); }
                ui.end_row();
            }
        });
        if let Some(i) = del { self.ext.api.comp_tests.remove(i); }
        ui.horizontal(|ui| {
            if ui.button("+ Add test").clicked() { self.ext.api.comp_tests.push(Assertion::new(AssertKind::StatusEquals, "", "200")); }
            if let Some(r) = &self.comp_resp {
                if ui.button("Re-run on last response").clicked() { self.ext.api.last_results = run_tests(&self.ext.api.comp_tests, r); }
            }
        });
        ui.label(RichText::new("Tests are saved with the request and run after every send.").small().color(Color32::GRAY));
    }

    /// Code tab: generated code for the current request (variables substituted).
    pub fn api_composer_code(&mut self, ui: &mut egui::Ui) {
        let sr = self.current_saved("");
        let req = prepare(&sr, &self.ext.api.active_vars());
        let s = crate::codegen::session_from_request(&req);
        let langs = crate::codegen::LANGUAGES;
        let idx = self.ext.api.code_lang.min(langs.len() - 1);
        let code = crate::codegen::generate(langs[idx], &s);
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("comp_code_lang").selected_text(langs[idx]).show_ui(ui, |ui| {
                for (i, l) in langs.iter().enumerate() { ui.selectable_value(&mut self.ext.api.code_lang, i, *l); }
            });
            if ui.button("Copy").clicked() { ui.ctx().copy_text(code.clone()); self.comp_msg = format!("{} code copied", langs[idx]); }
        });
        egui::ScrollArea::vertical().max_height(220.0).id_salt("comp_code_scroll").show(ui, |ui| {
            ui.add(egui::TextEdit::multiline(&mut code.as_str()).code_editor().desired_width(f32::INFINITY));
        });
    }

    /// Test results under the response.
    pub fn api_composer_results(&self, ui: &mut egui::Ui) {
        let rs = &self.ext.api.last_results;
        if rs.is_empty() { return; }
        let p = rs.iter().filter(|r| r.passed).count();
        let col = if p == rs.len() { Color32::from_rgb(90, 190, 90) } else { Color32::LIGHT_RED };
        ui.label(RichText::new(format!("Tests: {p}/{} passed", rs.len())).strong().color(col));
        for r in rs {
            let (mark, c) = if r.passed { ("PASS", Color32::from_rgb(90, 190, 90)) } else { ("FAIL", Color32::LIGHT_RED) };
            ui.horizontal(|ui| { ui.label(RichText::new(mark).monospace().color(c)); ui.label(&r.label); ui.label(RichText::new(&r.detail).small().color(Color32::GRAY)); });
        }
    }

    // ---------- Windows ----------

    fn api_envs_window(&mut self, ctx: &egui::Context) {
        if !self.ext.api.show_envs { return; }
        let mut open = true;
        let mut save = false;
        egui::Window::new("Environments").open(&mut open).default_width(480.0).show(ctx, |ui| {
            let st = &mut self.ext.api;
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut st.env_new).hint_text("new environment name").desired_width(180.0));
                if ui.button("Add").clicked() && !st.env_new.trim().is_empty() {
                    let n = st.env_new.trim().to_string();
                    if st.envs.get_mut(&n).is_none() { st.envs.envs.push(Environment { name: n.clone(), vars: vec![KvRow::new("baseUrl", "http://127.0.0.1:8080")] }); }
                    st.env_edit = n;
                    st.env_new.clear();
                    save = true;
                }
            });
            ui.horizontal_wrapped(|ui| {
                ui.label("Edit:");
                for e in &st.envs.envs { if ui.selectable_label(st.env_edit == e.name, &e.name).clicked() { st.env_edit = e.name.clone(); } }
            });
            ui.label(RichText::new(format!("Active: {}", if st.envs.active.is_empty() { "(none)" } else { &st.envs.active })).small());
            let edit = st.env_edit.clone();
            let mut delete = false;
            let mut activate = false;
            if let Some(e) = st.envs.get_mut(&edit) {
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&e.name).strong());
                    if ui.button("Set active").clicked() { activate = true; }
                    if ui.button("Delete").clicked() { delete = true; }
                });
                let before = e.vars.clone();
                let mut del = None;
                egui::Grid::new("env_vars_grid").num_columns(4).striped(true).show(ui, |ui| {
                    for (i, r) in e.vars.iter_mut().enumerate() {
                        ui.checkbox(&mut r.enabled, "");
                        ui.add(egui::TextEdit::singleline(&mut r.key).hint_text("variable").desired_width(140.0));
                        ui.add(egui::TextEdit::singleline(&mut r.value).hint_text("value").desired_width(220.0));
                        if ui.small_button("x").clicked() { del = Some(i); }
                        ui.end_row();
                    }
                });
                if let Some(i) = del { e.vars.remove(i); }
                if ui.button("+ Variable").clicked() { e.vars.push(KvRow::new("", "")); }
                if e.vars != before { save = true; }
                ui.label(RichText::new("Use {{name}} in the URL, params, headers or body.").small().color(Color32::GRAY));
            }
            if activate { st.envs.active = edit.clone(); save = true; }
            if delete {
                st.envs.envs.retain(|e| e.name != edit);
                if st.envs.active == edit { st.envs.active.clear(); }
                st.env_edit.clear();
                save = true;
            }
            if !st.msg.is_empty() { ui.label(RichText::new(&st.msg).small().color(Color32::GRAY)); }
        });
        self.ext.api.show_envs = open;
        if save { self.api_save_envs(); }
    }

    /// Start running collection `coll` with the active environment and Composer settings.
    pub fn api_run_start(&mut self, coll: &str, ctx: Option<egui::Context>) -> bool {
        self.comp_reload();
        let Some(c) = self.comp_collections.iter().find(|c| c.name == coll).cloned() else { self.ext.api.run_msg = format!("no collection '{coll}'"); return false };
        let st = &mut self.ext.api;
        st.run_results.clear();
        st.runner = Some(Runner::start(c.requests, st.active_vars(), st.settings.clone(), proxy_addr(), ctx));
        st.run_msg = format!("running '{}'...", c.name);
        true
    }

    /// Collect finished runner results (called each frame).
    pub fn api_run_poll(&mut self) {
        let st = &mut self.ext.api;
        let Some(r) = &st.runner else { return };
        st.run_results.extend(r.rx.try_iter());
        if r.is_done() {
            let stopped = r.stop.load(Ordering::SeqCst);
            if let Some(r) = st.runner.take() { st.run_results.extend(r.join()); }
            st.run_msg = format!("{}{}", if stopped { "stopped: " } else { "done: " }, run_summary(&st.run_results));
        }
    }

    fn api_runner_window(&mut self, ctx: &egui::Context) {
        self.api_run_poll();
        if !self.ext.api.show_runner { return; }
        if !self.comp_loaded { self.comp_reload(); }
        let mut open = true;
        let mut start = false;
        let colls = self.comp_collections.clone();
        egui::Window::new("Collection runner").open(&mut open).default_width(640.0).show(ctx, |ui| {
            let st = &mut self.ext.api;
            ui.horizontal(|ui| {
                ui.label("Collection:");
                Self::api_coll_combo(ui, "runner_coll", &colls, &mut st.runner_coll);
                ui.label(format!("Environment: {}", if st.envs.active.is_empty() { "(none)" } else { &st.envs.active }));
                let running = st.runner.is_some();
                if ui.add_enabled(!running && !st.runner_coll.is_empty(), egui::Button::new("Run")).clicked() { start = true; }
                if ui.add_enabled(running, egui::Button::new("Stop")).clicked() { if let Some(r) = &st.runner { r.request_stop(); } }
            });
            ui.label(RichText::new(format!("Sends {} (redirects {}, timeout {} s; change in the Composer)", if st.settings.via_proxy { "via the capture proxy" } else { "directly" }, if st.settings.follow_redirects { "followed" } else { "not followed" }, st.settings.timeout_secs)).small().color(Color32::GRAY));
            if let Some(r) = &st.runner { ui.label(format!("{} / {}", st.run_results.len(), r.total)); }
            if !st.run_msg.is_empty() { ui.label(RichText::new(&st.run_msg).strong()); }
            ui.separator();
            egui::ScrollArea::vertical().max_height(360.0).id_salt("runner_scroll").show(ui, |ui| {
                egui::Grid::new("runner_grid").num_columns(7).striped(true).show(ui, |ui| {
                    for h in ["#", "Request", "Status", "Time", "Size", "Passed", "Failed"] { ui.label(RichText::new(h).strong()); }
                    ui.end_row();
                    for (i, r) in st.run_results.iter().enumerate() {
                        ui.label((i + 1).to_string());
                        ui.label(format!("{} {}", r.method, r.name)).on_hover_text(&r.url);
                        if r.error.is_empty() { ui.label(r.status.to_string()); } else { ui.label(RichText::new("ERR").color(Color32::LIGHT_RED)).on_hover_text(&r.error); }
                        ui.label(format!("{} ms", r.elapsed_ms));
                        ui.label(format!("{} B", r.size));
                        ui.label(RichText::new(r.passed.to_string()).color(Color32::from_rgb(90, 190, 90)));
                        let fl = ui.label(RichText::new(r.failed.to_string()).color(if r.failed > 0 { Color32::LIGHT_RED } else { Color32::GRAY }));
                        let fails: Vec<String> = r.results.iter().filter(|x| !x.passed).map(|x| format!("{} ({})", x.label, x.detail)).collect();
                        if !fails.is_empty() { fl.on_hover_text(fails.join("\n")); }
                        ui.end_row();
                    }
                });
            });
        });
        self.ext.api.show_runner = open;
        if start { let c = self.ext.api.runner_coll.clone(); self.api_run_start(&c, Some(ctx.clone())); }
    }

    /// Save a new collection under a unique name and refresh the list.
    pub fn api_store_collection(&mut self, mut c: Collection) -> Result<String, String> {
        self.comp_reload();
        let base = c.name.clone();
        let mut i = 2;
        while self.comp_collections.iter().any(|x| crate::model::safe_file_name(&x.name) == crate::model::safe_file_name(&c.name)) { c.name = format!("{base} ({i})"); i += 1; }
        crate::model::save_collection(&self.comp_dir, &c)?;
        self.comp_reload();
        Ok(c.name)
    }

    /// Import a Postman or OpenAPI JSON file (detected from its content).
    pub fn api_import_file(&mut self, path: &str) -> Result<String, String> {
        let p = path.trim().trim_matches('"');
        let t = std::fs::read_to_string(p).map_err(|e| format!("read {p}: {e}"))?;
        let v: Value = serde_json::from_str(&t).map_err(|e| format!("not JSON: {e}"))?;
        if v.get("openapi").is_some() || v.get("swagger").is_some() {
            let c = from_openapi(&t)?;
            let n = c.requests.len();
            let name = self.api_store_collection(c)?;
            Ok(format!("imported OpenAPI as '{name}' ({n} requests)"))
        } else {
            let (c, vars) = from_postman(&t)?;
            let n = c.requests.len();
            let name = self.api_store_collection(c)?;
            if !vars.is_empty() {
                self.ext.api.envs.upsert(Environment { name: format!("{name} variables"), vars });
                self.api_save_envs();
            }
            Ok(format!("imported Postman collection as '{name}' ({n} requests)"))
        }
    }

    /// Export collection `coll` to `path` as "postman" or "openapi".
    pub fn api_export_file(&mut self, coll: &str, kind: &str, path: &str) -> Result<String, String> {
        self.comp_reload();
        let c = self.comp_collections.iter().find(|c| c.name == coll).cloned().ok_or(format!("no collection '{coll}'"))?;
        let v = if kind == "openapi" { to_openapi(&c) } else { to_postman(&c, &self.ext.api.active_vars()) };
        let p = path.trim().trim_matches('"');
        if p.is_empty() { return Err("enter a file path".into()); }
        std::fs::write(p, serde_json::to_string_pretty(&v).unwrap_or_default()).map_err(|e| format!("write {p}: {e}"))?;
        Ok(format!("exported '{coll}' to {p}"))
    }

    /// Build a collection from the selected session (only_selected) or all visible sessions.
    pub fn api_collection_from_traffic(&mut self, name: &str, only_selected: bool) -> Result<String, String> {
        let sessions = self.api_sessions(only_selected);
        if sessions.is_empty() { return Err(if only_selected { "no session selected".into() } else { "no visible sessions".into() }); }
        let refs: Vec<&SessionRecord> = sessions.iter().collect();
        let c = collection_from_sessions(if name.trim().is_empty() { "Captured traffic" } else { name.trim() }, &refs);
        let n = c.requests.len();
        let name = self.api_store_collection(c)?;
        Ok(format!("collection '{name}' created with {n} requests"))
    }

    fn api_io_window(&mut self, ctx: &egui::Context) {
        if !self.ext.api.show_io { return; }
        if !self.comp_loaded { self.comp_reload(); }
        let mut open = true;
        let mut act: Option<&'static str> = None;
        let colls = self.comp_collections.clone();
        egui::Window::new("API import / export").open(&mut open).default_width(560.0).show(ctx, |ui| {
            let st = &mut self.ext.api;
            ui.horizontal(|ui| { ui.label("File:"); ui.add(egui::TextEdit::singleline(&mut st.io_path).hint_text(r"C:\path\file.json").desired_width(380.0)); });
            if ui.button("Import file (Postman v2.1 or OpenAPI 3 JSON)").clicked() { act = Some("import"); }
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("Collection:");
                Self::api_coll_combo(ui, "io_coll", &colls, &mut st.io_coll);
                if ui.button("Export Postman").clicked() { act = Some("postman"); }
                if ui.button("Export OpenAPI").clicked() { act = Some("openapi"); }
            });
            ui.separator();
            ui.label(RichText::new("Build a collection from captured traffic").strong());
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut st.traffic_name).hint_text("Captured traffic").desired_width(180.0));
                if ui.button("From selected session").clicked() { act = Some("sel"); }
                if ui.button("From all visible sessions").clicked() { act = Some("vis"); }
            });
            if !st.io_msg.is_empty() { ui.label(RichText::new(&st.io_msg).small()); }
        });
        self.ext.api.show_io = open;
        if let Some(a) = act {
            let (p, c, n) = (self.ext.api.io_path.clone(), self.ext.api.io_coll.clone(), self.ext.api.traffic_name.clone());
            let r = match a {
                "import" => self.api_import_file(&p),
                "postman" | "openapi" => self.api_export_file(&c, a, &p),
                "sel" => self.api_collection_from_traffic(&n, true),
                _ => self.api_collection_from_traffic(&n, false),
            };
            self.ext.api.io_msg = match r { Ok(m) => m, Err(e) => format!("Error: {e}") };
        }
    }

    /// Start the mock server for collection `coll` on `port` (0 = random). Returns the port.
    pub fn api_mock_start(&mut self, coll: &str, port: u16) -> Result<u16, String> {
        self.api_mock_stop();
        self.comp_reload();
        let c = self.comp_collections.iter().find(|c| c.name == coll).cloned().ok_or(format!("no collection '{coll}'"))?;
        let routes = routes_from_collection(&c, &self.ext.api.active_vars());
        if routes.is_empty() { return Err("this collection has no saved responses (send a request, then save it; or import examples)".into()); }
        let m = MockServer::start(&format!("127.0.0.1:{port}"), routes.clone())?;
        let p = m.port;
        self.ext.api.mock = Some(m);
        self.ext.api.mock_routes = routes;
        Ok(p)
    }

    pub fn api_mock_stop(&mut self) {
        if let Some(mut m) = self.ext.api.mock.take() { m.stop(); }
        self.ext.api.mock_routes.clear();
    }

    fn api_mock_window(&mut self, ctx: &egui::Context) {
        if !self.ext.api.show_mock { return; }
        if !self.comp_loaded { self.comp_reload(); }
        let mut open = true;
        let (mut start, mut stop) = (false, false);
        let colls = self.comp_collections.clone();
        egui::Window::new("Mock server").open(&mut open).default_width(520.0).show(ctx, |ui| {
            let st = &mut self.ext.api;
            ui.horizontal(|ui| {
                ui.label("Collection:");
                Self::api_coll_combo(ui, "mock_coll", &colls, &mut st.mock_coll);
                ui.label("Port:");
                ui.add(egui::TextEdit::singleline(&mut st.mock_port).hint_text("8090").desired_width(60.0));
                let running = st.mock.is_some();
                if ui.add_enabled(!running, egui::Button::new("Start")).clicked() { start = true; }
                if ui.add_enabled(running, egui::Button::new("Stop")).clicked() { stop = true; }
            });
            match &st.mock {
                Some(m) => { ui.label(RichText::new(format!("Running on http://127.0.0.1:{}  ({} connections)", m.port, m.hits.load(Ordering::SeqCst))).strong().color(Color32::from_rgb(90, 190, 90))); }
                None => { ui.label(RichText::new("Stopped").color(Color32::GRAY)); }
            }
            ui.label(RichText::new("Answers each saved request's response by method + path; {{var}}, {id}, :id and * path segments match anything.").small().color(Color32::GRAY));
            if !st.mock_msg.is_empty() { ui.label(RichText::new(&st.mock_msg).small()); }
            egui::Grid::new("mock_routes").num_columns(3).striped(true).show(ui, |ui| {
                for r in &st.mock_routes { ui.label(&r.method); ui.label(&r.path); ui.label(r.status.to_string()); ui.end_row(); }
            });
        });
        self.ext.api.show_mock = open;
        if start {
            let c = self.ext.api.mock_coll.clone();
            let port = self.ext.api.mock_port.trim().parse::<u16>().unwrap_or(8090);
            self.ext.api.mock_msg = match self.api_mock_start(&c, port) { Ok(p) => format!("serving '{c}' on port {p}"), Err(e) => format!("Error: {e}") };
        }
        if stop { self.api_mock_stop(); self.ext.api.mock_msg = "stopped".into(); }
    }

    fn api_gql_window(&mut self, ctx: &egui::Context) {
        if !self.ext.api.show_gql { return; }
        let mut open = true;
        let sess = self.api_sessions(true).into_iter().next();
        let mut to_composer: Option<SavedRequest> = None;
        egui::Window::new("GraphQL view").open(&mut open).default_width(520.0).show(ctx, |ui| {
            let Some(s) = &sess else { ui.label("Select a session in the list."); return };
            ui.label(RichText::new(format!("#{} {} {}", s.id, s.method, s.url)).small());
            let hdrs: Vec<(String, String)> = s.request_headers.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            match detect_graphql(&s.method, &s.url, &hdrs, &s.request_body) {
                None => { ui.label(RichText::new("Not a GraphQL request (no JSON body with a \"query\" field).").color(Color32::GRAY)); }
                Some(g) => {
                    api_gql_view(ui, &g, "sess_gql");
                    if ui.button("Open in Composer (GraphQL mode)").clicked() {
                        if let Some(r) = collection_from_sessions("x", &[s]).requests.into_iter().next() { to_composer = Some(r); }
                    }
                }
            }
        });
        self.ext.api.show_gql = open;
        if let Some(mut r) = to_composer {
            r.body_mode = "graphql".into();
            r.example = None;
            if !self.ext.api.tabs.is_empty() { self.api_tab_new(); }
            self.apply_saved(&r);
            self.show_composer = true;
        }
    }
}
