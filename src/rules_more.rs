//! G12 rule extras on top of rules_ext: query / cookie rewrites, status override, user-agent switcher,
//! "disable caching", "disable cookies", throttling (bandwidth, packet loss, presets), Map Local
//! for a whole folder, and allow-list rules. Pure helpers; the proxy applies the resulting plans.
use serde::{Deserialize, Serialize};

use crate::model::{Rule, RuleResponse};
use crate::rules_ext::HeaderEdit;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(default)]
pub struct RuleExtras {
    /// Query parameter edits: op "set" / "add" / "remove".
    pub query: Vec<HeaderEdit>,
    /// Request cookie edits: op "set" / "remove".
    pub cookies: Vec<HeaderEdit>,
    /// Replace the response status (0 = keep).
    pub status: u16,
    /// Replace the User-Agent ("" = keep).
    pub user_agent: String,
    /// Strip conditional / cache headers so every request reaches the server and nothing is cached.
    pub no_cache: bool,
    /// Drop Cookie and Set-Cookie headers.
    pub no_cookies: bool,
    /// Download bandwidth limit in kilobits per second (0 = unlimited).
    pub kbps: u32,
    /// Chance (0-100 %) to drop the connection instead of answering.
    pub loss_pct: u8,
}

impl RuleExtras {
    pub fn is_empty(&self) -> bool {
        *self == RuleExtras::default()
    }
}

/// Network presets: (name, added latency ms, download kbit/s, loss %).
pub const THROTTLE_PRESETS: &[(&str, u64, u32, u8)] = &[
    ("No throttling", 0, 0, 0),
    ("3G", 400, 750, 0),
    ("4G", 70, 9_000, 0),
    ("Slow Wi-Fi", 150, 1_500, 1),
    ("Bad network", 600, 250, 10),
    ("Offline", 0, 0, 100),
];

/// Common User-Agent strings for the switcher.
pub const USER_AGENTS: &[(&str, &str)] = &[
    ("Chrome (Windows)", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36"),
    ("Firefox (Windows)", "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:143.0) Gecko/20100101 Firefox/143.0"),
    ("Safari (iPhone)", "Mozilla/5.0 (iPhone; CPU iPhone OS 18_6 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.6 Mobile/15E148 Safari/604.1"),
    ("Chrome (Android)", "Mozilla/5.0 (Linux; Android 15; Pixel 9) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Mobile Safari/537.36"),
    ("Googlebot", "Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)"),
    ("curl", "curl/8.10.1"),
];

// ------------------------------------------------------------------------------------------ text forms

/// "set name=value" / "add name=value" / "remove name" lines.
pub fn kv_edits_from_text(t: &str) -> Vec<HeaderEdit> {
    t.lines()
        .filter_map(|l| {
            let l = l.trim();
            let (op, rest) = l.split_once(char::is_whitespace)?;
            let op = op.to_ascii_lowercase();
            if !matches!(op.as_str(), "set" | "add" | "remove") {
                return None;
            }
            let (name, value) = match rest.split_once('=') {
                Some((n, v)) => (n.trim(), v.trim()),
                None => (rest.trim(), ""),
            };
            (!name.is_empty()).then(|| HeaderEdit { op, name: name.to_string(), value: value.to_string() })
        })
        .collect()
}

pub fn kv_edits_to_text(v: &[HeaderEdit]) -> String {
    v.iter()
        .map(|e| if e.op == "remove" { format!("remove {}", e.name) } else { format!("{} {}={}", e.op, e.name, e.value) })
        .collect::<Vec<_>>()
        .join("\n")
}

// ------------------------------------------------------------------------------------------ query

fn pct(s: &str) -> String {
    crate::model::pct_encode(s)
}

/// Apply query parameter edits to a URL (the fragment-free URL keeps its order otherwise).
pub fn edit_query(url: &str, edits: &[HeaderEdit]) -> String {
    if edits.is_empty() {
        return url.to_string();
    }
    let (base, query) = match url.split_once('?') {
        Some((b, q)) => (b, q),
        None => (url, ""),
    };
    let mut pairs: Vec<(String, String)> = query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| match p.split_once('=') {
            Some((k, v)) => (k.to_string(), v.to_string()),
            None => (p.to_string(), String::new()),
        })
        .collect();
    let dec = |s: &str| crate::inspect::url_decode(s, true);
    for e in edits {
        let name = e.name.trim();
        match e.op.as_str() {
            "remove" => pairs.retain(|(k, _)| dec(k) != name),
            "add" => pairs.push((pct(name), pct(&e.value))),
            _ => {
                if let Some(i) = pairs.iter().position(|(k, _)| dec(k) == name) {
                    pairs[i].1 = pct(&e.value);
                    let mut n = 0;
                    pairs.retain(|(k, _)| {
                        if dec(k) == name {
                            n += 1;
                            n == 1
                        } else {
                            true
                        }
                    });
                } else {
                    pairs.push((pct(name), pct(&e.value)));
                }
            }
        }
    }
    if pairs.is_empty() {
        return base.to_string();
    }
    let q: Vec<String> = pairs.into_iter().map(|(k, v)| if v.is_empty() { k } else { format!("{k}={v}") }).collect();
    format!("{base}?{}", q.join("&"))
}

