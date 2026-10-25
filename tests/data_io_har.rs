//! G6 / G17: HAR 1.2 export structure and export -> import round trip.
use std::collections::BTreeMap;

use ferrotap::data_io;
use ferrotap::har;
use ferrotap::model::{SessionMark, SessionRecord};

fn rec(id: usize, method: &str, url: &str, status: u16) -> SessionRecord {
    let mut rq = BTreeMap::new();
    rq.insert("Host".to_string(), "example.com".to_string());
    rq.insert("Content-Type".to_string(), "application/json".to_string());
    let mut rs = BTreeMap::new();
    rs.insert("Content-Type".to_string(), "text/html; charset=utf-8".to_string());
    rs.insert("X-Test".to_string(), "yes".to_string());
    SessionRecord {
        id,
        method: method.to_string(),
        url: url.to_string(),
        status,
        http_version: "HTTP/1.1".to_string(),
        response_http_version: "HTTP/2.0".to_string(),
        request_headers: rq,
        response_headers: rs,
        request_body: if method == "POST" { "{\"a\":1}".to_string() } else { String::new() },
        response_body: format!("<html>{id} \u{e9}</html>"),
        start_time: "2026-10-16T10:00:00+00:00".to_string(),
        duration_ms: 250,
        mark: Some(SessionMark::Red),
        comment: "note".to_string(),
        dns_ms: Some(5),
        connect_ms: Some(20),
        tls_ms: Some(30),
        ttfb_ms: Some(100),
        remote_ip: Some("93.184.216.34".to_string()),
        ..Default::default()
    }
}

#[test]
fn har_export_structure() {
    let sessions = vec![rec(1, "GET", "https://example.com/a?x=1&y=hello%20world", 200), rec(2, "POST", "https://example.com/api", 201)];
    let v = har::to_har(&sessions);
    let log = &v["log"];
    assert_eq!(log["version"], "1.2");
    assert_eq!(log["creator"]["name"], "Ferrotap");
    assert!(log["creator"]["version"].as_str().unwrap().len() > 0);
    let entries = log["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2);

    let e = &entries[0];
    assert_eq!(e["startedDateTime"], "2026-10-16T10:00:00+00:00");
    assert_eq!(e["time"], 250);
    assert_eq!(e["request"]["method"], "GET");
    assert_eq!(e["request"]["httpVersion"], "HTTP/1.1");
    let hdrs = e["request"]["headers"].as_array().unwrap();
    assert!(hdrs.iter().any(|h| h["name"] == "Host" && h["value"] == "example.com"));
    let qs = e["request"]["queryString"].as_array().unwrap();
    assert_eq!(qs.len(), 2);
    assert_eq!(qs[1]["name"], "y");
    assert_eq!(qs[1]["value"], "hello world");
    assert!(e["request"].get("postData").is_none());
    assert_eq!(e["response"]["status"], 200);
    assert_eq!(e["response"]["statusText"], "OK");
    assert_eq!(e["response"]["httpVersion"], "HTTP/2.0");
    assert_eq!(e["response"]["content"]["mimeType"], "text/html; charset=utf-8");
    assert_eq!(e["response"]["content"]["text"], "<html>1 \u{e9}</html>");
    assert!(e["response"]["headers"].as_array().unwrap().iter().any(|h| h["name"] == "X-Test"));
    assert_eq!(e["serverIPAddress"], "93.184.216.34");
    assert_eq!(e["comment"], "note");

    // timings: connect includes ssl; wait = ttfb; receive = remainder
    let t = &e["timings"];
    assert_eq!(t["dns"], 5);
    assert_eq!(t["connect"], 50);
    assert_eq!(t["ssl"], 30);
    assert_eq!(t["send"], 0);
    assert_eq!(t["wait"], 100);
    assert_eq!(t["receive"], 250 - 5 - 50 - 100);
    assert_eq!(t["blocked"], -1);

    let p = &entries[1]["request"]["postData"];
    assert_eq!(p["mimeType"], "application/json");
    assert_eq!(p["text"], "{\"a\":1}");
    assert_eq!(entries[1]["response"]["status"], 201);
}

#[test]
fn har_timings_unknown_are_minus_one() {
    let mut s = rec(1, "GET", "http://example.com/", 200);
    s.dns_ms = None;
    s.connect_ms = None;
    s.tls_ms = None;
    s.ttfb_ms = None;
    let t = har::timings_json(&s);
    assert_eq!(t["dns"], -1);
    assert_eq!(t["connect"], -1);
    assert_eq!(t["ssl"], -1);
    assert_eq!(t["wait"], 250);
    assert_eq!(t["receive"], 0);
}

#[test]
fn har_round_trip_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sub").join("out.har");
    let sessions = vec![rec(1, "GET", "https://example.com/a?x=1", 200), rec(2, "POST", "https://example.com/api", 404)];
    assert_eq!(data_io::export_sessions(&path, data_io::ExportFormat::Har, &sessions).unwrap(), 2);
    let back = data_io::import_path(&path).unwrap();
    assert_eq!(back.len(), 2);
    for (a, b) in sessions.iter().zip(back.iter()) {
        assert_eq!(a.method, b.method);
        assert_eq!(a.url, b.url);
        assert_eq!(a.status, b.status);
        assert_eq!(a.request_headers, b.request_headers);
        assert_eq!(a.response_headers, b.response_headers);
        assert_eq!(a.request_body, b.request_body);
        assert_eq!(a.response_body, b.response_body);
        assert_eq!(a.duration_ms, b.duration_ms);
        assert_eq!(a.dns_ms, b.dns_ms);
        assert_eq!(a.connect_ms, b.connect_ms);
        assert_eq!(a.tls_ms, b.tls_ms);
        assert_eq!(a.ttfb_ms, b.ttfb_ms);
        assert_eq!(a.start_time, b.start_time);
        assert_eq!(a.comment, b.comment);
        assert_eq!(a.mark, b.mark);
        assert_eq!(a.remote_ip, b.remote_ip);
        assert_eq!(a.http_version, b.http_version);
        assert_eq!(a.response_http_version, b.response_http_version);
    }
}

