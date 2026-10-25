//! Binary / compressed bodies keep their exact bytes (base64 beside the lossy text), so auto-decode,
//! Hex and gRPC work on what was really captured; HAR import/export keeps them byte-exact.
use ferrotap::model::{body_b64, body_text, SessionRecord};
use std::io::Write;

fn gzip(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

fn session_with_response(raw: &[u8], ce: &str) -> SessionRecord {
    let mut s = SessionRecord { url: "https://example.com/a".into(), status: 200, ..Default::default() };
    s.response_headers.insert("content-encoding".into(), ce.into());
    s.response_headers.insert("content-type".into(), "application/json".into());
    s.set_response_body(raw);
    s
}

#[test]
fn utf8_bodies_need_no_copy() {
    assert_eq!(body_b64(b"plain text"), None);
    assert_eq!(body_text(b"plain text"), "plain text");
    let s = session_with_response(b"{\"a\":1}", "identity");
    assert!(s.response_body_b64.is_none());
    assert_eq!(&*s.response_bytes(), b"{\"a\":1}");
}

#[test]
fn binary_bodies_round_trip_exactly() {
    let raw = gzip(b"{\"hello\":\"world\"}");
    let s = session_with_response(&raw, "gzip");
    assert!(s.response_body_b64.is_some());
    assert_eq!(&*s.response_bytes(), &raw[..]);
    // survives the JSON storage used by sessions.json / .ftap
    let back: SessionRecord = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
    assert_eq!(&*back.response_bytes(), &raw[..]);
}

#[test]
fn auto_decode_uses_exact_bytes() {
    let raw = gzip(b"{\"hello\":\"world\"}");
    let s = session_with_response(&raw, "gzip");
    match ferrotap::inspect::decode_body_bytes(&s.response_bytes(), s.response_header("content-encoding"), s.response_header("content-type")) {
        ferrotap::inspect::BodyDecode::Decoded { encodings, text } => {
            assert_eq!(encodings, vec!["gzip".to_string()]);
            assert_eq!(text, "{\"hello\":\"world\"}");
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn grpc_frames_from_exact_bytes() {
    // one uncompressed frame holding protobuf field 1 = varint 300 (0xAC 0x02 is not UTF-8)
    let raw = [0u8, 0, 0, 0, 3, 0x08, 0xAC, 0x02];
    let mut s = SessionRecord::default();
    s.set_response_body(&raw);
    assert!(s.response_body.contains('\u{fffd}'));
    let v = ferrotap::streams::grpc_view_bytes(&s.response_bytes()).unwrap();
    assert!(v.contains("300"), "{v}");
}

#[test]
fn har_keeps_binary_bodies() {
    let raw = gzip(b"payload");
    let s = session_with_response(&raw, "gzip");
    let har = ferrotap::har::to_har(&[s]);
    assert_eq!(har["log"]["entries"][0]["response"]["content"]["encoding"], "base64");
    let back = ferrotap::har::parse_har(&har.to_string()).unwrap();
    assert_eq!(&*back[0].response_bytes(), &raw[..]);
}
