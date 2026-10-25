//! G5/G12 rules: richer matching and actions (redirect, map, rewrite, block, templates, counters).
//! Owned by one builder; the app calls `rules_ext_menu` from the toolbar and `rules_ext_windows` every frame.
//!
//! The rule data lives in `model::Rule`; this module holds the condition / edit types, the matching
//! engine the proxy uses (`plan_request` / `plan_response`), per-rule hit counters, response templates,
//! rule group helpers and the client process lookup.
use std::collections::HashMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::model::{Rule, RuleResponse, SessionRecord};
use crate::ui::GridApp;

/// This module's UI state (kept in `GridApp::ext.rules`).
#[derive(Default)]
pub struct State {
    /// Group names collapsed in the rule list (UI only).
    pub collapsed: Vec<String>,
    /// Editor text buffers for the selected rule (rule id they belong to).
    pub edit_for: Option<u64>,
    pub req_hdr_text: String,
    pub resp_hdr_text: String,
    pub req_body_text: String,
    pub resp_body_text: String,
    /// G12 extras editor buffers (query / cookie edits).
    pub query_text: String,
    pub cookie_text: String,
    /// Rule set import / export file and mode.
    pub io_path: String,
    pub io_replace: bool,
}

impl GridApp {
    /// Toolbar entry (a button or a menu) for this module.
    pub fn rules_ext_menu(&mut self, _ui: &mut egui::Ui) {}

    /// Windows this module draws each frame.
    pub fn rules_ext_windows(&mut self, _ctx: &egui::Context) {}
}

fn yes() -> bool {
    true
}

// ---------------------------------------------------------------------------------------------
// Data types stored inside a Rule
// ---------------------------------------------------------------------------------------------

/// Extra match condition. `kind`: header, cookie, body, process, status, resp_header, resp_body.
/// `op`: contains, equals, regex, not_contains, exists. For `status` the value is a spec such as
/// `404`, `4xx`, `500-599` or a comma list of those.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct RuleCond {
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub kind: String,
    /// Header or cookie name (unused for body / process / status).
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub op: String,
    #[serde(default)]
    pub value: String,
}

/// Header edit: `op` add (append), set (replace / insert) or remove.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct HeaderEdit {
    #[serde(default)]
    pub op: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub value: String,
}

/// Body text replacement (plain text, or a regex with `$1` style captures).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct TextReplace {
    #[serde(default)]
    pub find: String,
    #[serde(default)]
    pub replace: String,
    #[serde(default)]
    pub regex: bool,
}

pub const ACTIONS: &[(&str, &str)] = &[
    ("canned", "Canned response"),
    ("file", "Map Local (file)"),
    ("folder", "Map Local (folder)"),
    ("redirect", "Redirect (301/302)"),
    ("map_remote", "Map Remote"),
    ("modify", "Pass through + modify"),
    ("block", "Block (403)"),
    ("close", "Close connection"),
    ("allow_only", "Allow list (block what doesn't match)"),
];

pub const COND_KINDS: &[&str] = &["header", "cookie", "body", "process", "status", "resp_header", "resp_body"];
pub const COND_OPS: &[&str] = &["contains", "equals", "regex", "not_contains", "exists"];

pub fn action_label(a: &str) -> &'static str {
    ACTIONS.iter().find(|(k, _)| *k == a).map(|(_, l)| *l).unwrap_or("Canned response")
}

/// True when the action answers locally / ends the exchange (no further rules are considered).
pub fn is_terminal(action: &str) -> bool {
    !matches!(action, "modify" | "map_remote" | "allow_only")
}

// ---------------------------------------------------------------------------------------------
// Matching
// ---------------------------------------------------------------------------------------------

/// What the proxy knows about a request when rules are evaluated.
#[derive(Debug, Clone, Default)]
pub struct ReqCtx<'a> {
    pub method: &'a str,
    pub url: &'a str,
    pub headers: &'a [(String, String)],
    pub body: &'a [u8],
    pub process: Option<&'a str>,
}

/// Response side, for response-stage rules.
#[derive(Debug, Clone, Default)]
pub struct RespCtx<'a> {
    pub status: u16,
    pub headers: &'a [(String, String)],
    pub body: &'a [u8],
}

/// Host part (without port) of an absolute URL.
pub fn url_host(url: &str) -> String {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let auth = rest.split(['/', '?', '#']).next().unwrap_or("");
    let auth = auth.rsplit_once('@').map(|(_, h)| h).unwrap_or(auth);
    if auth.starts_with('[') {
        return auth.split(']').next().unwrap_or("").trim_start_matches('[').to_ascii_lowercase();
    }
    auth.split(':').next().unwrap_or("").to_ascii_lowercase()
}

