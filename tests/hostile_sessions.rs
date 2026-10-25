//! Odd and hostile captured data must never crash the window: every inspector tab and the grid
//! render (several frames) over sessions with broken encodings, deep nesting, control characters,
//! multi-byte text cut at odd places and huge header values.
use eframe::egui;
use ferrotap::model::SessionRecord;
use ferrotap::ui::{inspector_panel, InspectorTab};

fn hostile() -> Vec<SessionRecord> {
    let mut v = Vec::new();
    let mut add = |url: &str, ct: &str, ce: Option<&str>, body: &[u8], f: &dyn Fn(&mut SessionRecord)| {
        let mut s = SessionRecord { id: v.len() + 1, method: "GET".into(), url: url.into(), status: 200, ..Default::default() };
        s.response_headers.insert("content-type".into(), ct.into());
        if let Some(ce) = ce {
            s.response_headers.insert("content-encoding".into(), ce.into());
        }
        s.set_response_body(body);
        f(&mut s);
        v.push(s);
    };
    let none = |_: &mut SessionRecord| {};
    add("https://emoji.test/\u{1f600}/\u{e9}t\u{e9}?q=\u{4e2d}\u{6587}&x=%ZZ", "text/plain; charset=utf-8", None, "\u{1f600}\u{1f600}\u{e9}\u{4e2d}".repeat(3000).as_bytes(), &none);
    add("https://ctrl.test/a\u{0}b\u{7}c", "text/plain", None, b"line\r\nwith\x00nul\x1b[31mansi\x7f", &|s| {
        s.request_headers.insert("x-ctl".into(), "a\u{0}b\r\nInjected: yes".into());
    });
    add("https://bad-gzip.test/", "application/json", Some("gzip"), b"\x1f\x8b\x08\x00not really gzip", &none);
    add("https://bad-br.test/", "text/html", Some("br"), b"\xff\xfe\xfd", &none);
    add("https://multi-ce.test/", "text/plain", Some("gzip, deflate, zstd"), b"plain", &none);
    add("https://fake.png.test/x.png", "image/png", None, b"\x89PNG\r\n\x1a\nGARBAGE", &none);
    add("https://deep-json.test/", "application/json", None, format!("{}{}", "[".repeat(100_000), "]".repeat(100_000)).as_bytes(), &none);
    add("https://deep-obj.test/", "application/json", None, "{\"a\":".repeat(50_000).as_bytes(), &none);
    add("https://deep-xml.test/", "application/xml", None, format!("{}{}", "<a>".repeat(50_000), "</a>".repeat(50_000)).as_bytes(), &none);
    add("https://deep-html.test/", "text/html", None, format!("<html><body>{}x{}</body></html>", "<div><p><span>".repeat(20_000), "</span></p></div>".repeat(20_000)).as_bytes(), &none);
    add("https://js.test/a.js", "application/javascript", None, "function(){".repeat(30_000).as_bytes(), &none);
    add("https://huge-header.test/", "text/plain", None, b"ok", &|s| {
        s.response_headers.insert("x-big".into(), "v".repeat(100_000));
        s.response_headers.insert("set-cookie".into(), "a=1; Expires=garbage; SameSite=\u{1f600}\nb".into());
        s.request_headers.insert("cookie".into(), "=;;=x; \u{e9}=\u{1f600}".into());
    });
    add("https://form.test/", "text/plain", None, b"", &|s| {
        s.method = "POST".into();
        s.request_headers.insert("content-type".into(), "multipart/form-data; boundary=".into());
        s.set_request_body(b"--\r\n\r\n--x--\xff\xfe");
    });
    add("weird", "", None, b"", &|s| {
        s.status = 0;
        s.http_version = String::new();
        s.start_time = "not a time".into();
    });
    add("https://trailer.test/", "application/grpc", None, &[0, 0, 0, 0, 200, 1, 2], &|s| {
        s.response_trailers = vec![("grpc-status".into(), "\u{1f600}".into())];
    });
    add("https://sse.test/", "text/event-stream", None, b"data: {broken\n\nid\nretry: x\n\n", &none);
    v
}

#[test]
fn hostile_sessions_render_in_every_tab() {
    let tabs = [
        InspectorTab::Headers, InspectorTab::Raw, InspectorTab::Text, InspectorTab::Json, InspectorTab::Params, InspectorTab::Cookies,
        InspectorTab::Forms, InspectorTab::Xml, InspectorTab::Js, InspectorTab::Trailers, InspectorTab::Timings, InspectorTab::Sizes,
        InspectorTab::Hex, InspectorTab::Image, InspectorTab::Preview,
    ];
    let ctx = egui::Context::default();
    for s in hostile() {
        let mut st = ferrotap::inspect::State::default();
        for tab in tabs {
            let (mut rq, mut rs) = (tab, tab);
            for _ in 0..2 {
                ctx.run_ui(egui::RawInput::default(), |ui| inspector_panel(ui, &mut rq, &mut rs, &mut st, &s)).textures_delta.clear();
            }
        }
    }
}

#[test]
fn hostile_sessions_in_the_main_window() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("FERROTAP_UI_STATE", dir.path().join("ui.json"));
    std::env::set_var("FERROTAP_SETTINGS", dir.path().join("settings.json"));
    let (_tx, rx) = std::sync::mpsc::channel();
    let mut app = ferrotap::ui::GridApp::new(rx, "test");
    let all = hostile();
    let n = all.len();
    for s in all {
        app.ingest_session(s);
    }
    let ctx = egui::Context::default();
    let mut frame = eframe::Frame::_new_kittest();
    let input = || egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1600.0, 1000.0))), ..Default::default() };
    for id in 1..=n {
        app.select(id);
        for _ in 0..2 {
            ctx.run_ui(input(), |ui| eframe::App::ui(&mut app, ui, &mut frame)).textures_delta.clear();
        }
    }
    let i = &mut app.ext.insights;
    (i.show_overview, i.show_waterfall, i.show_errors, i.show_requests, i.show_sizes, i.show_hygiene, i.show_cookies) = (true, true, true, true, true, true, true);
    for _ in 0..2 {
        ctx.run_ui(input(), |ui| {
            let c = ui.ctx().clone();
            app.insights_windows(&c);
            app.protocols_window(&c);
        }).textures_delta.clear();
    }
}
