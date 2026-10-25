//! HAR 1.2 export and import (G6 export/import, G17 built-in HAR viewer).
//! Pure functions over `SessionRecord`, no UI code, so they are unit-testable.
use std::collections::BTreeMap;
use std::path::Path;

use base64::Engine as _;
use serde_json::{json, Value};

use crate::model::{SessionMark, SessionRecord};

fn split_version(v: &str) -> String {
    if v.is_empty() { "HTTP/1.1".to_string() } else { v.to_string() }
}

fn headers_json(h: &BTreeMap<String, String>) -> Vec<Value> {
    h.iter().map(|(k, v)| json!({"name": k, "value": v})).collect()
}

/// HAR `request.cookies` from the `Cookie` request header (name/value only).
fn request_cookies_json(h: &BTreeMap<String, String>) -> Vec<Value> {
    match header_ci(h, "cookie") {
        Some(v) => crate::inspect::parse_cookie_header(v).into_iter().map(|(k, v)| json!({"name": k, "value": v})).collect(),
        None => Vec::new(),
    }
}

/// HAR `response.cookies` from every `Set-Cookie` response header, with the attributes HAR defines.
fn response_cookies_json(h: &BTreeMap<String, String>) -> Vec<Value> {
    crate::inspect::response_set_cookies(h)
        .into_iter()
        .map(|c| {
            let mut o = json!({"name": c.name, "value": c.value});
            if let Some(p) = c.path { o["path"] = json!(p); }
            if let Some(d) = c.domain { o["domain"] = json!(d); }
            if let Some(e) = c.expires { o["expires"] = json!(e); }
            if c.http_only { o["httpOnly"] = json!(true); }
            if c.secure { o["secure"] = json!(true); }
            o
        })
        .collect()
}

fn header_ci<'a>(h: &'a BTreeMap<String, String>, name: &str) -> Option<&'a str> {
    h.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
}

/// Query string pairs of a URL (percent-decoded).
pub fn query_pairs(url: &str) -> Vec<(String, String)> {
    let Some(q) = url.split_once('?').map(|(_, q)| q) else { return Vec::new() };
    let q = q.split('#').next().unwrap_or("");
    q.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (crate::model::pct_decode(k), crate::model::pct_decode(v))
        })
        .collect()
}

/// Reason phrase for common status codes (HAR wants a statusText).
pub fn status_text(code: u16) -> &'static str {
    match code {
        100 => "Continue", 101 => "Switching Protocols", 102 => "Processing", 103 => "Early Hints",
        200 => "OK", 201 => "Created", 202 => "Accepted", 203 => "Non-Authoritative Information",
        204 => "No Content", 205 => "Reset Content", 206 => "Partial Content", 207 => "Multi-Status",
        208 => "Already Reported", 226 => "IM Used",
        300 => "Multiple Choices", 301 => "Moved Permanently", 302 => "Found", 303 => "See Other",
        304 => "Not Modified", 305 => "Use Proxy", 307 => "Temporary Redirect", 308 => "Permanent Redirect",
        400 => "Bad Request", 401 => "Unauthorized", 402 => "Payment Required", 403 => "Forbidden",
        404 => "Not Found", 405 => "Method Not Allowed", 406 => "Not Acceptable",
        407 => "Proxy Authentication Required", 408 => "Request Timeout", 409 => "Conflict",
        410 => "Gone", 411 => "Length Required", 412 => "Precondition Failed", 413 => "Content Too Large",
        414 => "URI Too Long", 415 => "Unsupported Media Type", 416 => "Range Not Satisfiable",
        417 => "Expectation Failed", 418 => "I'm a teapot", 421 => "Misdirected Request",
        422 => "Unprocessable Content", 423 => "Locked", 424 => "Failed Dependency", 425 => "Too Early",
        426 => "Upgrade Required", 428 => "Precondition Required", 429 => "Too Many Requests",
        431 => "Request Header Fields Too Large", 451 => "Unavailable For Legal Reasons",
        500 => "Internal Server Error", 501 => "Not Implemented", 502 => "Bad Gateway",
        503 => "Service Unavailable", 504 => "Gateway Timeout", 505 => "HTTP Version Not Supported",
        506 => "Variant Also Negotiates", 507 => "Insufficient Storage", 508 => "Loop Detected",
        510 => "Not Extended", 511 => "Network Authentication Required",
        _ => "",
    }
}

fn opt(v: Option<u64>) -> i64 {
    v.map(|x| x as i64).unwrap_or(-1)
}