/// Host pattern: `*.example.com` (that domain and its subdomains), `=exact.host`, or a substring.
pub fn host_matches(pat: &str, host: &str) -> bool {
    let p = pat.trim().to_ascii_lowercase();
    if p.is_empty() {
        return true;
    }
    let h = host.to_ascii_lowercase();
    if let Some(suf) = p.strip_prefix("*.") {
        return h == suf || h.ends_with(&format!(".{suf}"));
    }
    if let Some(ex) = p.strip_prefix('=') {
        return h == ex;
    }
    h.contains(&p)
}

/// Status spec: `404`, `4xx`, `500-599`, comma lists; empty matches everything.
pub fn status_matches(spec: &str, code: u16) -> bool {
    let spec = spec.trim();
    if spec.is_empty() {
        return true;
    }
    spec.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).any(|s| {
        let sl = s.to_ascii_lowercase();
        if sl.len() == 3 && sl.ends_with("xx") {
            return sl.chars().next().and_then(|c| c.to_digit(10)).map(|d| code / 100 == d as u16).unwrap_or(false);
        }
        if let Some((a, b)) = sl.split_once('-') {
            if let (Ok(a), Ok(b)) = (a.trim().parse::<u16>(), b.trim().parse::<u16>()) {
                return code >= a && code <= b;
            }
        }
        sl.parse::<u16>().map(|v| v == code).unwrap_or(false)
    })
}

/// Compare one value with an operator (contains / equals are case-insensitive).
pub fn value_matches(op: &str, want: &str, have: Option<&str>) -> bool {
    match op {
        "exists" => have.is_some(),
        "not_contains" => !have.map(|h| h.to_lowercase().contains(&want.to_lowercase())).unwrap_or(false),
        "equals" => have.map(|h| h.trim().to_lowercase() == want.trim().to_lowercase()).unwrap_or(false),
        "regex" => match (have, regex::Regex::new(want)) {
            (Some(h), Ok(r)) => r.is_match(h),
            _ => false,
        },
        _ => have.map(|h| h.to_lowercase().contains(&want.to_lowercase())).unwrap_or(false),
    }
}

fn header_value(headers: &[(String, String)], name: &str) -> Option<String> {
    let vals: Vec<&str> = headers
        .iter()
        .filter(|(k, _)| k.trim().eq_ignore_ascii_case(name.trim()))
        .map(|(_, v)| v.as_str())
        .collect();
    if vals.is_empty() {
        None
    } else {
        Some(vals.join(", "))
    }
}

/// Value of one cookie from the request's Cookie header(s).
pub fn cookie_value(headers: &[(String, String)], name: &str) -> Option<String> {
    headers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("cookie"))
        .flat_map(|(_, v)| v.split(';'))
        .map(|p| match p.split_once('=') {
            Some((k, v)) => (k.trim().to_string(), v.trim().to_string()),
            None => (p.trim().to_string(), String::new()),
        })
        .find(|(k, _)| k == name.trim())
        .map(|(_, v)| v)
}

/// Does one condition hold? Response-side kinds are false when `resp` is None.
pub fn cond_matches(c: &RuleCond, req: &ReqCtx, resp: Option<&RespCtx>) -> bool {
    if !c.enabled {
        return true;
    }
    let op = if c.op.is_empty() { "contains" } else { c.op.as_str() };
    match c.kind.as_str() {
        "header" => value_matches(op, &c.value, header_value(req.headers, &c.name).as_deref()),
        "cookie" => value_matches(op, &c.value, cookie_value(req.headers, &c.name).as_deref()),
        "body" => value_matches(op, &c.value, Some(&String::from_utf8_lossy(req.body))),
        "process" => value_matches(op, &c.value, req.process),
        "status" => match resp {
            Some(r) => {
                let ok = status_matches(&c.value, r.status);
                if op == "not_contains" { !ok } else { ok }
            }
            None => false,
        },
        "resp_header" => match resp {
            Some(r) => value_matches(op, &c.value, header_value(r.headers, &c.name).as_deref()),
            None => false,
        },
        "resp_body" => match resp {
            Some(r) => value_matches(op, &c.value, Some(&String::from_utf8_lossy(r.body))),
            None => false,
        },
        _ => true,
    }
}

