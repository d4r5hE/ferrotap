//! Insights and other analysis windows over 100k sessions: each must render within a frame budget
//! (debug build) once its numbers are computed.
use eframe::egui;
use ferrotap::model::SessionRecord;
use std::time::{Duration, Instant};

fn session(i: usize) -> SessionRecord {
    let mut s = SessionRecord {
        id: i,
        method: if i % 5 == 0 { "POST" } else { "GET" }.into(),
        url: format!("https://host{}.test/api/v1/items/{}?page={}", i % 40, i % 900, i % 7),
        status: [200, 200, 304, 404, 500][i % 5],
        duration_ms: (i % 900) as _,
        http_version: "HTTP/2.0".into(),
        response_http_version: if i % 3 == 0 { "HTTP/1.1" } else { "HTTP/2.0" }.into(),
        ..Default::default()
    };
    s.request_headers.insert("accept".into(), "application/json".into());
    s.request_headers.insert("cookie".into(), format!("sid={}", i % 50));
    s.response_headers.insert("content-type".into(), "application/json".into());
    s.response_headers.insert("set-cookie".into(), "a=1; Path=/".into());
    s.set_response_body(format!("{{\"id\":{i}}}").as_bytes());
    s
}

#[test]
fn insight_windows_with_100k_sessions() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("FERROTAP_UI_STATE", dir.path().join("ui.json"));
    std::env::set_var("FERROTAP_SETTINGS", dir.path().join("settings.json"));
    std::env::set_var("FERROTAP_SNAPSHOTS", dir.path().join("snapshots"));
    let (_tx, rx) = std::sync::mpsc::channel();
    let mut app = ferrotap::ui::GridApp::new(rx, "test");
    for i in 1..=100_000 {
        app.ingest_session(session(i));
    }
    let ctx = egui::Context::default();
    let input = || egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1600.0, 1000.0))), ..Default::default() };
    type Opener = fn(&mut ferrotap::ui::GridApp, bool);
    let windows: Vec<(&str, Opener)> = vec![
        ("overview", |a, on| a.ext.insights.show_overview = on),
        ("waterfall", |a, on| a.ext.insights.show_waterfall = on),
        ("errors", |a, on| a.ext.insights.show_errors = on),
        ("requests", |a, on| a.ext.insights.show_requests = on),
        ("sizes", |a, on| a.ext.insights.show_sizes = on),
        ("hygiene", |a, on| a.ext.insights.show_hygiene = on),
        ("cookies", |a, on| a.ext.insights.show_cookies = on),
        ("protocols", |a, on| a.ext.show_protocols = on),
    ];
    let mut report = Vec::new();
    let mut slow = Vec::new();
    for (name, open) in windows {
        open(&mut app, true);
        let mut worst = Duration::ZERO;
        for i in 0..4 {
            let t = Instant::now();
            let mut out = ctx.run_ui(input(), |ui| {
                let c = ui.ctx().clone();
                app.insights_windows(&c);
                app.protocols_window(&c);
            });
            out.textures_delta.clear();
            if i > 0 {
                worst = worst.max(t.elapsed());
            }
        }
        open(&mut app, false);
        report.push(format!("{name}: {worst:?}"));
        if worst >= Duration::from_millis(250) {
            slow.push(format!("{name}: {worst:?}"));
        }
    }
    // traffic groups side panel, inside full main-window frames
    app.show_groups_panel = true;
    let mut frame = eframe::Frame::_new_kittest();
    let mut worst = Duration::ZERO;
    for i in 0..4 {
        let t = Instant::now();
        ctx.run_ui(input(), |ui| eframe::App::ui(&mut app, ui, &mut frame)).textures_delta.clear();
        if i > 0 {
            worst = worst.max(t.elapsed());
        }
    }
    report.push(format!("main window + groups: {worst:?}"));
    if worst >= Duration::from_millis(250) {
        slow.push(format!("groups: {worst:?}"));
    }
    eprintln!("{}", report.join(", "));
    assert!(slow.is_empty(), "slow frames: {slow:?}");
}