/// HAR `timings` object built from the session's measured phases.
/// `connect` includes `ssl` (as the HAR spec requires); `receive` is what is left of the total.
pub fn timings_json(s: &SessionRecord) -> Value {
    let dns = opt(s.dns_ms);
    let ssl = opt(s.tls_ms);
    let connect = match (s.connect_ms, s.tls_ms) {
        (None, None) => -1,
        (c, t) => (c.unwrap_or(0) + t.unwrap_or(0)) as i64,
    };
    let total = s.duration_ms as i64;
    let pre = dns.max(0) + connect.max(0);
    let wait = match s.ttfb_ms {
        Some(w) => w as i64,
        None => (total - pre).max(0),
    };
    let receive = (total - pre - wait).max(0);
    json!({"blocked": -1, "dns": dns, "connect": connect, "ssl": ssl, "send": 0, "wait": wait, "receive": receive})
}

/// One HAR entry for a session.
pub fn entry_json(s: &SessionRecord) -> Value {
    let req_ct = header_ci(&s.request_headers, "content-type").unwrap_or("").to_string();
    let resp_ct = header_ci(&s.response_headers, "content-type").unwrap_or("").to_string();
    let query: Vec<Value> = query_pairs(&s.url).into_iter().map(|(k, v)| json!({"name": k, "value": v})).collect();
    let mut request = json!({
        "method": s.method,
        "url": s.url,
        "httpVersion": split_version(&s.http_version),
        "cookies": request_cookies_json(&s.request_headers),
        "headers": headers_json(&s.request_headers),
        "queryString": query,
        "headersSize": -1,
        "bodySize": s.request_body.len(),
    });
    if !s.request_body.is_empty() {
        let mut post = json!({"mimeType": req_ct, "text": s.request_body});
        // For form posts, HAR consumers expect the decoded fields under `params`.
        if req_ct.split(';').next().unwrap_or("").trim().eq_ignore_ascii_case("application/x-www-form-urlencoded") {
            let params: Vec<Value> = crate::inspect::parse_urlencoded(&s.request_body)
                .into_iter()
                .map(|(k, v)| json!({"name": k, "value": v}))
                .collect();
            if !params.is_empty() {
                post["params"] = json!(params);
            }
        }
        request["postData"] = post;
    }
    let redirect = header_ci(&s.response_headers, "location").unwrap_or("").to_string();
    let resp_ver = if s.response_http_version.is_empty() { &s.http_version } else { &s.response_http_version };
    let response = json!({
        "status": s.status,
        "statusText": status_text(s.status),
        "httpVersion": split_version(resp_ver),
        "cookies": response_cookies_json(&s.response_headers),
        "headers": headers_json(&s.response_headers),
        "content": {
            "size": s.response_decoded_size.unwrap_or(s.response_body.len()),
            "mimeType": resp_ct,
            "text": s.response_body_b64.as_deref().unwrap_or(&s.response_body),
        },
        "redirectURL": redirect,
        "headersSize": -1,
        "bodySize": s.response_raw_size.unwrap_or(s.response_body.len()),
    });
    let started = if s.start_time.is_empty() { chrono::Utc::now().to_rfc3339() } else { s.start_time.clone() };
    let mut e = json!({
        "startedDateTime": started,
        "time": s.duration_ms as u64,
        "request": request,
        "response": response,
        "cache": {},
        "timings": timings_json(s),
    });
    if s.response_body_b64.is_some() { e["response"]["content"]["encoding"] = json!("base64"); }
    if let Some(ip) = &s.remote_ip { e["serverIPAddress"] = json!(ip); }
    if !s.comment.is_empty() { e["comment"] = json!(s.comment); }
    if let Some(m) = s.mark { e["_mark"] = json!(m.as_str()); }
    if let Some(p) = &s.process_name { e["_process"] = json!(p); }
    if let Some(v) = &s.tls_version { e["_tlsVersion"] = json!(v); }
    e
}

/// A complete HAR 1.2 document for the given sessions.
pub fn to_har(sessions: &[SessionRecord]) -> Value {
    let entries: Vec<Value> = sessions.iter().map(entry_json).collect();
    json!({
        "log": {
            "version": "1.2",
            "creator": {"name": "Ferrotap", "version": env!("CARGO_PKG_VERSION")},
            "pages": [],
            "entries": entries,
        }
    })
}

