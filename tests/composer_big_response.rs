//! A huge Composer response is shown shortened (laying out all of it took gigabytes).
use eframe::egui;
use std::time::{Duration, Instant};

#[test]
fn eight_mb_response_is_shortened() {
    let dir = tempfile::tempdir().unwrap();
    for (k, f) in [("FERROTAP_UI_STATE", "ui.json"), ("FERROTAP_SETTINGS", "settings.json"), ("FERROTAP_COLLECTIONS_DIR", "collections")] {
        std::env::set_var(k, dir.path().join(f));
    }
    let (_tx, rx) = std::sync::mpsc::channel();
    let mut app = ferrotap::ui::GridApp::new(rx, "test");
    app.comp_resp = Some(ferrotap::model::ComposerResponse { status: 200, reason: "OK".into(), headers: vec![], body: "abcdefgh\n".repeat(1_000_000), elapsed_ms: 5, error: String::new() });
    app.show_composer = true;
    let ctx = egui::Context::default();
    let mut worst = Duration::ZERO;
    let mut text = String::new();
    for i in 0..4 {
        let t = Instant::now();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| app.composer_window(ui.ctx()));
        if i > 0 {
            worst = worst.max(t.elapsed());
        }
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
    assert!(worst < Duration::from_millis(250), "frame took {worst:?}");
    assert!(text.contains("Showing the first 200000 of 9000000 bytes"), "{}", &text[..text.len().min(2000)]);
}
