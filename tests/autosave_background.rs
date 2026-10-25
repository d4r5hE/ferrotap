//! Auto-save writes on a thread: the frame that starts it stays short even with many sessions,
//! and the file and message arrive afterwards.
use eframe::egui;
use ferrotap::data_io::AutoSaveCfg;
use ferrotap::model::SessionRecord;
use std::time::{Duration, Instant};

#[test]
fn autosave_does_not_block_the_frame() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("FERROTAP_UI_STATE", dir.path().join("ui.json"));
    std::env::set_var("FERROTAP_SETTINGS", dir.path().join("settings.json"));
    std::env::set_var("FERROTAP_SNAPSHOTS", dir.path().join("snapshots"));
    let (_tx, rx) = std::sync::mpsc::channel();
    let mut app = ferrotap::ui::GridApp::new(rx, "test");
    for i in 1..=50_000 {
        let mut s = SessionRecord { id: i, method: "GET".into(), url: format!("https://h{}.test/a/{i}", i % 40), status: 200, ..Default::default() };
        s.response_headers.insert("content-type".into(), "application/json".into());
        s.set_response_body(format!("{{\"id\":{i},\"name\":\"item\"}}").as_bytes());
        app.ingest_session(s);
    }
    let out_dir = dir.path().join("auto");
    app.ext.data.autosave = AutoSaveCfg { enabled: true, interval_min: 1, folder: out_dir.display().to_string(), keep: 3 };
    app.ext.data.autosave_loaded = true;
    app.ext.data.autosave_last = Some(Instant::now() - Duration::from_secs(120));
    let ctx = egui::Context::default();
    let t = Instant::now();
    ctx.run_ui(egui::RawInput::default(), |ui| app.data_io_windows(ui.ctx())).textures_delta.clear();
    let took = t.elapsed();
    assert!(app.ext.data.autosave_msg.starts_with("Auto-saving 50000"), "{}", app.ext.data.autosave_msg);
    assert!(took < Duration::from_millis(400), "the frame waited for the write: {took:?}");
    let start = Instant::now();
    while app.ext.data.autosave_job.is_some() {
        assert!(start.elapsed() < Duration::from_secs(60), "auto-save never finished");
        std::thread::sleep(Duration::from_millis(100));
        ctx.run_ui(egui::RawInput::default(), |ui| app.data_io_windows(ui.ctx())).textures_delta.clear();
    }
    assert!(app.ext.data.autosave_msg.starts_with("Auto-saved 50000"), "{}", app.ext.data.autosave_msg);
    let files: Vec<_> = std::fs::read_dir(&out_dir).unwrap().flatten().collect();
    assert_eq!(files.len(), 1);
}
