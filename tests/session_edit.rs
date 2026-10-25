//! G1 edit a captured session in place; a cached response rule built from the edited session.
use ferrotap::model::SessionRecord;
use ferrotap::rules_ext::{plan_request, ReqCtx};
use ferrotap::session_edit::*;

fn sample() -> SessionRecord {
    let mut s = SessionRecord { id: 3, method: "POST".into(), url: "https://api.test/items".into(), status: 200, ..Default::default() };
    s.request_headers.insert("content-type".into(), "application/json".into());
    s.request_headers.insert("content-length".into(), "7".into());
    s.set_request_body(b"{\"a\":1}");
    s.response_headers.insert("set-cookie".into(), "a=1\nb=2".into());
    s.response_headers.insert("content-type".into(), "application/json".into());
    s.set_response_body(b"{\"ok\":true}");
    s
}

#[test]
fn header_text_round_trip() {
    let s = sample();
    let t = headers_to_text(&s.response_headers);
    assert!(t.contains("set-cookie: a=1\nset-cookie: b=2"), "{t}");
    assert_eq!(headers_from_text(&t), s.response_headers);
    assert_eq!(headers_from_text("X-A: 1\nnot a header\nx-a: 2\n: empty"), [("x-a".to_string(), "1, 2".to_string())].into());
}

#[test]
fn draft_applies_edits_and_keeps_binary_bodies() {
    let s = sample();
    let mut d = Draft::from_session(&s);
    d.method = "put".into();
    d.req_body = "{\"a\":22}".into();
    d.status = 503;
    d.resp_body = "down".into();
    d.resp_headers.push_str("\nretry-after: 5");
    let e = d.apply_to(&s);
    assert_eq!(e.method, "PUT");
    assert_eq!(e.request_body, "{\"a\":22}");
    assert_eq!(e.request_header("content-length"), Some("8"), "length follows the edited body");
    assert_eq!((e.status, e.response_body.as_str()), (503, "down"));
    assert_eq!(e.response_header("retry-after"), Some("5"));
    assert_eq!(e.comment, "edited");

    let mut b = sample();
    b.set_response_body(&[0xff, 0x00, 0xfe]);
    let d = Draft::from_session(&b);
    assert!(d.resp_binary);
    assert_eq!(&*d.apply_to(&b).response_bytes(), &[0xff, 0x00, 0xfe]);
}

#[test]
fn edit_in_the_app_and_serve_as_cached_response() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("FERROTAP_RULES_DIR", dir.path());
    let (_tx, rx) = std::sync::mpsc::channel();
    if std::env::var_os("FERROTAP_UI_STATE").is_none() { std::env::set_var("FERROTAP_UI_STATE", std::env::temp_dir().join(format!("ferrotap_ui_test_{}.json", std::process::id()))); }
    let mut app = ferrotap::ui::GridApp::new(rx, "test");
    app.ingest_session(sample());
    app.session_edit_open(3);
    app.ext.edit.draft.as_mut().unwrap().resp_body = "{\"cached\":1}".into();
    let saved = app.session_edit_save().unwrap();
    assert_eq!(saved.response_body, "{\"cached\":1}");
    assert!(app.session_edit_save().is_some(), "saving twice is fine");
    app.session_edit_open(99);
    assert_eq!(app.ext.edit.draft.as_ref().unwrap().id, 3, "unknown id keeps the current draft");

    let rule = ferrotap::rules_ext::rule_from_session(&saved);
    let plan = plan_request(&[rule], &ReqCtx { method: "POST", url: "https://api.test/items", ..Default::default() });
    let resp = plan.response.expect("answered locally");
    assert_eq!(resp.body, b"{\"cached\":1}");
    let other = plan_request(&[ferrotap::rules_ext::rule_from_session(&saved)], &ReqCtx { method: "POST", url: "https://api.test/items/2", ..Default::default() });
    assert!(other.response.is_none(), "only this URL");
}

#[test]
fn large_bodies_are_kept_not_put_in_a_text_box() {
    use eframe::egui;
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("FERROTAP_RULES_DIR", dir.path());
    let (_tx, rx) = std::sync::mpsc::channel();
    if std::env::var_os("FERROTAP_UI_STATE").is_none() { std::env::set_var("FERROTAP_UI_STATE", std::env::temp_dir().join(format!("ferrotap_ui_test_{}.json", std::process::id()))); }
    let mut app = ferrotap::ui::GridApp::new(rx, "test");
    let mut s = sample();
    let big = "line of text\n".repeat(200_000);
    s.set_response_body(big.as_bytes());
    app.ingest_session(s);
    app.session_edit_open(3);
    let ctx = egui::Context::default();
    let mut text = String::new();
    for _ in 0..2 {
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| app.session_edit_window(ui.ctx()));
        out.textures_delta.clear();
        text.clear();
        fn walk(x: &egui::Shape, s: &mut String) {
            match x {
                egui::Shape::Text(t) => {
                    s.push_str(t.galley.text());
                    s.push('\n');
                }
                egui::Shape::Vec(v) => v.iter().for_each(|y| walk(y, s)),
                _ => {}
            }
        }
        out.shapes.iter().for_each(|c| walk(&c.shape, &mut text));
    }
    assert!(text.contains("Large body (2.6 MB): kept as captured"), "{}", &text[..text.len().min(1500)]);
    let saved = app.session_edit_save().unwrap();
    assert_eq!(saved.response_body, big);
}