/// True when some active rule needs the client's process name (the proxy only looks it up then).
pub fn needs_process(rules: &[Rule]) -> bool {
    rules.iter().any(|r| r.enabled && !r.group_off && r.conds.iter().any(|c| c.enabled && c.kind == "process"))
}

pub fn is_response_rule(r: &Rule) -> bool {
    r.stage == "response"
}

/// Case-insensitive glob match of the whole string: `*` matches any run (including empty), `?`
/// matches one character. Used by the auto-responder's "glob" URL match kind.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut mark): (Option<usize>, usize) = (None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Full match of a rule: enabled (and its group), URL pattern, method, host, every condition.
pub fn rule_matches(r: &Rule, req: &ReqCtx, resp: Option<&RespCtx>) -> bool {
    if !r.enabled || r.group_off {
        return false;
    }
    let has_other = !r.host.trim().is_empty() || r.conds.iter().any(|c| c.enabled);
    if r.pattern.is_empty() && !has_other {
        return false;
    }
    if !r.method.is_empty() && !r.method.eq_ignore_ascii_case(req.method) {
        return false;
    }
    if !r.pattern.is_empty() {
        let ok = match r.match_kind.as_str() {
            "regex" => regex::Regex::new(&r.pattern).map(|x| x.is_match(req.url)).unwrap_or(false),
            "glob" => glob_match(&r.pattern, req.url),
            _ => req.url.contains(&r.pattern),
        };
        if !ok {
            return false;
        }
    }
    if !host_matches(&r.host, &url_host(req.url)) {
        return false;
    }
    r.conds.iter().all(|c| cond_matches(c, req, resp))
}

// ---------------------------------------------------------------------------------------------
// Hit counters
// ---------------------------------------------------------------------------------------------

static HITS: std::sync::OnceLock<Mutex<HashMap<u64, u64>>> = std::sync::OnceLock::new();
static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn hits() -> &'static Mutex<HashMap<u64, u64>> {
    HITS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A fresh, process-unique rule id (time based so ids from earlier runs are unlikely to collide).
pub fn new_rule_id() -> u64 {
    let base = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(1);
    let n = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    base.wrapping_add(n.wrapping_mul(7919)).max(1)
}

/// Give every rule without an id (old rule files) or with a duplicated id a unique one.
pub fn ensure_ids(rules: &mut [Rule]) {
    let mut seen = std::collections::HashSet::new();
    for r in rules.iter_mut() {
        if r.id == 0 || !seen.insert(r.id) {
            r.id = new_rule_id();
            seen.insert(r.id);
        }
    }
}

pub fn record_hit(id: u64) {
    if let Ok(mut m) = hits().lock() {
        *m.entry(id).or_insert(0) += 1;
    }
}

pub fn rule_hits(id: u64) -> u64 {
    hits().lock().ok().and_then(|m| m.get(&id).copied()).unwrap_or(0)
}

pub fn reset_hits(id: u64) {
    if let Ok(mut m) = hits().lock() {
        m.remove(&id);
    }
}

pub fn reset_hits_for(rules: &[Rule]) {
    for r in rules {
        reset_hits(r.id);
    }
}

// ---------------------------------------------------------------------------------------------
// Plans (what the proxy must do)
// ---------------------------------------------------------------------------------------------

/// Result of evaluating the request-stage rules.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReqPlan {
    /// Names of the rules that matched, in order.
    pub applied: Vec<String>,
    pub delay_ms: u64,
    /// Answer locally with this response.
    pub response: Option<RuleResponse>,
    /// Drop the client connection without a response.
    pub close: bool,
    /// Map Remote: forward to this URL instead.
    pub new_url: Option<String>,
    pub req_headers: Vec<HeaderEdit>,
    pub req_body: Vec<TextReplace>,
    /// Edits to apply later to the upstream response.
    pub resp_headers: Vec<HeaderEdit>,
    pub resp_body: Vec<TextReplace>,
    /// Query edits changed the URL (same host): forward to this URL.
    pub query_url: Option<String>,
    /// Response status override, bandwidth limit (kbit/s, 0 = none) and drop chance (%).
    pub resp_status: Option<u16>,
    pub kbps: u32,
    pub loss_pct: u8,
}

/// Result of evaluating the response-stage rules.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RespPlan {
    pub applied: Vec<String>,
    pub delay_ms: u64,
    pub response: Option<RuleResponse>,
    pub close: bool,
    pub resp_headers: Vec<HeaderEdit>,
    pub resp_body: Vec<TextReplace>,
    pub resp_status: Option<u16>,
    pub kbps: u32,
    pub loss_pct: u8,
}

