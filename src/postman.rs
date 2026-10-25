//! Postman Collection (schema v2.1.0) export and import. Postman and many other API clients speak
//! this format, so it is a convenient way to hand a capture off for replay, or to pull one in.
use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{json, Value};

use crate::model::SessionRecord;

/// A short, human item name: `METHOD /path` (query and fragment stripped), falling back to the URL.
fn item_name(s: &SessionRecord) -> String {
    let after_scheme = s.url.split_once("://").map(|x| x.1).unwrap_or(&s.url);
    let path = match after_scheme.find('/') {
        Some(i) => &after_scheme[i..],
        None => "/",
    };
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let path = if path.is_empty() { "/" } else { path };
    format!("{} {}", s.method, path)
}

/// A Postman body/preview language hint from a Content-Type value.
fn lang_from_ct(ct: &str) -> &'static str {
    let ct = ct.to_ascii_lowercase();
    if ct.contains("json") {
        "json"
    } else if ct.contains("xml") {
        "xml"
    } else if ct.contains("html") {
        "html"
    } else {
        "text"
    }
}

/// Case-insensitive header lookup.
fn header_value<'a>(headers: &'a BTreeMap<String, String>, name: &str) -> Option<&'a str> {
    headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
}

/// The request body's `options.raw.language` hint, from the request Content-Type.
fn body_language(s: &SessionRecord) -> &'static str {
    lang_from_ct(header_value(&s.request_headers, "content-type").unwrap_or(""))
}

/// One Postman collection item for a session.
fn item_json(s: &SessionRecord) -> Value {
    let header: Vec<Value> = s.request_headers.iter().map(|(k, v)| json!({"key": k, "value": v})).collect();
    let query: Vec<Value> = crate::har::query_pairs(&s.url).into_iter().map(|(k, v)| json!({"key": k, "value": v})).collect();
    let mut url = json!({ "raw": s.url });
    if !query.is_empty() {
        url["query"] = Value::Array(query);
    }
    let mut request = json!({
        "method": s.method,
        "header": header,
        "url": url,
    });
    if !s.request_body.is_empty() {
        request["body"] = json!({
            "mode": "raw",
            "raw": s.request_body,
            "options": { "raw": { "language": body_language(s) } },
        });
    }
    // A captured response becomes a saved Postman example (handy for mocking/replay). status == 0
    // means no response was recorded (e.g. a failed connection), so leave the examples empty.
    let response = if s.status == 0 {
        Vec::new()
    } else {
        let resp_header: Vec<Value> = s.response_headers.iter().map(|(k, v)| json!({"key": k, "value": v})).collect();
        let preview = lang_from_ct(header_value(&s.response_headers, "content-type").unwrap_or(""));
        vec![json!({
            "name": item_name(s),
            "originalRequest": request.clone(),
            "status": crate::har::status_text(s.status),
            "code": s.status,
            "header": resp_header,
            "body": s.response_body,
            "_postman_previewlanguage": preview,
        })]
    };
    json!({
        "name": item_name(s),
        "request": request,
        "response": response,
    })
}

/// Build the whole collection document.
pub fn to_collection(sessions: &[SessionRecord]) -> Value {
    let items: Vec<Value> = sessions.iter().map(item_json).collect();
    json!({
        "info": {
            "name": "Ferrotap export",
            "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json",
            "description": "Captured by Ferrotap",
        },
        "item": items,
    })
}

/// Write a Postman Collection file; returns the number of items written.
pub fn export_postman(path: &Path, sessions: &[SessionRecord]) -> Result<usize, String> {
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
        }
    }
    let text = serde_json::to_string_pretty(&to_collection(sessions)).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(sessions.len())
}

// ---- import ----

/// A collection URL is either a bare string or `{ "raw": "...", ... }`.
fn url_string(u: &Value) -> String {
    if let Some(s) = u.as_str() {
        return s.to_string();
    }
    u["raw"].as_str().unwrap_or("").to_string()
}

