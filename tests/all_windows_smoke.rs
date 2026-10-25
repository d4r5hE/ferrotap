//! Smoke test: every tool window open at once over a varied session list, several frames,
//! headless. Catches panics in rendering paths. All data paths point at a temp folder; windows
//! that would call adb / Ollama / certutil get their state pre-filled so nothing external runs.
use eframe::egui;
use ferrotap::model::SessionRecord;
use std::io::Write as _;

fn sessions() -> Vec<SessionRecord> {
    let mut v = Vec::new();
    let mut a = SessionRecord { id: 1, method: "GET".into(), url: "https://shop.test/?q=1".into(), status: 200, duration_ms: 120, http_version: "HTTP/2.0".into(), response_http_version: "HTTP/1.1".into(), ..Default::default() };
    a.response_headers.insert("content-type".into(), "text/html".into());
    a.set_response_body(b"<html><head><title>T</title></head><body><h1>Hi</h1><a href='/x'>x</a></body></html>");
    v.push(a);
    let mut b = SessionRecord { id: 2, method: "POST".into(), url: "https://api.test/v1/items".into(), status: 500, duration_ms: 900, comment: "server broke".into(), ..Default::default() };
    b.request_headers.insert("authorization".into(), "Bearer x".into());
    b.request_headers.insert("cookie".into(), "sid=1".into());
    b.response_headers.insert("content-encoding".into(), "gzip".into());
    b.response_headers.insert("content-type".into(), "application/json".into());
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(b"{\"error\":true}").unwrap();
    b.set_response_body(&e.finish().unwrap());
    v.push(b);
    let mut c = SessionRecord { id: 3, method: "GET".into(), url: "https://img.test/a.png".into(), status: 200, ..Default::default() };
    c.response_headers.insert("content-type".into(), "image/png".into());
    c.set_response_body(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3]);
    v.push(c);
    let mut d = SessionRecord { id: 4, method: "GET".into(), url: "https://live.test/events".into(), status: 200, ..Default::default() };
    d.response_headers.insert("content-type".into(), "text/event-stream".into());
    d.set_response_body(b"data: a\n\ndata: b\n\n");
    v.push(d);
    let mut g = SessionRecord { id: 5, method: "POST".into(), url: "https://grpc.test/shop.v1.Shop/Get".into(), status: 200, ..Default::default() };
    g.response_headers.insert("content-type".into(), "application/grpc".into());
    g.set_response_body(&[0, 0, 0, 0, 2, 8, 1]);
    v.push(g);
    v.push(SessionRecord { id: 6, method: "CONNECT".into(), url: "tunnel://x.test:443".into(), status: 200, ..Default::default() });
    v.push(SessionRecord { id: 7, method: "GET".into(), url: "http://127.0.0.1:9/gone".into(), status: 502, comment: "Upstream failed: refused".into(), ..Default::default() });
    v
}

#[test]
fn every_window_renders_without_panic() {
    let dir = tempfile::tempdir().unwrap();
    for (k, f) in [
        ("FERROTAP_SETTINGS", "settings.json"),
        ("FERROTAP_CONN_SETTINGS", "conn.json"),
        ("FERROTAP_RULES_DIR", "rules"),
        ("FERROTAP_COLLECTIONS_DIR", "collections"),
        ("FERROTAP_SNAPSHOTS", "snapshots"),
        ("FERROTAP_SCRIPTS", "scripts.json"),
        ("FERROTAP_PLUGINS", "plugins"),
        ("FERROTAP_PROTOS", "protos.json"),
        ("FERROTAP_SAVED_FILTERS", "saved_filters.json"),
    ] {
        std::env::set_var(k, dir.path().join(f));
    }
    let (_tx, rx) = std::sync::mpsc::channel();
    if std::env::var_os("FERROTAP_UI_STATE").is_none() { std::env::set_var("FERROTAP_UI_STATE", std::env::temp_dir().join(format!("ferrotap_ui_test_{}.json", std::process::id()))); }
    let mut app = ferrotap::ui::GridApp::new(rx, "test");
    for s in sessions() {
        app.ingest_session(s);
    }
    app.select(2);
    app.ext.sel.set(vec![1, 2, 3]);
    // open everything
    app.show_composer = true;
    app.show_rules = true;
    app.show_breakpoints = true;
    app.show_filter_builder = true;
    app.show_saved_filters = true;
    app.show_groups_panel = true;
    let i = &mut app.ext.insights;
    (i.show_overview, i.show_waterfall, i.show_errors, i.show_requests, i.show_sizes, i.show_hygiene, i.show_cookies) = (true, true, true, true, true, true, true);
    let d = &mut app.ext.data;
    (d.show_export, d.show_import, d.show_snapshots, d.show_autosave, d.show_compare, d.show_report) = (true, true, true, true, true, true);
    let a = &mut app.ext.api;
    (a.show_envs, a.show_runner, a.show_io, a.show_gql) = (true, true, true, true);
    let l = &mut app.ext.llm;
    (l.show_inspector, l.show_calls, l.show_agents, l.show_cache, l.show_prices) = (true, true, true, true, true);
    app.ext.qol.show_settings = true;
    app.ext.qol.show_shortcuts = true;
    app.ext.conn.show = true;
    app.ext.launch.show = true;
    app.ext.scripts.show = true;
    app.ext.packets.show = true;
    app.ext.streams.open = true;
    app.ext.show_protocols = true;
    app.ext.android.show = true;
    app.ext.android.devices = Some(Ok(vec![]));
    app.ext.ai.show = true;
    app.ext.ai.models = Some(Ok(vec!["m".into()]));
    app.replay_adv_open();
    app.session_edit_open(2);
    app.compare_sessions(1, 2);
    let ctx = egui::Context::default();
    let big = || egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1920.0, 1200.0))), ..Default::default() };
    for _ in 0..4 {
        let mut out = ctx.run_ui(big(), |ui| {
            let c = ui.ctx().clone();
            app.composer_window(&c);
            app.rules_window(&c);
            app.breakpoints_ui(&c);
            app.filter_builder_window(&c);
            app.saved_filters_window(&c);
            app.columns_window(&c);
            app.insights_windows(&c);
            app.data_io_windows(&c);
            app.api_tools_windows(&c);
            app.llm_traffic_windows(&c);
            app.qol_windows(&c);
            app.inspect_windows(&c);
            app.automation_windows(&c);
            app.compare_window(&c);
            app.conn_windows(&c);
            app.launch_windows(&c);
            app.scripts_windows(&c);
            app.packets_window(&c);
            app.streams_windows(&c);
            app.protocols_window(&c);
            app.android_window(&c);
            app.ai_window(&c);
            app.replay_adv_window(&c);
            app.session_edit_window(&c);
            app.traffic_tabs_bar(ui);
            app.inspector_popout(&c);
        });
        out.textures_delta.clear();
    }
}