/// Fold the throttle values of a rule into a plan's (strictest wins).
fn throttle_into(kbps: &mut u32, loss: &mut u8, m: &crate::rules_more::RuleExtras) {
    if m.kbps > 0 && (*kbps == 0 || m.kbps < *kbps) {
        *kbps = m.kbps;
    }
    *loss = (*loss).max(m.loss_pct.min(100));
}

/// Map Remote target URL for `url`. A regex rule may use `$1` captures in the target; a target with
/// no path (`http://host:port`) keeps the original path and query; otherwise the target replaces the URL.
pub fn map_remote_url(r: &Rule, url: &str) -> String {
    let t = r.target.trim();
    if r.match_kind == "regex" && t.contains('$') {
        if let Ok(re) = regex::Regex::new(&r.pattern) {
            return re.replace(url, t).to_string();
        }
    }
    let (scheme, rest) = match t.split_once("://") {
        Some(x) => x,
        None => ("http", t),
    };
    let host = rest.trim_end_matches('/');
    if host.contains('/') || host.contains('?') {
        return format!("{scheme}://{rest}");
    }
    let orig_rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let pq = orig_rest.find(['/', '?']).map(|i| &orig_rest[i..]).unwrap_or("/");
    format!("{scheme}://{host}{pq}")
}

/// Local response for a terminal action (canned / file / redirect / block).
pub fn terminal_response(r: &Rule) -> RuleResponse {
    match r.action.as_str() {
        "redirect" => {
            let status = if (300..400).contains(&r.status) { r.status } else { 302 };
            let mut headers = vec![("Location".to_string(), r.target.trim().to_string())];
            headers.extend(
                r.headers
                    .iter()
                    .filter(|h| h.enabled && !h.key.trim().is_empty() && !h.key.trim().eq_ignore_ascii_case("location"))
                    .map(|h| (h.key.trim().to_string(), h.value.clone())),
            );
            if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type")) {
                headers.push(("Content-Type".into(), "text/plain; charset=utf-8".into()));
            }
            RuleResponse { name: r.name.clone(), status, headers, body: format!("Redirecting to {}", r.target.trim()).into_bytes() }
        }
        "block" => RuleResponse {
            name: r.name.clone(),
            status: 403,
            headers: vec![("Content-Type".into(), "text/plain; charset=utf-8".into())],
            body: format!("Blocked by Ferrotap rule '{}'", r.name).into_bytes(),
        },
        _ => crate::model::rule_response(r),
    }
}

/// Evaluate the request-stage rules in order. Terminal actions and rules marked "stop" end processing.
pub fn plan_request(rules: &[Rule], req: &ReqCtx) -> ReqPlan {
    let mut p = ReqPlan::default();
    let mut url = req.url.to_string();
    // Allow list: when any allow-only rule is active, a request must match one of them.
    let allow: Vec<&Rule> = rules.iter().filter(|r| r.action == "allow_only" && !is_response_rule(r) && r.enabled && !r.group_off).collect();
    if !allow.is_empty() {
        match allow.iter().find(|r| rule_matches(r, req, None)) {
            Some(r) => record_hit(r.id),
            None => {
                p.applied.push("allow list".into());
                p.response = Some(crate::rules_more::not_allowed_response());
                return p;
            }
        }
    }
    for r in rules.iter().filter(|r| !is_response_rule(r) && r.action != "allow_only") {
        let cx = ReqCtx { url: &url, ..req.clone() };
        if !rule_matches(r, &cx, None) {
            continue;
        }
        record_hit(r.id);
        p.applied.push(r.name.clone());
        p.delay_ms = p.delay_ms.saturating_add(r.delay_ms);
        match r.action.as_str() {
            "close" => {
                p.close = true;
                return p;
            }
            "modify" | "map_remote" => {
                if r.action == "map_remote" && !r.target.trim().is_empty() {
                    url = map_remote_url(r, &url);
                    p.new_url = Some(url.clone());
                    p.query_url = None;
                }
                if !r.more.query.is_empty() {
                    url = crate::rules_more::edit_query(&url, &r.more.query);
                    if p.new_url.is_some() { p.new_url = Some(url.clone()) } else { p.query_url = Some(url.clone()) }
                }
                p.req_headers.extend(r.req_headers.iter().cloned());
                p.req_headers.extend(crate::rules_more::request_edits(&r.more, req.headers));
                p.req_body.extend(r.req_body.iter().cloned());
                p.resp_headers.extend(r.resp_headers.iter().cloned());
                p.resp_headers.extend(crate::rules_more::response_edits(&r.more));
                p.resp_body.extend(r.resp_body.iter().cloned());
                if r.more.status > 0 {
                    p.resp_status = Some(r.more.status);
                }
                throttle_into(&mut p.kbps, &mut p.loss_pct, &r.more);
                if r.stop {
                    return p;
                }
            }
            "folder" => {
                p.response = Some(crate::rules_more::folder_response(r, &url));
                return p;
            }
            _ => {
                p.response = Some(terminal_response(r));
                return p;
            }
        }
    }
    p
}

