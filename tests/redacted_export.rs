//! "Share a redacted copy": exports with the privacy settings applied; originals untouched.
use ferrotap::data_io::{export_sessions, import_path, redacted, ExportFormat};
use ferrotap::model::SessionRecord;
use ferrotap::qol::PrivacySettings;

fn rec() -> SessionRecord {
    let mut s = SessionRecord { id: 1, method: "GET".into(), url: "https://api.example.com/x?token=abc&q=1".into(), status: 200, ..Default::default() };
    s.request_headers.insert("authorization".into(), "Bearer secret".into());
    s.request_headers.insert("cookie".into(), "sid=42; theme=dark".into());
    s.set_request_body(b"{\"password\":\"hunter2\"}");
    // binary response holding the secret between non-UTF-8 bytes
    s.set_response_body(&[0xff, 0xfe, b'h', b'u', b'n', b't', b'e', b'r', b'2', 0x00]);
    s
}

#[test]
fn redacted_export_masks_and_keeps_originals() {
    let p = PrivacySettings { cookies: vec!["sid".into()], body_regexes: vec!["hunter2".into()], ..Default::default() };
    let orig = vec![rec()];
    let red = redacted(&orig, &p);
    assert_eq!(orig[0].request_header("authorization"), Some("Bearer secret"));
    let r = &red[0];
    assert_eq!(r.request_header("authorization"), Some("[REDACTED]"));
    assert!(r.request_header("cookie").unwrap().contains("theme=dark"));
    assert!(!r.request_header("cookie").unwrap().contains("42"));
    assert!(!r.url.contains("abc") && r.url.contains("q=1"), "{}", r.url);
    assert!(!r.request_body.contains("hunter2"));
    let raw = r.response_bytes();
    assert!(!raw.windows(7).any(|w| w == b"hunter2"));
    assert_eq!(&raw[..2], &[0xff, 0xfe]);

    let dir = tempfile::tempdir().unwrap();
    for (fmt, name) in [(ExportFormat::Har, "a.har"), (ExportFormat::Ftap, "a.ftap")] {
        let path = dir.path().join(name);
        export_sessions(&path, fmt, &red).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("secret") && !text.contains("hunter2"), "{name} leaks");
        let back = import_path(&path).unwrap();
        assert_eq!(&back[0].response_bytes()[..2], &[0xff, 0xfe]);
    }
}
