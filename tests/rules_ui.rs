//! Rules and Breakpoints windows render the new controls (rule set import/export, apply + capture,
//! Browse for a response file, Resume all). Rules are kept in a temp folder.
use eframe::egui;

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

fn render(app: &mut ferrotap::ui::GridApp, ctx: &egui::Context) -> String {
    let mut text = String::new();
    for _ in 0..3 {
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            app.rules_window(ui.ctx());
            app.breakpoints_ui(ui.ctx());
            app.ext.pick.show(ui.ctx());
        });
        out.textures_delta.clear();
        text.clear();
        for s in &out.shapes {
            collect_text(&s.shape, &mut text);
        }
    }
    text
}

#[test]
fn rules_and_breakpoint_windows_show_new_controls() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("FERROTAP_RULES_DIR", dir.path());
    let (_tx, rx) = std::sync::mpsc::channel();
    if std::env::var_os("FERROTAP_UI_STATE").is_none() { std::env::set_var("FERROTAP_UI_STATE", std::env::temp_dir().join(format!("ferrotap_ui_test_{}.json", std::process::id()))); }
    let mut app = ferrotap::ui::GridApp::new(rx, "test");
    app.rules = vec![ferrotap::model::Rule { name: "f".into(), pattern: "x".into(), action: "file".into(), ..Default::default() }];
    app.show_rules = true;
    app.show_breakpoints = true;
    let ctx = egui::Context::default();
    let t = render(&mut app, &ctx);
    for want in ["Apply rules + start capture", "Rule set file", "Export", "Import", "replace current rules", "Resume all", "Abort all"] {
        assert!(t.contains(want), "missing {want:?} in:\n{t}");
    }

    // the picker lists a folder and filters by extension
    std::fs::write(dir.path().join("set.json"), "[]").unwrap();
    std::fs::write(dir.path().join("other.txt"), "").unwrap();
    app.ext.pick.open_file("rules_io", "Choose a rule set file", &dir.path().display().to_string(), &["json"]);
    let t = render(&mut app, &ctx);
    assert!(t.contains("Choose a rule set file") && t.contains("set.json"), "{t}");
    assert!(!t.contains("other.txt"), "{t}");
    assert!(app.ext.pick.take("rules_io").is_none(), "nothing picked yet");
}

#[test]
fn connections_window_renders() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("FERROTAP_CONN_SETTINGS", dir.path().join("conn.json"));
    let (_tx, rx) = std::sync::mpsc::channel();
    if std::env::var_os("FERROTAP_UI_STATE").is_none() { std::env::set_var("FERROTAP_UI_STATE", std::env::temp_dir().join(format!("ferrotap_ui_test_{}.json", std::process::id()))); }
    let mut app = ferrotap::ui::GridApp::new(rx, "test");
    app.ext.conn.show = true;
    let ctx = egui::Context::default();
    let mut text = String::new();
    for _ in 0..3 {
        let tall = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1400.0, 4000.0))), ..Default::default() };
        let mut out = ctx.run_ui(tall, |ui| app.conn_windows(ui.ctx()));
        out.textures_delta.clear();
        text.clear();
        for s in &out.shapes {
            collect_text(&s.shape, &mut text);
        }
    }
    for want in ["Connections", "Allow remote devices to connect", "Ignore server certificate errors (unsafe)", "What is recorded", "Streaming mode", "Save"] {
        assert!(text.contains(want), "missing {want:?} in:\n{text}");
    }
}

#[test]
fn intercept_window_renders() {
    let (_tx, rx) = std::sync::mpsc::channel();
    if std::env::var_os("FERROTAP_UI_STATE").is_none() { std::env::set_var("FERROTAP_UI_STATE", std::env::temp_dir().join(format!("ferrotap_ui_test_{}.json", std::process::id()))); }
    let mut app = ferrotap::ui::GridApp::new(rx, "test");
    app.ext.launch.show = true;
    let ctx = egui::Context::default();
    let mut text = String::new();
    for _ in 0..3 {
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| app.launch_windows(ui.ctx()));
        out.textures_delta.clear();
        text.clear();
        for s in &out.shapes {
            collect_text(&s.shape, &mut text);
        }
    }
    for want in ["Intercept", "Browsers (fresh profile)", "Terminals", "PowerShell", "A program", "Start with capture", "Started from Ferrotap (0)"] {
        assert!(text.contains(want), "missing {want:?} in:\n{text}");
    }
}