/// Write a HAR file; returns the number of entries written.
pub fn export_har(path: &Path, sessions: &[SessionRecord]) -> Result<usize, String> {
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
        }
    }
    let text = serde_json::to_string_pretty(&to_har(sessions)).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(sessions.len())
}

fn headers_from(v: &Value) -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    if let Some(arr) = v.as_array() {
        for h in arr {
            let name = h["name"].as_str().unwrap_or("").to_string();
            if name.is_empty() { continue; }
            let value = h["value"].as_str().unwrap_or("").to_string();
            out.entry(name).and_modify(|old| { old.push_str(", "); old.push_str(&value); }).or_insert(value);
        }
    }
    out
}

fn num(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_f64().map(|f| f.round() as i64))
}

fn nonneg(v: &Value) -> Option<u64> {
    num(v).filter(|x| *x >= 0).map(|x| x as u64)
}

fn content_bytes(c: &Value) -> Vec<u8> {
    let text = c["text"].as_str().unwrap_or("");
    if c["encoding"].as_str() == Some("base64") {
        match base64::engine::general_purpose::STANDARD.decode(text.trim()) {
            Ok(bytes) => bytes,
            Err(_) => text.as_bytes().to_vec(),
        }
    } else {
        text.as_bytes().to_vec()
    }
}

/// Convert one HAR entry into a session (id 0; the caller assigns ids).
pub fn session_from_entry(e: &Value) -> SessionRecord {
    let req = &e["request"];
    let resp = &e["response"];
    let t = &e["timings"];
    let ssl = nonneg(&t["ssl"]);
    let connect = nonneg(&t["connect"]).map(|c| c.saturating_sub(ssl.unwrap_or(0)));
    let mut request_body = req["postData"]["text"].as_str().unwrap_or("").to_string();
    if request_body.is_empty() {
        if let Some(params) = req["postData"]["params"].as_array() {
            request_body = params
                .iter()
                .map(|p| format!("{}={}", crate::model::pct_encode(p["name"].as_str().unwrap_or("")), crate::model::pct_encode(p["value"].as_str().unwrap_or(""))))
                .collect::<Vec<_>>()
                .join("&");
        }
    }
    let response_raw = content_bytes(&resp["content"]);
    let body_size = nonneg(&resp["bodySize"]).map(|x| x as usize);
    let content_size = nonneg(&resp["content"]["size"]).map(|x| x as usize);
    SessionRecord {
        id: 0,
        method: req["method"].as_str().unwrap_or("GET").to_string(),
        url: req["url"].as_str().unwrap_or("").to_string(),
        status: num(&resp["status"]).unwrap_or(0).clamp(0, u16::MAX as i64) as u16,
        http_version: req["httpVersion"].as_str().unwrap_or("").to_string(),
        response_http_version: resp["httpVersion"].as_str().unwrap_or("").to_string(),
        request_headers: headers_from(&req["headers"]),
        response_headers: headers_from(&resp["headers"]),
        request_body,
        response_body: crate::model::body_text(&response_raw),
        response_body_b64: crate::model::body_b64(&response_raw),
        start_time: e["startedDateTime"].as_str().unwrap_or("").to_string(),
        duration_ms: nonneg(&e["time"]).unwrap_or(0) as u128,
        mark: e["_mark"].as_str().and_then(SessionMark::parse),
        comment: e["comment"].as_str().unwrap_or("").to_string(),
        response_raw_size: body_size,
        response_decoded_size: content_size,
        response_trailers: Vec::new(),
        process_name: e["_process"].as_str().map(str::to_string),
        pid: None,
        client_ip: None,
        remote_ip: e["serverIPAddress"].as_str().filter(|s| !s.is_empty()).map(str::to_string),
        tls_version: e["_tlsVersion"].as_str().map(str::to_string),
        dns_ms: nonneg(&t["dns"]),
        connect_ms: connect,
        tls_ms: ssl,
        ttfb_ms: nonneg(&t["wait"]),
        ..Default::default()
    }
}

/// Parse a HAR document (text) into sessions.
pub fn parse_har(text: &str) -> Result<Vec<SessionRecord>, String> {
    let text = text.trim_start_matches('\u{feff}');
    let v: Value = serde_json::from_str(text).map_err(|e| format!("not valid JSON: {e}"))?;
    let entries = v["log"]["entries"].as_array().ok_or("not a HAR file (no log.entries)")?;
    Ok(entries.iter().map(session_from_entry).collect())
}

/// Read a .har file from disk.
pub fn import_har(path: &Path) -> Result<Vec<SessionRecord>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    parse_har(&text)
}