/// Evaluate the response-stage rules (they can also test the status / response headers / body).
pub fn plan_response(rules: &[Rule], req: &ReqCtx, resp: &RespCtx) -> RespPlan {
    let mut p = RespPlan::default();
    for r in rules.iter().filter(|r| is_response_rule(r)) {
        if !rule_matches(r, req, Some(resp)) {
            continue;
        }
        record_hit(r.id);
        p.applied.push(r.name.clone());
        p.delay_ms = p.delay_ms.saturating_add(r.delay_ms);
        match r.action.as_str() {
            "close" => {
                p.close = true;
                return p;
            }
            "modify" | "map_remote" => {
                p.resp_headers.extend(r.resp_headers.iter().cloned());
                p.resp_headers.extend(crate::rules_more::response_edits(&r.more));
                p.resp_body.extend(r.resp_body.iter().cloned());
                if r.more.status > 0 {
                    p.resp_status = Some(r.more.status);
                }
                throttle_into(&mut p.kbps, &mut p.loss_pct, &r.more);
                if r.stop {
                    return p;
                }
            }
            "allow_only" => {}
            "folder" => {
                p.response = Some(crate::rules_more::folder_response(r, req.url));
                return p;
            }
            _ => {
                p.response = Some(terminal_response(r));
                return p;
            }
        }
    }
    p
}

// ---------------------------------------------------------------------------------------------
// Edits
// ---------------------------------------------------------------------------------------------

/// Apply header edits to an ordered header list.
pub fn apply_header_edits(headers: &mut Vec<(String, String)>, edits: &[HeaderEdit]) {
    for e in edits {
        let name = e.name.trim();
        if name.is_empty() {
            continue;
        }
        match e.op.as_str() {
            "remove" => headers.retain(|(k, _)| !k.eq_ignore_ascii_case(name)),
            "add" => headers.push((name.to_string(), e.value.clone())),
            _ => {
                // set / replace: first occurrence gets the value, later duplicates are dropped.
                if let Some(pos) = headers.iter().position(|(k, _)| k.eq_ignore_ascii_case(name)) {
                    headers[pos].1 = e.value.clone();
                    let mut i = pos + 1;
                    while i < headers.len() {
                        if headers[i].0.eq_ignore_ascii_case(name) {
                            headers.remove(i);
                        } else {
                            i += 1;
                        }
                    }
                } else {
                    headers.push((name.to_string(), e.value.clone()));
                }
            }
        }
    }
}

/// Apply text replacements to a (UTF-8 lossy) body. Returns None when nothing changed.
pub fn apply_body_replace(body: &[u8], edits: &[TextReplace]) -> Option<Vec<u8>> {
    if edits.iter().all(|e| e.find.is_empty()) {
        return None;
    }
    let orig = String::from_utf8_lossy(body).to_string();
    let mut s = orig.clone();
    for e in edits.iter().filter(|e| !e.find.is_empty()) {
        if e.regex {
            if let Ok(re) = regex::Regex::new(&e.find) {
                s = re.replace_all(&s, e.replace.as_str()).to_string();
            }
        } else {
            s = s.replace(&e.find, &e.replace);
        }
    }
    if s == orig {
        None
    } else {
        Some(s.into_bytes())
    }
}

/// Decode a body by its Content-Encoding (for rewriting). None if it is not encoded or cannot be decoded.
pub fn decode_for_edit(body: &[u8], content_encoding: Option<&str>) -> Option<Vec<u8>> {
    let ce = content_encoding?.trim();
    if ce.is_empty() || ce.eq_ignore_ascii_case("identity") {
        return None;
    }
    let codings = crate::decode::parse_content_encodings(ce).ok()?;
    let limits = crate::decode::DecoderLimits::default();
    let mut data = body.to_vec();
    for c in codings.into_iter().rev() {
        data = crate::decode::decompress_single(&data, c, &limits).ok()?;
    }
    Some(data)
}

// ---------------------------------------------------------------------------------------------
// Process lookup (Windows): which local process owns the client's TCP socket
// ---------------------------------------------------------------------------------------------

