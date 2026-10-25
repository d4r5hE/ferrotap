//! G2 Filter Builder autocomplete: suggestions for every field from the captured sessions.
use eframe::egui;
use ferrotap::filter_suggest::*;
use ferrotap::g2::{ConditionOp, FilterCondition, FilterField};
use ferrotap::model::SessionRecord;

fn rec(id: usize, url: &str, status: u16, process: &str) -> SessionRecord {
    let mut s = SessionRecord { id, method: if id % 2 == 0 { "POST" } else { "GET" }.into(), url: url.into(), status, process_name: Some(process.into()), tls_version: Some("TLSv1.3".into()), ..Default::default() };
    s.request_headers.insert("cookie".into(), format!("sid={id}; theme=dark"));
    s.request_headers.insert("x-trace".into(), format!("t{id}"));
    s.response_headers.insert("content-type".into(), "application/json; charset=utf-8".into());
    s
}

fn rows() -> Vec<SessionRecord> {
    vec![
        rec(1, "https://api.example.com/v1/users?x=1", 200, "chrome.exe"),
        rec(2, "https://api.example.com/v1/items", 404, "chrome.exe"),
        rec(3, "https://cdn.other.net/app.js", 200, "node.exe"),
        rec(4, "https://api.example.com/v1/users", 500, "chrome.exe"),
    ]
}

#[test]
fn suggestions_per_field() {
    let r = rows();
    assert_eq!(suggest(&r, &FilterField::Host, "", 8), vec!["api.example.com", "cdn.other.net"], "most frequent first");
    assert_eq!(suggest(&r, &FilterField::Host, "oth", 8), vec!["cdn.other.net"]);
    assert_eq!(suggest(&r, &FilterField::Path, "/v1/u", 8), vec!["/v1/users"], "query dropped, duplicates merged");
    assert_eq!(suggest(&r, &FilterField::Status, "", 8), vec!["200", "404", "500"]);
    assert_eq!(suggest(&r, &FilterField::ProcessName, "n", 8)[0], "node.exe", "prefix matches first");
    assert_eq!(suggest(&r, &FilterField::ContentType, "", 8), vec!["application/json"]);
    assert_eq!(suggest(&r, &FilterField::TlsVersion, "", 8), vec!["TLSv1.3"]);
    assert_eq!(suggest(&r, &FilterField::Method, "p", 8), vec!["POST"]);
    assert_eq!(suggest(&r, &FilterField::Cookie("theme".into()), "", 8), vec!["dark"]);
    assert!(suggest_names(&r, &FilterField::Cookie(String::new()), "s", 8).contains(&"sid".to_string()));
    assert_eq!(suggest(&r, &FilterField::ReqHeader("x-trace".into()), "t3", 8), Vec::<String>::new(), "exact match is not suggested again");
    assert!(suggest_names(&r, &FilterField::ReqHeader(String::new()), "x-", 8).contains(&"x-trace".to_string()));
    assert_eq!(suggest(&r, &FilterField::Host, "", 1).len(), 1);
    assert!(suggest(&r, &FilterField::RequestBody, "", 8).is_empty());
}

#[test]
fn builder_shows_the_popup_for_the_focused_row() {
    let (_tx, rx) = std::sync::mpsc::channel();
    if std::env::var_os("FERROTAP_UI_STATE").is_none() { std::env::set_var("FERROTAP_UI_STATE", std::env::temp_dir().join(format!("ferrotap_ui_test_{}.json", std::process::id()))); }
    let mut app = ferrotap::ui::GridApp::new(rx, "test");
    for s in rows() {
        app.ingest_session(s);
    }
    app.show_filter_builder = true;
    app.filter_builder_rows = vec![FilterCondition::new(FilterField::Host, ConditionOp::Contains, "api")];
    fn walk(s: &egui::Shape, out: &mut String) {
        match s {
            egui::Shape::Text(t) => {
                out.push_str(t.galley.text());
                out.push('\n');
            }
            egui::Shape::Vec(v) => v.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let ctx = egui::Context::default();
    let mut text = String::new();
    for _ in 0..2 {
        app.ext.fb_suggest = Some((0, false)); // as if the value box had focus
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| app.filter_builder_window(ui.ctx()));
        out.textures_delta.clear();
        text.clear();
        for s in &out.shapes {
            walk(&s.shape, &mut text);
        }
    }
    assert!(text.contains("api.example.com"), "{text}");
}

#[test]
fn query_comment_and_version_fields() {
    use ferrotap::g2::AdvancedFilterRule;
    let mut a = SessionRecord { id: 1, url: "https://s.test/search?q=rust&page=2".into(), comment: "checkout bug".into(), http_version: "HTTP/2.0".into(), response_http_version: "HTTP/1.1".into(), ..Default::default() };
    a.status = 200;
    let b = SessionRecord { id: 2, url: "https://s.test/".into(), http_version: "HTTP/1.1".into(), response_http_version: "HTTP/1.1".into(), ..Default::default() };
    let rows = vec![a, b];
    let hits = |c: FilterCondition| rows.iter().filter(|r| AdvancedFilterRule::single(c.clone()).matches(r)).map(|r| r.id).collect::<Vec<_>>();
    assert_eq!(hits(FilterCondition::new(FilterField::QueryParam("q".into()), ConditionOp::Equals, "rust")), vec![1]);
    assert_eq!(hits(FilterCondition::new(FilterField::QueryParam(String::new()), ConditionOp::Contains, "page=2")), vec![1]);
    assert_eq!(hits(FilterCondition::new(FilterField::Comment, ConditionOp::Contains, "bug")), vec![1]);
    assert_eq!(hits(FilterCondition::new(FilterField::HttpVersion, ConditionOp::Equals, "HTTP/1.1")), vec![2]);
    assert_eq!(hits(FilterCondition::new(FilterField::ResponseHttpVersion, ConditionOp::Equals, "HTTP/1.1")), vec![1, 2]);
    assert_eq!(suggest_names(&rows, &FilterField::QueryParam(String::new()), "", 8), vec!["page", "q"]);
    assert_eq!(suggest(&rows, &FilterField::QueryParam("q".into()), "", 8), vec!["rust"]);
    assert_eq!(suggest(&rows, &FilterField::Comment, "check", 8), vec!["checkout bug"]);
    assert_eq!(suggest(&rows, &FilterField::HttpVersion, "", 8), vec!["HTTP/1.1", "HTTP/2.0"]);
}