#[test]
fn har_import_foreign_file() {
    // A HAR as other tools write it: base64 content, duplicate headers, postData params, float times.
    let text = r#"{"log":{"version":"1.2","creator":{"name":"x","version":"1"},"entries":[
      {"startedDateTime":"2026-01-01T00:00:00Z","time":12.6,
       "request":{"method":"POST","url":"http://h/p","httpVersion":"HTTP/1.1",
          "headers":[{"name":"Accept","value":"a"},{"name":"Accept","value":"b"}],
          "queryString":[],"cookies":[],"headersSize":-1,"bodySize":3,
          "postData":{"mimeType":"application/x-www-form-urlencoded","params":[{"name":"k","value":"v 1"}]}},
       "response":{"status":500,"statusText":"","httpVersion":"HTTP/1.1","headers":[],"cookies":[],
          "content":{"size":5,"mimeType":"text/plain","text":"aGVsbG8=","encoding":"base64"},
          "redirectURL":"","headersSize":-1,"bodySize":-1},
       "cache":{},"timings":{"send":0,"wait":7.2,"receive":1}}]}}"#;
    let v = har::parse_har(text).unwrap();
    assert_eq!(v.len(), 1);
    let s = &v[0];
    assert_eq!(s.method, "POST");
    assert_eq!(s.status, 500);
    assert_eq!(s.request_headers["Accept"], "a, b");
    assert_eq!(s.request_body, "k=v%201");
    assert_eq!(s.response_body, "hello");
    assert_eq!(s.duration_ms, 13);
    assert_eq!(s.ttfb_ms, Some(7));
    assert_eq!(s.dns_ms, None);
    assert_eq!(s.response_raw_size, None);

    assert!(har::parse_har("{\"foo\":1}").is_err());
    assert!(har::parse_har("not json").is_err());
}

#[test]
fn status_text_covers_common_codes() {
    assert_eq!(har::status_text(200), "OK");
    assert_eq!(har::status_text(422), "Unprocessable Content");
    assert_eq!(har::status_text(429), "Too Many Requests");
    assert_eq!(har::status_text(451), "Unavailable For Legal Reasons");
    assert_eq!(har::status_text(418), "I'm a teapot");
    assert_eq!(har::status_text(308), "Permanent Redirect");
    assert_eq!(har::status_text(511), "Network Authentication Required");
    assert_eq!(har::status_text(599), ""); // unknown -> empty, as before
}

