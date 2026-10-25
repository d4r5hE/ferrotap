//! G3 inspectors: Content-Encoding auto-decode and the Decode tool (base64, URL, HTML, hex, JWT).
use ferrotap::inspect::*;
use std::io::Write;

fn gzip(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

#[test]
fn not_encoded_bodies_pass_through() {
    assert_eq!(decode_body("hello", None, None), BodyDecode::NotEncoded);
    assert_eq!(decode_body("hello", Some("identity"), None), BodyDecode::NotEncoded);
    assert_eq!(decode_body("hello", Some("  "), None), BodyDecode::NotEncoded);
}

#[test]
fn lossy_compressed_body_reports_honestly() {
    let stored = String::from_utf8_lossy(&gzip(b"{\"hello\":\"world\"}")).into_owned();
    match decode_body(&stored, Some("gzip"), None) {
        BodyDecode::Failed { encodings, reason, already_decoded } => {
            assert_eq!(encodings, vec!["gzip".to_string()]);
            assert!(reason.contains("lossy"), "{reason}");
            assert!(!already_decoded);
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn already_decoded_text_is_detected() {
    match decode_body("{\"ok\":true}", Some("gzip"), None) {
        BodyDecode::Failed { already_decoded, .. } => assert!(already_decoded),
        other => panic!("unexpected {other:?}"),
    }
    match decode_body("plain", Some("weird-enc"), None) {
        BodyDecode::Failed { reason, already_decoded, .. } => {
            assert!(reason.contains("Unsupported"));
            assert!(already_decoded);
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn charset_decoding() {
    // ISO-8859-1 / Latin-1: byte 0xE9 is é, 0xA9 is ©.
    let latin1 = vec![b'c', b'a', b'f', b'\xe9', b' ', b'\xa9'];
    assert_eq!(decode_charset(&latin1, Some("text/plain; charset=iso-8859-1")).unwrap(), "caf\u{e9} \u{a9}");
    assert_eq!(decode_charset(&latin1, Some("text/plain; charset=latin1")).unwrap(), "caf\u{e9} \u{a9}");

    // Windows-1252: 0x80 is the euro sign, 0x93/0x94 are curly quotes (differs from Latin-1).
    let cp1252 = vec![0x80, b'x', 0x93, b'y', 0x94];
    assert_eq!(decode_charset(&cp1252, Some("text/html;charset=windows-1252")).unwrap(), "\u{20ac}x\u{201c}y\u{201d}");

    // UTF-16: "hi" little-endian, with and without a BOM; and big-endian.
    assert_eq!(decode_charset(&[b'h', 0, b'i', 0], Some("application/json; charset=utf-16le")).unwrap(), "hi");
    assert_eq!(decode_charset(&[0xFF, 0xFE, b'h', 0, b'i', 0], Some("x; charset=utf-16")).unwrap(), "hi");
    assert_eq!(decode_charset(&[0, b'h', 0, b'i'], Some("x; charset=utf-16be")).unwrap(), "hi");

    // UTF-8, US-ASCII, no charset, and unknown/multibyte -> None (caller keeps lossy UTF-8).
    assert_eq!(decode_charset(&latin1, Some("text/plain; charset=utf-8")), None);
    assert_eq!(decode_charset(&latin1, Some("text/plain")), None);
    assert_eq!(decode_charset(&latin1, None), None);
    assert_eq!(decode_charset(&latin1, Some("text/plain; charset=shift_jis")), None);

    // Through the full body path: a gzipped Latin-1 body comes out decoded per charset.
    let gz = gzip(&latin1);
    match decode_body_bytes(&gz, Some("gzip"), Some("text/plain; charset=iso-8859-1")) {
        BodyDecode::Decoded { text, .. } => assert_eq!(text, "caf\u{e9} \u{a9}"),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn base64_variants() {
    assert_eq!(base64_decode("aGVsbG8=").unwrap(), b"hello");
    assert_eq!(base64_decode("aGVsbG8").unwrap(), b"hello");
    assert_eq!(base64_decode("aGVs\nbG8=").unwrap(), b"hello");
    assert_eq!(base64_decode("-_8").unwrap(), vec![0xfb, 0xff]);
    assert!(base64_decode("!!!").is_err());
    assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
    assert_eq!(run_decode(DecodeOp::Base64Decode, "AP8=").unwrap(), "(binary, 2 bytes)\n00 ff");
}

#[test]
fn hex_roundtrip() {
    assert_eq!(hex_encode(b"Hi!"), "48 69 21");
    assert_eq!(hex_decode("48 69 21").unwrap(), b"Hi!");
    assert_eq!(hex_decode("0x48:0x69,21").unwrap(), b"Hi!");
    assert_eq!(hex_decode("486921").unwrap(), b"Hi!");
    assert!(hex_decode("486").is_err());
    assert!(hex_decode("zz").is_err());
    assert_eq!(run_decode(DecodeOp::HexDecode, "68 69").unwrap(), "hi");
}

#[test]
fn html_entities() {
    assert_eq!(html_decode("a &lt;b&gt; &amp;amp; &#65;&#x42; &copy; &unknown; & alone"), "a <b> &amp; AB \u{a9} &unknown; & alone");
    assert_eq!(html_encode("<a href=\"x\">'&'</a>"), "&lt;a href=&quot;x&quot;&gt;&#39;&amp;&#39;&lt;/a&gt;");
}

#[test]
fn url_ops() {
    assert_eq!(run_decode(DecodeOp::UrlDecode, "a%20b+c%26").unwrap(), "a b c&");
    assert_eq!(run_decode(DecodeOp::UrlEncode, "a b&c").unwrap(), "a%20b%26c");
}

#[test]
fn jwt_header_and_payload() {
    let h = base64_encode(br#"{"alg":"HS256","typ":"JWT"}"#).trim_end_matches('=').replace('+', "-").replace('/', "_");
    let p = base64_encode(br#"{"sub":"123","name":"Jo","iat":1516239022,"exp":1900000000}"#).trim_end_matches('=').replace('+', "-").replace('/', "_");
    let tok = format!("Bearer {h}.{p}.c2lnbmF0dXJl");
    let j = jwt_decode(&tok).unwrap();
    assert!(j.header.contains("\"alg\": \"HS256\""), "{}", j.header);
    assert!(j.payload.contains("\"name\": \"Jo\""));
    assert_eq!(j.signature_bytes, 9);
    assert_eq!(j.times[0], ("iat".to_string(), "2018-01-18 01:30:22 UTC".to_string()));
    assert_eq!(j.times[1].0, "exp");
    let out = run_decode(DecodeOp::Jwt, &tok).unwrap();
    assert!(out.contains("// Payload") && out.contains("not verified"));
    assert!(jwt_decode("a.b").is_err());
    assert!(run_decode(DecodeOp::Jwt, "x.y.z").is_err());
}

#[test]
fn decode_bytes_all_codecs_and_stacks() {
    let text = b"The quick brown fox jumps over the lazy dog. 0123456789";
    let gz = gzip(text);
    assert_eq!(decode_bytes(&gz, "gzip").unwrap(), (vec!["gzip".to_string()], text.to_vec()));

    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    z.write_all(text).unwrap();
    assert_eq!(decode_bytes(&z.finish().unwrap(), "deflate").unwrap().1, text.to_vec());

    let mut br = Vec::new();
    {
        let mut w = brotli::CompressorWriter::new(&mut br, 4096, 5, 22);
        w.write_all(text).unwrap();
    }
    assert_eq!(decode_bytes(&br, "br").unwrap().1, text.to_vec());

    let zs = zstd::stream::encode_all(&text[..], 3).unwrap();
    assert_eq!(decode_bytes(&zs, "zstd").unwrap().1, text.to_vec());

    // "gzip, br" = gzip first, then brotli: decode br then gzip.
    let mut stacked = Vec::new();
    {
        let mut w = brotli::CompressorWriter::new(&mut stacked, 4096, 5, 22);
        w.write_all(&gz).unwrap();
    }
    let (applied, out) = decode_bytes(&stacked, "gzip, br").unwrap();
    assert_eq!(applied, vec!["br".to_string(), "gzip".to_string()]);
    assert_eq!(out, text.to_vec());

    assert!(decode_bytes(b"not gzip", "gzip").is_err());
    assert!(decode_bytes(b"x", "compress").is_err());
}

#[test]
fn pretty_json_handles_jsonp_ndjson_and_xssi() {
    // Plain JSON.
    assert!(pretty_json(r#"{"a":1}"#).contains("\"a\": 1"));
    // JSONP wrapper is preserved with the inner JSON pretty-printed.
    let p = pretty_json("cb({\"a\":1})");
    assert!(p.starts_with("cb("), "{p}");
    assert!(p.contains("\"a\": 1") && p.trim_end().ends_with(");"), "{p}");
    // XSSI guard prefix is stripped.
    assert!(pretty_json(")]}'\n{\"a\":1}").contains("\"a\": 1"));
    assert!(pretty_json("while(1);{\"a\":1}").contains("\"a\": 1"));
    // NDJSON: one value per line.
    let nd = pretty_json("{\"a\":1}\n{\"b\":2}");
    assert!(nd.contains("\"a\": 1") && nd.contains("\"b\": 2"), "{nd}");
    // Non-JSON and empty.
    assert!(pretty_json("nope{").contains("Not valid JSON"));
    assert_eq!(pretty_json("   "), "(Empty body)");
    // A JSON array must not be mistaken for a JSONP call.
    assert!(pretty_json("[1,2]").contains("1,"));
}

#[test]
fn decode_unix_timestamp() {
    // 1700000000 s = 2023-11-14 22:13:20 UTC
    let r = run_decode(DecodeOp::Timestamp, "1700000000").unwrap();
    assert!(r.contains("2023-11-14") && r.contains("22:13:20"), "{r}");
    // Same instant in milliseconds.
    assert!(run_decode(DecodeOp::Timestamp, "1700000000000").unwrap().contains("2023-11-14"));
    assert!(run_decode(DecodeOp::Timestamp, "notanumber").is_err());
}

#[test]
fn json_pretty_and_minify_ops() {
    let p = run_decode(DecodeOp::JsonPretty, r#"{"a":[1,2]}"#).unwrap();
    assert!(p.contains("\"a\": [") && p.contains('\n'), "{p}");
    let m = run_decode(DecodeOp::JsonMinify, "{\n  \"a\": 1,\n  \"b\": 2\n}").unwrap();
    assert_eq!(m, r#"{"a":1,"b":2}"#);
    assert!(run_decode(DecodeOp::JsonMinify, "nope").is_err());
}

#[test]
fn image_sniff_and_detect() {
    use ferrotap::imageview::{decode, is_image, sniff};
    assert_eq!(sniff(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0]), Some("PNG"));
    assert_eq!(sniff(&[0xff, 0xd8, 0xff, 0]), Some("JPEG"));
    assert_eq!(sniff(b"GIF89a....."), Some("GIF"));
    assert!(sniff(b"not an image").is_none());
    assert!(is_image(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a], ""));
    // Content-Type alone marks it an image (e.g. SVG, which decode can't rasterize).
    assert!(is_image(b"<svg/>", "image/svg+xml"));
    assert!(!is_image(b"plain", "text/plain"));
    // decode rejects non-images gracefully.
    assert!(decode(b"not an image at all").is_err());
}