/// Postman header array -> map, skipping disabled entries; repeats fold with ", " like HAR import.
fn import_headers(v: &Value) -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    if let Some(arr) = v.as_array() {
        for h in arr {
            if h["disabled"].as_bool() == Some(true) {
                continue;
            }
            let name = h["key"].as_str().unwrap_or("").to_string();
            if name.is_empty() {
                continue;
            }
            let value = h["value"].as_str().unwrap_or("").to_string();
            out.entry(name).and_modify(|old| { old.push_str(", "); old.push_str(&value); }).or_insert(value);
        }
    }
    out
}

/// Form a urlencoded body string from a Postman `urlencoded`/`formdata` array (file parts skipped).
fn encode_pairs(v: &Value) -> String {
    let Some(arr) = v.as_array() else { return String::new() };
    arr.iter()
        .filter(|p| p["disabled"].as_bool() != Some(true) && p["type"].as_str() != Some("file"))
        .map(|p| format!("{}={}", crate::model::pct_encode(p["key"].as_str().unwrap_or("")), crate::model::pct_encode(p["value"].as_str().unwrap_or(""))))
        .collect::<Vec<_>>()
        .join("&")
}

/// Extract the request body text from a Postman `body` object.
fn import_body(body: &Value) -> String {
    match body["mode"].as_str() {
        Some("raw") => body["raw"].as_str().unwrap_or("").to_string(),
        Some("urlencoded") => encode_pairs(&body["urlencoded"]),
        Some("formdata") => encode_pairs(&body["formdata"]),
        _ => String::new(),
    }
}

/// One request item -> session (id 0; the caller assigns ids). The item name becomes the comment.
fn session_from_item(it: &Value) -> SessionRecord {
    let req = &it["request"];
    let mut s = SessionRecord::default();
    if let Some(u) = req.as_str() {
        // Minimal form: "request" is just a URL string (implicitly GET).
        s.method = "GET".into();
        s.url = u.to_string();
    } else {
        s.method = req["method"].as_str().unwrap_or("GET").to_string();
        s.url = url_string(&req["url"]);
        s.request_headers = import_headers(&req["header"]);
        s.request_body = import_body(&req["body"]);
    }
    s.comment = it["name"].as_str().unwrap_or("").to_string();
    s
}

/// A folder item has its own `item` array; a request item has a `request`.
fn walk_item(it: &Value, out: &mut Vec<SessionRecord>) {
    if let Some(sub) = it["item"].as_array() {
        for s in sub {
            walk_item(s, out);
        }
    } else if !it["request"].is_null() {
        out.push(session_from_item(it));
    }
}

/// Parse a Postman collection (v2.0/v2.1) into sessions. Folders are flattened in document order.
pub fn parse_collection(text: &str) -> Result<Vec<SessionRecord>, String> {
    let text = text.trim_start_matches('\u{feff}');
    let v: Value = serde_json::from_str(text).map_err(|e| format!("not valid JSON: {e}"))?;
    let items = v["item"].as_array().ok_or("not a Postman collection (no item array)")?;
    let mut out = Vec::new();
    for it in items {
        walk_item(it, &mut out);
    }
    Ok(out)
}

