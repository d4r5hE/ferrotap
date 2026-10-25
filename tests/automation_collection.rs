//! G16 "record a browsing session into a replayable collection": sessions -> Composer collection
//! (same format as the Composer's collections), saved to and loaded from a temp folder.
use ferrotap::automation::{api_port_from_args, sessions_to_collection};
use ferrotap::model::{load_collections, save_collection, SessionRecord};

fn rec(id: usize, method: &str, url: &str, body: &str) -> SessionRecord {
    let mut r = SessionRecord { id, method: method.into(), url: url.into(), status: 200, request_body: body.into(), ..Default::default() };
    r.request_headers.insert("Host".into(), "shop.test".into());
    r.request_headers.insert("Content-Type".into(), "application/json".into());
    r.request_headers.insert("Content-Length".into(), body.len().to_string());
    r.request_headers.insert("Connection".into(), "keep-alive".into());
    r.request_headers.insert(":authority".into(), "shop.test".into());
    r
}

#[test]
fn sessions_become_saved_requests_in_order() {
    let v = vec![
        rec(7, "POST", "https://shop.test/api/cart?item=42&qty=2", r#"{"add":42}"#),
        rec(3, "GET", "https://shop.test/", ""),
    ];
    let c = sessions_to_collection("Checkout flow", &v);
    assert_eq!(c.name, "Checkout flow");
    assert_eq!(c.requests.len(), 2);
    let first = &c.requests[0];
    assert_eq!(first.name, "001 GET shop.test/");
    assert_eq!(first.url, "https://shop.test/");
    let second = &c.requests[1];
    assert_eq!(second.name, "002 POST shop.test/api/cart");
    assert_eq!(second.method, "POST");
    assert_eq!(second.url, "https://shop.test/api/cart");
    assert_eq!(second.params.len(), 2);
    assert_eq!((second.params[0].key.as_str(), second.params[0].value.as_str()), ("item", "42"));
    assert_eq!(second.body, r#"{"add":42}"#);
    let keys: Vec<&str> = second.headers.iter().map(|h| h.key.as_str()).collect();
    assert!(keys.contains(&"Content-Type") && keys.contains(&"Host"));
    assert!(!keys.iter().any(|k| k.eq_ignore_ascii_case("content-length") || k.eq_ignore_ascii_case("connection") || k.starts_with(':')));
    // Replays as the original request.
    let cr = second.to_composer();
    assert_eq!(cr.url, "https://shop.test/api/cart?item=42&qty=2");
    assert_eq!(cr.method, "POST");
    // The Composer dispatches http:// URLs (https is its own limitation), so check the wire form on one.
    let h = sessions_to_collection("h", &[rec(1, "POST", "http://shop.test/api/cart?item=42&qty=2", r#"{"add":42}"#)]);
    let wire = String::from_utf8(h.requests[0].to_composer().to_wire().unwrap()).unwrap();
    assert!(wire.starts_with("POST http://shop.test/api/cart?item=42&qty=2 HTTP/1.1\r\n"));
    assert!(wire.ends_with(r#"{"add":42}"#));
}

#[test]
fn collection_saves_and_loads_like_composer_collections() {
    let d = tempfile::tempdir().unwrap();
    let c = sessions_to_collection("Recorded / session", &[rec(1, "GET", "https://shop.test/a?x=1", "")]);
    let p = save_collection(d.path(), &c).unwrap();
    assert!(p.starts_with(d.path()));
    let all = load_collections(d.path());
    assert_eq!(all.len(), 1);
    assert_eq!(all[0], c);
}

#[test]
fn gui_api_port_argument() {
    let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(api_port_from_args(&a(&["ferrotap", "--api-port", "8898"])), Some(8898));
    assert_eq!(api_port_from_args(&a(&["ferrotap", "--api-port=0"])), Some(0));
    assert_eq!(api_port_from_args(&a(&["ferrotap"])), None);
    assert_eq!(api_port_from_args(&a(&["ferrotap", "--api-port", "x"])), None);
}