static PID_NAMES: std::sync::OnceLock<Mutex<HashMap<u32, String>>> = std::sync::OnceLock::new();

#[cfg(windows)]
fn hidden(cmd: &mut std::process::Command) -> &mut std::process::Command {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x0800_0000) // CREATE_NO_WINDOW
}

#[cfg(not(windows))]
fn hidden(cmd: &mut std::process::Command) -> &mut std::process::Command {
    cmd
}

/// PID owning the local TCP endpoint `addr` (the client's side of a loopback connection), via netstat.
pub fn pid_for_local_addr(addr: &std::net::SocketAddr) -> Option<u32> {
    let out = hidden(std::process::Command::new("netstat").args(["-ano", "-p", "TCP"])).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let want = addr.to_string();
    let want_v4 = match addr {
        std::net::SocketAddr::V6(v6) => v6.ip().to_ipv4_mapped().map(|ip| format!("{ip}:{}", v6.port())),
        _ => None,
    };
    for line in text.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() >= 5 && cols[0].eq_ignore_ascii_case("TCP") && (cols[1] == want || Some(cols[1].to_string()) == want_v4) {
            if let Ok(pid) = cols[cols.len() - 1].parse::<u32>() {
                if pid != 0 {
                    return Some(pid);
                }
            }
        }
    }
    None
}

/// Image name for a PID (cached), via tasklist.
pub fn process_name(pid: u32) -> Option<String> {
    let cache = PID_NAMES.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(n) = cache.lock().ok().and_then(|m| m.get(&pid).cloned()) {
        return Some(n);
    }
    let filter = format!("PID eq {pid}");
    let out = hidden(std::process::Command::new("tasklist").args(["/FI", filter.as_str(), "/FO", "CSV", "/NH"])).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().find(|l| l.starts_with('"'))?;
    let name = line.split("\",\"").next()?.trim_matches('"').to_string();
    if name.is_empty() {
        return None;
    }
    if let Ok(mut m) = cache.lock() {
        m.insert(pid, name.clone());
    }
    Some(name)
}

/// Process name of the client connected from `addr` (only local clients can be resolved).
pub fn client_process(addr: &std::net::SocketAddr) -> Option<String> {
    crate::procinfo::lookup(addr).map(|(_, n)| n).or_else(|| pid_for_local_addr(addr).and_then(process_name))
}

// ---------------------------------------------------------------------------------------------
// Templates, duplicate, groups, rule from session
// ---------------------------------------------------------------------------------------------

/// A predefined response: (name, status, headers, body).
pub type Template = (&'static str, u16, Vec<(&'static str, &'static str)>, &'static str);

/// Predefined responses for the rule editor.
pub fn templates() -> Vec<Template> {
    let json = vec![("Content-Type", "application/json")];
    let text = vec![("Content-Type", "text/plain; charset=utf-8")];
    vec![
        ("200 OK (JSON)", 200, json.clone(), "{\"ok\": true}"),
        ("204 No Content", 204, vec![], ""),
        ("301 Moved Permanently", 301, vec![("Location", "https://example.com/")], ""),
        ("302 Found", 302, vec![("Location", "https://example.com/")], ""),
        ("401 Unauthorized", 401, vec![("Content-Type", "application/json"), ("WWW-Authenticate", "Bearer")], "{\"error\": \"unauthorized\"}"),
        ("403 Forbidden", 403, text.clone(), "Forbidden"),
        ("404 Not Found", 404, text.clone(), "Not Found"),
        ("500 Internal Server Error", 500, text.clone(), "Internal Server Error"),
        ("502 Bad Gateway", 502, text.clone(), "Bad Gateway"),
        ("503 Service Unavailable", 503, vec![("Content-Type", "text/plain; charset=utf-8"), ("Retry-After", "120")], "Service Unavailable"),
        (
            "CORS preflight OK",
            204,
            vec![
                ("Access-Control-Allow-Origin", "*"),
                ("Access-Control-Allow-Methods", "GET, POST, PUT, PATCH, DELETE, OPTIONS"),
                ("Access-Control-Allow-Headers", "*"),
                ("Access-Control-Max-Age", "86400"),
            ],
            "",
        ),
    ]
}

/// Fill a rule's response from a template (by name). Returns false if the name is unknown.
pub fn apply_template(r: &mut Rule, name: &str) -> bool {
    let Some((_, status, headers, body)) = templates().into_iter().find(|t| t.0 == name) else {
        return false;
    };
    r.action = "canned".into();
    r.status = status;
    r.headers = headers
        .into_iter()
        .map(|(k, v)| crate::model::KvRow { enabled: true, key: k.into(), value: v.into() })
        .collect();
    r.body = body.into();
    if name.starts_with("CORS") && r.method.is_empty() {
        r.method = "OPTIONS".into();
    }
    true
}