/// Read a Postman collection file from disk.
pub fn import_postman(path: &Path) -> Result<Vec<SessionRecord>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    parse_collection(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> SessionRecord {
        let mut s = SessionRecord::default();
        s.method = "POST".into();
        s.url = "https://api.example.com/v1/items?page=2&q=red".into();
        s.request_headers.insert("Content-Type".into(), "application/json".into());
        s.request_body = "{\"name\":\"x\"}".into();
        s
    }

    #[test]
    fn collection_shape() {
        let c = to_collection(&[sample()]);
        assert_eq!(c["info"]["schema"], "https://schema.getpostman.com/json/collection/v2.1.0/collection.json");
        let item = &c["item"][0];
        assert_eq!(item["name"], "POST /v1/items"); // query stripped from the name
        assert_eq!(item["request"]["method"], "POST");
        assert_eq!(item["request"]["header"][0]["key"], "Content-Type");
        assert_eq!(item["request"]["url"]["raw"], "https://api.example.com/v1/items?page=2&q=red");
        let q = item["request"]["url"]["query"].as_array().unwrap();
        assert_eq!(q.len(), 2);
        assert_eq!((q[0]["key"].as_str(), q[0]["value"].as_str()), (Some("page"), Some("2")));
        assert_eq!(item["request"]["body"]["mode"], "raw");
        assert_eq!(item["request"]["body"]["raw"], "{\"name\":\"x\"}");
        assert_eq!(item["request"]["body"]["options"]["raw"]["language"], "json");
        assert_eq!(item["response"].as_array().unwrap().len(), 0); // sample() has no response
        // Whole document serializes as valid JSON.
        assert!(serde_json::to_string(&c).unwrap().contains("Ferrotap export"));
    }

    #[test]
    fn response_becomes_example() {
        let mut s = sample();
        s.status = 201;
        s.response_headers.insert("Content-Type".into(), "application/json".into());
        s.response_body = "{\"id\":7}".into();
        let c = to_collection(&[s]);
        let ex = &c["item"][0]["response"][0];
        assert_eq!(ex["code"], 201);
        assert_eq!(ex["status"], "Created"); // reason phrase
        assert_eq!(ex["_postman_previewlanguage"], "json");
        assert_eq!(ex["body"], "{\"id\":7}");
        assert_eq!(ex["header"][0]["key"], "Content-Type");
        assert_eq!(ex["originalRequest"]["method"], "POST"); // example carries the request
    }

    #[test]
    fn get_has_no_body_and_root_path() {
        let mut s = SessionRecord::default();
        s.method = "GET".into();
        s.url = "https://h.test".into();
        let c = to_collection(&[s]);
        let item = &c["item"][0];
        assert_eq!(item["name"], "GET /");
        assert!(item["request"]["body"].is_null()); // no body on a GET
        assert!(item["request"]["url"]["query"].is_null()); // no query array when there is none
    }

    #[test]
    fn round_trips_through_parse() {
        let text = serde_json::to_string(&to_collection(&[sample()])).unwrap();
        let back = parse_collection(&text).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].method, "POST");
        assert_eq!(back[0].url, "https://api.example.com/v1/items?page=2&q=red");
        assert_eq!(back[0].request_headers.get("Content-Type").map(String::as_str), Some("application/json"));
        assert_eq!(back[0].request_body, "{\"name\":\"x\"}");
    }

    #[test]
    fn imports_folders_urlencoded_and_disabled_headers() {
        let doc = r#"{
          "info": {"schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},
          "item": [
            {"name": "folder", "item": [
              {"name": "login", "request": {
                "method": "POST",
                "url": {"raw": "https://h.test/login"},
                "header": [
                  {"key": "X-On", "value": "1"},
                  {"key": "X-Off", "value": "nope", "disabled": true}
                ],
                "body": {"mode": "urlencoded", "urlencoded": [
                  {"key": "u", "value": "a b"},
                  {"key": "p", "value": "x&y"}
                ]}
              }}
            ]}
          ]
        }"#;
        let s = parse_collection(doc).unwrap();
        assert_eq!(s.len(), 1); // folder flattened to its one request
        assert_eq!(s[0].method, "POST");
        assert_eq!(s[0].url, "https://h.test/login");
        assert_eq!(s[0].comment, "login"); // item name kept as a note
        assert!(s[0].request_headers.contains_key("X-On"));
        assert!(!s[0].request_headers.contains_key("X-Off")); // disabled header dropped
        assert_eq!(s[0].request_body, "u=a%20b&p=x%26y"); // urlencoded + percent-encoded
    }

    #[test]
    fn bare_string_request_and_bad_input() {
        let doc = r#"{"item":[{"name":"ping","request":"https://h.test/ping"}]}"#;
        let s = parse_collection(doc).unwrap();
        assert_eq!((s[0].method.as_str(), s[0].url.as_str()), ("GET", "https://h.test/ping"));
        assert!(parse_collection("{\"nope\":1}").is_err()); // no item array
        assert!(parse_collection("not json").is_err());
    }
}