#[test]
fn other_windows_with_100k_sessions() {
    let dir = tempfile::tempdir().unwrap();
    for (k, f) in [("FERROTAP_UI_STATE", "ui.json"), ("FERROTAP_SETTINGS", "settings.json"), ("FERROTAP_SNAPSHOTS", "snapshots"), ("FERROTAP_RULES_DIR", "rules"), ("FERROTAP_COLLECTIONS_DIR", "collections"), ("FERROTAP_SCRIPTS", "scripts.json"), ("FERROTAP_PLUGINS", "plugins"), ("FERROTAP_PROTOS", "protos.json"), ("FERROTAP_CONN_SETTINGS", "conn.json"), ("FERROTAP_LLM_CACHE", "llm_cache.json"), ("FERROTAP_LLM_PRICES", "llm_prices.json")] {
        std::env::set_var(k, dir.path().join(f));
    }
    let (_tx, rx) = std::sync::mpsc::channel();
    let mut app = ferrotap::ui::GridApp::new(rx, "test");
    for i in 1..=100_000 {
        app.ingest_session(session(i));
    }
    app.select(500);
    let ctx = egui::Context::default();
    let input = || egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1600.0, 1000.0))), ..Default::default() };
    type Opener = fn(&mut ferrotap::ui::GridApp, bool);
    let windows: Vec<(&str, Opener)> = vec![
        ("llm inspector", |a, on| a.ext.llm.show_inspector = on),
        ("llm calls", |a, on| a.ext.llm.show_calls = on),
        ("llm agents", |a, on| a.ext.llm.show_agents = on),
        ("llm cache", |a, on| a.ext.llm.show_cache = on),
        ("messages", |a, on| a.ext.streams.open = on),
        ("export", |a, on| a.ext.data.show_export = on),
        ("snapshots", |a, on| a.ext.data.show_snapshots = on),
        ("report", |a, on| a.ext.data.show_report = on),
        ("api runner", |a, on| a.ext.api.show_runner = on),
        ("composer", |a, on| a.show_composer = on),
        ("rules", |a, on| a.show_rules = on),
        ("filter builder", |a, on| a.show_filter_builder = on),
        ("settings", |a, on| a.ext.qol.show_settings = on),
        ("connections", |a, on| a.ext.conn.show = on),
        ("scripts", |a, on| a.ext.scripts.show = on),
    ];
    let mut report = Vec::new();
    let mut slow = Vec::new();
    for (name, open) in windows {
        open(&mut app, true);
        let mut worst = Duration::ZERO;
        for i in 0..4 {
            let t = Instant::now();
            let mut out = ctx.run_ui(input(), |ui| {
                let c = ui.ctx().clone();
                app.llm_traffic_windows(&c);
                app.streams_windows(&c);
                app.data_io_windows(&c);
                app.api_tools_windows(&c);
                app.composer_window(&c);
                app.rules_window(&c);
                app.filter_builder_window(&c);
                app.qol_windows(&c);
                app.conn_windows(&c);
                app.scripts_windows(&c);
            });
            out.textures_delta.clear();
            if i > 0 {
                worst = worst.max(t.elapsed());
            }
        }
        open(&mut app, false);
        report.push(format!("{name}: {worst:?}"));
        if worst >= Duration::from_millis(250) {
            slow.push(format!("{name}: {worst:?}"));
        }
    }
    eprintln!("{}", report.join(", "));
    assert!(slow.is_empty(), "slow frames: {slow:?}");
}

#[test]
fn compare_two_big_json_bodies() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("FERROTAP_UI_STATE", dir.path().join("ui.json"));
    std::env::set_var("FERROTAP_SETTINGS", dir.path().join("settings.json"));
    let (_tx, rx) = std::sync::mpsc::channel();
    let mut app = ferrotap::ui::GridApp::new(rx, "test");
    for (id, salt) in [(1usize, "a"), (2, "b")] {
        let mut s = session(id);
        let items: Vec<String> = (0..4000).map(|i| format!("{{\"id\":{i},\"v\":\"{}\"}}", if i % 97 == 0 { salt } else { "same" })).collect();
        s.set_response_body(format!("[{}]", items.join(",")).as_bytes());
        app.ingest_session(s);
    }
    app.compare_sessions(1, 2);
    let ctx = egui::Context::default();
    let mut worst = Duration::ZERO;
    for i in 0..4 {
        let t = Instant::now();
        ctx.run_ui(egui::RawInput::default(), |ui| app.compare_window(ui.ctx())).textures_delta.clear();
        if i > 0 {
            worst = worst.max(t.elapsed());
        }
    }
    eprintln!("compare frame: {worst:?}");
    assert!(worst < Duration::from_millis(250), "compare re-diffed every frame: {worst:?}");
}
