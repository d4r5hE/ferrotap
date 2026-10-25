//! G7 "Open in Composer" / "Save as Composer request" from a captured session.
use ferrotap::model::SessionRecord;

#[test]
fn session_opens_and_saves_into_composer() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("FERROTAP_COLLECTIONS_DIR", dir.path());
    let (_tx, rx) = std::sync::mpsc::channel();
    if std::env::var_os("FERROTAP_UI_STATE").is_none() { std::env::set_var("FERROTAP_UI_STATE", std::env::temp_dir().join(format!("ferrotap_ui_test_{}.json", std::process::id()))); }
    let mut app = ferrotap::ui::GridApp::new(rx, "test");
    let mut s = SessionRecord { id: 5, method: "POST".into(), url: "https://api.example.com/v1/items?limit=2".into(), status: 201, ..Default::default() };
    s.request_headers.insert("content-type".into(), "application/json".into());
    s.request_headers.insert("host".into(), "api.example.com".into());
    s.set_request_body(b"{\"a\":1}");
    s.set_response_body(b"{\"id\":9}");
    app.ingest_session(s);
    app.ingest_session(SessionRecord { id: 6, method: "CONNECT".into(), url: "tunnel://x:443".into(), ..Default::default() });

    assert!(app.session_to_composer(5, false));
    assert!(app.show_composer);
    assert_eq!(app.comp_method, "POST");
    assert_eq!(app.comp_url, "https://api.example.com/v1/items");
    assert!(app.comp_params.iter().any(|p| p.key == "limit" && p.value == "2"));
    assert!(app.comp_hdr_rows.iter().any(|h| h.key == "content-type"));
    assert!(!app.comp_hdr_rows.iter().any(|h| h.key == "host"), "framing headers dropped");
    assert_eq!(app.comp_body, "{\"a\":1}");

    // saving twice gives two uniquely named requests in the collection, with the response as example
    assert!(app.session_to_composer(5, true));
    assert!(app.session_to_composer(5, true));
    let colls = ferrotap::model::load_collections(dir.path());
    let c = colls.iter().find(|c| c.name == "Saved from sessions").expect("collection saved");
    assert_eq!(c.requests.len(), 2);
    assert_ne!(c.requests[0].name, c.requests[1].name);
    assert_eq!(c.requests[0].example.as_ref().map(|e| e.status), Some(201));

    assert!(!app.session_to_composer(6, false), "CONNECT tunnels are refused");
    assert!(!app.session_to_composer(99, false));
}