// ------------------------------------------------------------------------------------------ headers

/// New Cookie header value after edits (None = no cookies left, remove the header).
pub fn edit_cookie_header(current: Option<&str>, edits: &[HeaderEdit]) -> Option<String> {
    let mut list: Vec<(String, String)> = current.map(crate::inspect::parse_cookie_header).unwrap_or_default();
    for e in edits {
        let name = e.name.trim();
        match e.op.as_str() {
            "remove" => list.retain(|(k, _)| k != name),
            _ => match list.iter_mut().find(|(k, _)| k == name) {
                Some(c) => c.1 = e.value.clone(),
                None => list.push((name.to_string(), e.value.clone())),
            },
        }
    }
    (!list.is_empty()).then(|| list.into_iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("; "))
}

fn set(name: &str, value: &str) -> HeaderEdit {
    HeaderEdit { op: "set".into(), name: name.into(), value: value.into() }
}

fn remove(name: &str) -> HeaderEdit {
    HeaderEdit { op: "remove".into(), name: name.into(), value: String::new() }
}

/// Request header edits implied by the extras (cookies, user-agent, no-cache, no-cookies).
pub fn request_edits(m: &RuleExtras, headers: &[(String, String)]) -> Vec<HeaderEdit> {
    let mut v = Vec::new();
    if !m.user_agent.trim().is_empty() {
        v.push(set("User-Agent", m.user_agent.trim()));
    }
    if m.no_cache {
        v.extend(["If-None-Match", "If-Modified-Since", "If-Match", "If-Unmodified-Since", "If-Range"].map(remove));
        v.push(set("Cache-Control", "no-cache"));
        v.push(set("Pragma", "no-cache"));
    }
    if m.no_cookies {
        v.push(remove("Cookie"));
    } else if !m.cookies.is_empty() {
        let cur = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("cookie")).map(|(_, v)| v.as_str());
        match edit_cookie_header(cur, &m.cookies) {
            Some(c) => v.push(set("Cookie", &c)),
            None => v.push(remove("Cookie")),
        }
    }
    v
}

/// Response header edits implied by the extras.
pub fn response_edits(m: &RuleExtras) -> Vec<HeaderEdit> {
    let mut v = Vec::new();
    if m.no_cache {
        v.extend(["ETag", "Last-Modified", "Expires", "Age"].map(remove));
        v.push(set("Cache-Control", "no-store, no-cache, must-revalidate"));
    }
    if m.no_cookies {
        v.push(remove("Set-Cookie"));
    }
    v
}

// ------------------------------------------------------------------------------------------ throttling