/// Copy of a rule with a new id and a "(copy)" name.
pub fn duplicate_rule(r: &Rule) -> Rule {
    let mut c = r.clone();
    c.id = new_rule_id();
    c.name = format!("{} (copy)", r.name);
    c
}

/// Tag of an exported rule set file.
pub const RULESET_FORMAT: &str = "ferrotap-rules/1";

/// A rule set as JSON: `{"format": "ferrotap-rules/1", "rules": [...]}`.
pub fn rules_to_json(rules: &[Rule]) -> String {
    serde_json::to_string_pretty(&serde_json::json!({ "format": RULESET_FORMAT, "rules": rules })).unwrap_or_default()
}

/// Rules from a rule set file (our object form or a bare JSON array, like rules.json).
/// Every rule gets a fresh id so imports never collide with existing rules.
pub fn rules_from_json(text: &str) -> Result<Vec<Rule>, String> {
    let v: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| format!("not valid JSON: {e}"))?;
    let list = match &v {
        serde_json::Value::Array(_) => v.clone(),
        serde_json::Value::Object(o) => o.get("rules").cloned().ok_or("no \"rules\" list in this file")?,
        _ => return Err("expected a list of rules".into()),
    };
    let mut rules: Vec<Rule> = serde_json::from_value(list).map_err(|e| format!("bad rule: {e}"))?;
    for r in rules.iter_mut() {
        r.id = new_rule_id();
    }
    Ok(rules)
}

pub fn export_rule_set(path: &std::path::Path, rules: &[Rule]) -> Result<usize, String> {
    if let Some(d) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(d).map_err(|e| format!("create {}: {e}", d.display()))?;
    }
    std::fs::write(path, rules_to_json(rules)).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(rules.len())
}

/// Import a rule set file, or every .json rule set in a folder (sorted by name).
pub fn import_rule_set(path: &std::path::Path) -> Result<Vec<Rule>, String> {
    let read = |p: &std::path::Path| -> Result<Vec<Rule>, String> {
        let t = std::fs::read_to_string(p).map_err(|e| format!("read {}: {e}", p.display()))?;
        if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("farx")) {
            // Fiddler Classic AutoResponder export
            return crate::farx::parse_farx(&t).map_err(|e| format!("{}: {e}", p.display()));
        }
        rules_from_json(&t).map_err(|e| format!("{}: {e}", p.display()))
    };
    if !path.is_dir() {
        return read(path);
    }
    let mut files: Vec<_> = std::fs::read_dir(path)
        .map_err(|e| format!("read {}: {e}", path.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("json") || x.eq_ignore_ascii_case("farx")))
        .collect();
    files.sort();
    let mut out = Vec::new();
    for f in files {
        out.extend(read(&f)?);
    }
    Ok(out)
}

/// Group names in first-seen order ("" = ungrouped).
/// Groups nest with '/': "api/mocks" is inside "api". Ungrouped ("") first, then every group and
/// its parents, parents before children.
pub fn group_names(rules: &[Rule]) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    if rules.iter().any(|r| r.group.is_empty()) {
        v.push(String::new());
    }
    let mut named: Vec<String> = Vec::new();
    for r in rules.iter().filter(|r| !r.group.is_empty()) {
        let parts: Vec<&str> = r.group.split('/').collect();
        for i in 1..=parts.len() {
            let g = parts[..i].join("/");
            if !named.contains(&g) {
                named.push(g);
            }
        }
    }
    named.sort_by(|a, b| a.split('/').cmp(b.split('/')));
    v.extend(named);
    v
}

/// `rule_group` is `group` or nested inside it.
pub fn in_group(rule_group: &str, group: &str) -> bool {
    rule_group == group || (!group.is_empty() && rule_group.starts_with(group) && rule_group[group.len()..].starts_with('/'))
}

/// Nesting depth of a group name (0 for top level).
pub fn group_depth(group: &str) -> usize {
    group.matches('/').count()
}

/// Enable / disable a whole group and its sub-groups (keeps each rule's own enabled flag).
pub fn set_group_enabled(rules: &mut [Rule], group: &str, on: bool) {
    for r in rules.iter_mut().filter(|r| in_group(&r.group, group)) {
        r.group_off = !on;
    }
}

