//! SAZ archives: our zip reader/writer, export -> import round trip, and parsing raw session files.
use ferrotap::data_io::{export_sessions, import_path, ExportFormat};
use ferrotap::model::{SessionMark, SessionRecord};
use ferrotap::saz::*;
use std::io::Write;

fn sample() -> Vec<SessionRecord> {
    let mut a = SessionRecord {
        id: 7,
        method: "POST".into(),
        url: "https://api.example.com/v1/items?x=1".into(),
        status: 201,
        http_version: "HTTP/1.1".into(),
        response_http_version: "HTTP/1.1".into(),
        start_time: "2026-10-23T10:00:00+00:00".into(),
        duration_ms: 250,
        dns_ms: Some(5),
        connect_ms: Some(20),
        tls_ms: Some(30),
        ttfb_ms: Some(100),
        comment: "created <ok> & fine".into(),
        mark: Some(SessionMark::Green),
        client_ip: Some("127.0.0.1".into()),
        process_name: Some("curl.exe".into()),
        pid: Some(4242),
        ..Default::default()
    };
    a.request_headers.insert("content-type".into(), "application/json".into());
    a.set_request_body(b"{\"name\":\"x\"}");
    a.response_headers.insert("set-cookie".into(), "a=1; Path=/\nb=2".into());
    a.response_headers.insert("content-encoding".into(), "gzip".into());
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(b"{\"id\":1}").unwrap();
    a.set_response_body(&e.finish().unwrap());
    let mut b = SessionRecord { method: "GET".into(), url: "http://plain.example/".into(), status: 200, mark: Some(SessionMark::Strikeout), ..Default::default() };
    b.response_headers.insert("transfer-encoding".into(), "chunked".into());
    b.set_response_body(b"hello world");
    b.response_trailers = vec![("x-checksum".into(), "abc".into())];
    vec![a, b]
}

#[test]
fn zip_round_trip() {
    let files = vec![("raw/01_c.txt".to_string(), b"GET / HTTP/1.1\r\n\r\n".to_vec()), ("big.bin".to_string(), vec![7u8; 100_000])];
    let z = zip_write(&files).unwrap();
    assert!(z.len() < 50_000, "deflated");
    let back = zip_read(&z).unwrap();
    assert_eq!(back.len(), 2);
    assert_eq!(back["big.bin"], vec![7u8; 100_000]);
    assert!(zip_read(b"not a zip").is_err());
}

#[test]
fn saz_round_trip_keeps_everything() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.saz");
    assert_eq!(export_sessions(&path, ExportFormat::Saz, &sample()).unwrap(), 2);
    if let Ok(out) = std::env::var("SAZ_OUT") {
        std::fs::copy(&path, out).unwrap(); // for checking the archive with another zip reader
    }
    let back = import_path(&path).unwrap();
    assert_eq!(back.len(), 2);
    let (a, o) = (&back[0], &sample()[0]);
    assert_eq!((a.method.as_str(), a.url.as_str(), a.status), ("POST", o.url.as_str(), 201));
    assert_eq!(a.request_body, o.request_body);
    assert_eq!(&*a.response_bytes(), &*o.response_bytes(), "gzip body byte-exact");
    assert_eq!(a.response_header("set-cookie"), Some("a=1; Path=/\nb=2"));
    assert_eq!(a.duration_ms, 250);
    assert_eq!((a.dns_ms, a.connect_ms, a.tls_ms, a.ttfb_ms), (Some(5), Some(20), Some(30), Some(100)), "waterfall timers survive the round trip");
    assert_eq!(a.comment, "created <ok> & fine");
    assert_eq!(a.mark, Some(SessionMark::Green));
    assert_eq!(a.process_name.as_deref(), Some("curl.exe"));
    assert_eq!(a.pid, Some(4242));
    assert_eq!(a.client_ip.as_deref(), Some("127.0.0.1"));
    let b = &back[1];
    assert_eq!(b.response_body, "hello world", "re-chunked on export, dechunked on import");
    assert_eq!(b.response_trailers, vec![("x-checksum".to_string(), "abc".to_string())]);
    assert_eq!(b.mark, Some(SessionMark::Strikeout));
}

#[test]
fn parses_relative_targets_and_chunked_bodies() {
    let c = b"GET /path?q=1 HTTP/1.1\r\nHost: www.example.com\r\nAccept: */*\r\n\r\n";
    let s = b"HTTP/1.1 404 Not Found\r\nTransfer-Encoding: chunked\r\nContent-Type: text/plain\r\n\r\n4\r\nnope\r\n0\r\n\r\n";
    let m = r#"<Session SID="3"><SessionTimers ClientBeginRequest="2026-01-01T00:00:00.0000000+01:00" ClientDoneResponse="2026-01-01T00:00:01.5000000+01:00" /><SessionFlags><SessionFlag N="ui-color" V="red" /><SessionFlag N="ui-comments" V="a &amp; b" /></SessionFlags></Session>"#;
    let r = session_from_files(c, Some(s), Some(m));
    assert_eq!(r.url, "https://www.example.com/path?q=1");
    assert_eq!(r.status, 404);
    assert_eq!(r.response_body, "nope");
    assert_eq!(r.duration_ms, 1500);
    assert_eq!(r.mark, Some(SessionMark::Red));
    assert_eq!(r.comment, "a & b");
    let t = session_from_files(b"CONNECT example.com:443 HTTP/1.1\r\n\r\n", None, None);
    assert_eq!(t.url, "tunnel://example.com:443");
}
