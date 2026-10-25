//! The inspector can pop out into its own window (embedded when no native backend is present).
use eframe::egui;
use ferrotap::model::SessionRecord;

fn collect_text(shape: &egui::Shape, out: &mut String) {
    match shape {
        egui::Shape::Text(t) => {
            out.push_str(t.galley.text());
            out.push('\n');
        }
        egui::Shape::Vec(v) => v.iter().for_each(|s| collect_text(s, out)),
        _ => {}
    }
}

#[test]
fn popped_out_inspector_renders() {
    let (_tx, rx) = std::sync::mpsc::channel();
    if std::env::var_os("FERROTAP_UI_STATE").is_none() { std::env::set_var("FERROTAP_UI_STATE", std::env::temp_dir().join(format!("ferrotap_ui_test_{}.json", std::process::id()))); }
    let mut app = ferrotap::ui::GridApp::new(rx, "test");
    app.ingest_session(SessionRecord { id: 12, method: "GET".into(), url: "https://pop.test/x".into(), status: 200, ..Default::default() });
    app.ext.popout_inspector = true;
    let ctx = egui::Context::default();
    let mut text = String::new();
    for _ in 0..2 {
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| app.inspector_popout(ui.ctx()));
        out.textures_delta.clear();
        text.clear();
        for s in &out.shapes {
            collect_text(&s.shape, &mut text);
        }
    }
    assert!(text.contains("Dock"), "{text}");
    assert!(app.ext.popout_inspector);
}