pub fn group_enabled(rules: &[Rule], group: &str) -> bool {
    !rules.iter().filter(|r| in_group(&r.group, group)).all(|r| r.group_off)
}

/// Copies of every rule in `group` (sub-groups included) under a new "<group> (copy)" name.
pub fn duplicate_group(rules: &[Rule], group: &str) -> Vec<Rule> {
    let existing = group_names(rules);
    let mut name = format!("{group} (copy)");
    let mut n = 2;
    while existing.contains(&name) {
        name = format!("{group} (copy {n})");
        n += 1;
    }
    rules
        .iter()
        .filter(|r| in_group(&r.group, group))
        .map(|r| {
            let mut c = r.clone();
            c.id = new_rule_id();
            c.group = format!("{name}{}", &r.group[group.len()..]);
            c
        })
        .collect()
}

/// Prefill a rule from a captured session: exact URL + method, and its response as canned answer.
pub fn rule_from_session(s: &SessionRecord) -> Rule {
    let mut r = Rule::default();
    r.id = new_rule_id();
    r.name = format!("{} {}", s.method, s.url.chars().take(60).collect::<String>());
    r.match_kind = "regex".into();
    r.pattern = format!("^{}$", regex::escape(&s.url));
    r.method = s.method.clone();
    r.status = if s.status >= 100 { s.status } else { 200 };
    let skip = ["content-length", "transfer-encoding", "content-encoding", "connection"];
    r.headers = s
        .response_headers
        .iter()
        .filter(|(k, _)| !skip.contains(&k.to_ascii_lowercase().as_str()))
        .map(|(k, v)| crate::model::KvRow { enabled: true, key: k.clone(), value: v.clone() })
        .collect();
    r.body = s.response_body.clone();
    r
}

/// One-line summary of a rule's match and action for the list.
pub fn rule_summary(r: &Rule) -> String {
    let meth = if r.method.is_empty() { "ANY" } else { r.method.as_str() };
    let pat = if r.pattern.is_empty() { "*".to_string() } else { format!("{} {}", r.match_kind, r.pattern) };
    let mut s = format!("[{meth} {pat}");
    if !r.host.trim().is_empty() {
        s.push_str(&format!(" host={}", r.host.trim()));
    }
    for c in r.conds.iter().filter(|c| c.enabled) {
        s.push_str(&format!(" {}{}{} {}", c.kind, if c.name.is_empty() { "" } else { ":" }, c.name, c.op));
        if !c.value.is_empty() {
            s.push_str(&format!(" '{}'", c.value));
        }
    }
    s.push_str(&format!("] {}-> {}", if is_response_rule(r) { "(response) " } else { "" }, action_label(&r.action)));
    if r.delay_ms > 0 {
        s.push_str(&format!(" +{} ms", r.delay_ms));
    }
    if r.stop {
        s.push_str(" (stop)");
    }
    s
}

// Line based text formats for the editor ("set Name: value" / "find => replace").

pub fn header_edits_to_text(v: &[HeaderEdit]) -> String {
    v.iter()
        .map(|e| {
            if e.op == "remove" {
                format!("remove {}", e.name)
            } else {
                format!("{} {}: {}", if e.op.is_empty() { "set" } else { e.op.as_str() }, e.name, e.value)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn header_edits_from_text(t: &str) -> Vec<HeaderEdit> {
    t.lines()
        .filter_map(|l| {
            let l = l.trim();
            let (op, rest) = l.split_once(char::is_whitespace)?;
            let op = op.to_ascii_lowercase();
            if !matches!(op.as_str(), "add" | "set" | "remove") {
                return None;
            }
            let (name, value) = match rest.split_once(':') {
                Some((n, v)) => (n.trim().to_string(), v.trim().to_string()),
                None => (rest.trim().to_string(), String::new()),
            };
            if name.is_empty() {
                return None;
            }
            Some(HeaderEdit { op, name, value })
        })
        .collect()
}

pub fn replaces_to_text(v: &[TextReplace]) -> String {
    v.iter()
        .map(|e| format!("{}{} => {}", if e.regex { "re:" } else { "" }, e.find, e.replace))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn replaces_from_text(t: &str) -> Vec<TextReplace> {
    t.lines()
        .filter_map(|l| {
            let (f, r) = l.split_once(" => ")?;
            let (regex, f) = match f.strip_prefix("re:") {
                Some(x) => (true, x),
                None => (false, f),
            };
            if f.is_empty() {
                return None;
            }
            Some(TextReplace { find: f.to_string(), replace: r.to_string(), regex })
        })
        .collect()
}
