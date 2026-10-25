//! G10 pop-out tool windows (embedded viewports when no native backend is present).
use eframe::egui;

fn text_of(ctx: &egui::Context, app: &mut ferrotap::ui::GridApp, key: &str) -> String {
    let mut text = String::new();
    for _ in 0..2 {
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| match key {
            "packets" => app.run_window(ui.ctx(), "packets", |a, c| a.packets_window(c)),
            _ => app.run_window(ui.ctx(), "composer", |a, c| a.composer_window(c)),
        });
        out.textures_delta.clear();
        text.clear();
        for s in &out.shapes {
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
            walk(&s.shape, &mut text);
        }
    }
    text
}

#[test]
fn popped_and_docked_windows_render() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("FERROTAP_COLLECTIONS_DIR", dir.path());
    let (_tx, rx) = std::sync::mpsc::channel();
    if std::env::var_os("FERROTAP_UI_STATE").is_none() { std::env::set_var("FERROTAP_UI_STATE", std::env::temp_dir().join(format!("ferrotap_ui_test_{}.json", std::process::id()))); }
    let mut app = ferrotap::ui::GridApp::new(rx, "test");
    let ctx = egui::Context::default();
    // docked
    app.ext.packets.show = true;
    assert!(text_of(&ctx, &mut app, "packets").contains("Packets"));
    // popped out: still drawn (in its own viewport)
    app.ext.popped.insert("composer".into());
    app.show_composer = true;
    let t = text_of(&ctx, &mut app, "composer");
    assert!(t.contains("Composer"), "{t}");
    assert!(app.ext.popped.contains("composer"));
    assert_eq!(ferrotap::popout::POPOUTS.len(), 7);
}