#[test]
fn har_export_cookies_and_form_params() {
    // A request with a Cookie header and a urlencoded form body; a response with two Set-Cookie
    // headers (one carrying attributes). HAR's cookies[] and postData.params[] must be populated.
    let mut rq = BTreeMap::new();
    rq.insert("Content-Type".to_string(), "application/x-www-form-urlencoded; charset=utf-8".to_string());
    rq.insert("Cookie".to_string(), "sid=abc123; theme=dark".to_string());
    let mut rs = BTreeMap::new();
    rs.insert(
        "Set-Cookie".to_string(),
        "sid=xyz; Path=/; Domain=example.com; HttpOnly; Secure\nflag=1".to_string(),
    );
    let s = SessionRecord {
        id: 1,
        method: "POST".to_string(),
        url: "https://example.com/login".to_string(),
        status: 200,
        http_version: "HTTP/1.1".to_string(),
        request_headers: rq,
        response_headers: rs,
        request_body: "user=alice&pass=p%40ss+word&next=".to_string(),
        ..Default::default()
    };
    let e = &har::to_har(&[s])["log"]["entries"][0];

    // Request cookies from the Cookie header.
    let rc = e["request"]["cookies"].as_array().unwrap();
    assert_eq!(rc.len(), 2);
    assert_eq!(rc[0]["name"], "sid");
    assert_eq!(rc[0]["value"], "abc123");
    assert_eq!(rc[1]["name"], "theme");
    assert_eq!(rc[1]["value"], "dark");

    // Form fields decoded under postData.params, with text still present for round trips.
    let pd = &e["request"]["postData"];
    assert_eq!(pd["text"], "user=alice&pass=p%40ss+word&next=");
    let params = pd["params"].as_array().unwrap();
    assert_eq!(params.len(), 3);
    assert_eq!(params[0]["name"], "user");
    assert_eq!(params[0]["value"], "alice");
    assert_eq!(params[1]["name"], "pass");
    assert_eq!(params[1]["value"], "p@ss word");
    assert_eq!(params[2]["name"], "next");
    assert_eq!(params[2]["value"], "");

    // Response cookies from both Set-Cookie headers, attributes included.
    let sc = e["response"]["cookies"].as_array().unwrap();
    assert_eq!(sc.len(), 2);
    assert_eq!(sc[0]["name"], "sid");
    assert_eq!(sc[0]["value"], "xyz");
    assert_eq!(sc[0]["path"], "/");
    assert_eq!(sc[0]["domain"], "example.com");
    assert_eq!(sc[0]["httpOnly"], true);
    assert_eq!(sc[0]["secure"], true);
    assert_eq!(sc[1]["name"], "flag");
    assert_eq!(sc[1]["value"], "1");
    assert!(sc[1].get("httpOnly").is_none());

    // A JSON body must not get params.
    let mut rq2 = BTreeMap::new();
    rq2.insert("Content-Type".to_string(), "application/json".to_string());
    let s2 = SessionRecord {
        id: 2, method: "POST".to_string(), url: "https://example.com/api".to_string(), status: 200,
        request_headers: rq2, request_body: "{\"a\":1}".to_string(), ..Default::default()
    };
    let e2 = &har::to_har(&[s2])["log"]["entries"][0];
    assert!(e2["request"]["postData"].get("params").is_none());
    assert_eq!(e2["request"]["cookies"].as_array().unwrap().len(), 0);
    assert_eq!(e2["response"]["cookies"].as_array().unwrap().len(), 0);
}

#[test]
fn fresh_ids_do_not_clash() {
    let existing = vec![rec(3, "GET", "http://a/", 200), rec(1_000_000_005, "GET", "http://b/", 200)];
    let mut imported = vec![rec(3, "GET", "http://c/", 200), rec(3, "GET", "http://d/", 200)];
    data_io::assign_fresh_ids(&mut imported, &existing);
    assert_eq!(imported[0].id, 1_000_000_006);
    assert_eq!(imported[1].id, 1_000_000_007);
    let mut imported2 = vec![rec(1, "GET", "http://c/", 200)];
    data_io::assign_fresh_ids(&mut imported2, &[rec(7, "GET", "http://a/", 200)]);
    assert_eq!(imported2[0].id, data_io::IMPORT_ID_BASE);
}