/// How long sending `bytes` takes at `kbps` kilobits per second (capped at 2 minutes).
pub fn throttle_delay(bytes: usize, kbps: u32) -> std::time::Duration {
    if kbps == 0 {
        return std::time::Duration::ZERO;
    }
    let ms = (bytes as u64 * 8) / kbps as u64; // bits / (kbit/s) = ms
    std::time::Duration::from_millis(ms.min(120_000))
}

/// True with probability `pct` percent.
pub fn should_drop(pct: u8) -> bool {
    if pct == 0 {
        return false;
    }
    if pct >= 100 {
        return true;
    }
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    (h.finish() % 100) < pct as u64
}

// ------------------------------------------------------------------------------------------ Map Local folder

pub fn content_type_for(path: &std::path::Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase().as_str() {
        "json" => "application/json",
        "html" | "htm" => "text/html; charset=utf-8",
        "txt" | "log" => "text/plain; charset=utf-8",
        "js" | "mjs" => "application/javascript",
        "css" => "text/css",
        "xml" => "application/xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "pdf" => "application/pdf",
        "wasm" => "application/wasm",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "mp4" => "video/mp4",
        "csv" => "text/csv; charset=utf-8",
        "map" => "application/json",
        _ => "application/octet-stream",
    }
}

/// The file under `dir` for `url`: the URL path after `base` (a URL prefix; empty = the whole path).
/// "/" or a folder maps to index.html. None when the path tries to leave the folder.
pub fn folder_file(dir: &str, base: &str, url: &str) -> Option<std::path::PathBuf> {
    let no_q = url.split(['?', '#']).next().unwrap_or(url);
    let rel = if !base.trim().is_empty() && no_q.starts_with(base.trim()) {
        &no_q[base.trim().len()..]
    } else {
        let rest = no_q.split_once("://").map(|(_, r)| r).unwrap_or(no_q);
        rest.find('/').map(|i| &rest[i..]).unwrap_or("/")
    };
    let rel = crate::inspect::url_decode(rel, false);
    let mut p = std::path::PathBuf::from(dir.trim());
    for seg in rel.split(['/', '\\']).filter(|s| !s.is_empty()) {
        if seg == ".." || seg == "." || seg.contains(':') {
            return None;
        }
        p.push(seg);
    }
    if p.is_dir() || rel.ends_with('/') || rel.is_empty() {
        p.push("index.html");
    }
    Some(p)
}

/// Response for a "folder" (Map Local folder) rule: `r.file` is the folder, `r.target` the URL base.
pub fn folder_response(r: &Rule, url: &str) -> RuleResponse {
    let text = |status: u16, msg: String| RuleResponse {
        name: r.name.clone(),
        status,
        headers: vec![("Content-Type".into(), "text/plain; charset=utf-8".into())],
        body: msg.into_bytes(),
    };
    let Some(path) = folder_file(&r.file, &r.target, url) else {
        return text(403, "Path outside the mapped folder".into());
    };
    match std::fs::read(&path) {
        Ok(b) => {
            let mut headers: Vec<(String, String)> =
                r.headers.iter().filter(|h| h.enabled && !h.key.trim().is_empty()).map(|h| (h.key.trim().to_string(), h.value.clone())).collect();
            if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type")) {
                headers.push(("Content-Type".into(), content_type_for(&path).into()));
            }
            RuleResponse { name: r.name.clone(), status: if r.status == 0 { 200 } else { r.status }, headers, body: b }
        }
        Err(_) => text(404, format!("Map Local: no file {}", path.display())),
    }
}

/// Response when an allow-list is active and the request is not on it.
pub fn not_allowed_response() -> RuleResponse {
    RuleResponse {
        name: "allow list".into(),
        status: 403,
        headers: vec![("Content-Type".into(), "text/plain; charset=utf-8".into())],
        body: b"Blocked by Ferrotap: not on the allow list".to_vec(),
    }
}
