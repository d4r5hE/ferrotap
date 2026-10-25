//! Large captures: the whole main window with 100k sessions must stay responsive (frame time
//! measured headlessly; generous limits because tests run in debug builds).
use eframe::egui;
use ferrotap::model::SessionRecord;
use std::time::{Duration, Instant};

fn session(i: usize) -> SessionRecord {
    let host = format!("host{}.test", i % 40);
    let mut s = SessionRecord {
        id: i,
        method: if i % 5 == 0 { "POST" } else { "GET" }.into(),
        url: format!("https://{host}/api/v1/items/{i}?page={}", i % 7),
        status: [200, 200, 304, 404, 500][i % 5],
        duration_ms: (i % 900) as _,
        http_version: "HTTP/2.0".into(),
        ..Default::default()
    };
    s.request_headers.insert("accept".into(), "application/json".into());
    s.request_headers.insert("user-agent".into(), "stress/1".into());
    s.response_headers.insert("content-type".into(), "application/json".into());
    s.set_response_body(format!("{{\"id\":{i},\"name\":\"item {i}\",\"tags\":[\"a\",\"b\"]}}").as_bytes());
    s
}

fn frames(app: &mut ferrotap::ui::GridApp, ctx: &egui::Context, n: usize) -> Duration {
    let mut frame = eframe::Frame::_new_kittest();
    let input = || egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1600.0, 1000.0))), ..Default::default() };
    let mut worst = Duration::ZERO;
    for _ in 0..n {
        let t = Instant::now();
        let mut out = ctx.run_ui(input(), |ui| eframe::App::ui(app, ui, &mut frame));
        out.textures_delta.clear();
        worst = worst.max(t.elapsed());
    }
    worst
}

#[test]
fn main_window_with_100k_sessions() {
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
    let (tx, rx) = std::sync::mpsc::channel();
    if std::env::var_os("FERROTAP_UI_STATE").is_none() { std::env::set_var("FERROTAP_UI_STATE", std::env::temp_dir().join(format!("ferrotap_ui_test_{}.json", std::process::id()))); }
    let mut app = ferrotap::ui::GridApp::new(rx, "test");
    let t = Instant::now();
    for i in 1..=100_000 {
        app.ingest_session(session(i));
    }
    let ingest = t.elapsed();
    let ctx = egui::Context::default();
    frames(&mut app, &ctx, 2);
    let idle = frames(&mut app, &ctx, 5);
    // live traffic arriving while the window is open
    for i in 100_001..=100_500 {
        tx.send(ferrotap::model::SessionEvent::New(session(i))).unwrap();
    }
    let live = frames(&mut app, &ctx, 3);
    app.select(50_000);
    let selected = frames(&mut app, &ctx, 3);
    app.set_quick_filter("host7.test");
    let filtered = frames(&mut app, &ctx, 3);
    app.set_quick_filter("");
    // a traffic tab holding a fifth of the sessions
    let t = Instant::now();
    let tab = app.tabs_manager.tab_from("big", (1..=20_000).map(|i| i * 5).collect());
    app.tabs_manager.add_to(tab, &(1..=20_000).map(|i| i * 5).collect::<Vec<_>>());
    let tab_build = t.elapsed();
    assert_eq!(app.tabs_manager.tabs[tab].session_ids.len(), 20_000);
    let tabbed = frames(&mut app, &ctx, 3);
    eprintln!("ingest {ingest:?}, worst frame: idle {idle:?}, live {live:?}, selected {selected:?}, filtered {filtered:?}, tab {tabbed:?} (built in {tab_build:?})");
    assert!(tab_build < Duration::from_millis(500), "adding to a tab took {tab_build:?}");
    assert_eq!(app.session_count(), 100_500);
    for (what, d) in [("idle", idle), ("live", live), ("selected", selected), ("filtered", filtered), ("tab", tabbed)] {
        assert!(d < Duration::from_millis(500), "{what} frame took {d:?}");
    }
}
