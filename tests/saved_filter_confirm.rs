//! Saved filters: delete and "Revert to Defaults" ask for confirmation first.
use eframe::egui;
use ferrotap::g2::{AdvancedFilterRule, ConditionOp, FilterCondition, FilterField};

fn render(app: &mut ferrotap::ui::GridApp, ctx: &egui::Context) -> String {
    let mut text = String::new();
    for _ in 0..2 {
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| app.saved_filters_window(ui.ctx()));
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
fn destructive_actions_need_confirmation() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("FERROTAP_SAVED_FILTERS", dir.path().join("saved_filters.json"));
    let (_tx, rx) = std::sync::mpsc::channel();
    if std::env::var_os("FERROTAP_UI_STATE").is_none() { std::env::set_var("FERROTAP_UI_STATE", std::env::temp_dir().join(format!("ferrotap_ui_test_{}.json", std::process::id()))); }
    let mut app = ferrotap::ui::GridApp::new(rx, "test");
    app.saved_filters.add("Only POST", AdvancedFilterRule::single(FilterCondition::new(FilterField::Method, ConditionOp::Equals, "POST")));
    let id = app.saved_filters.filters.last().unwrap().id.clone();
    let n = app.saved_filters.filters.len();
    app.show_saved_filters = true;
    let ctx = egui::Context::default();
    let t = render(&mut app, &ctx);
    assert!(t.contains("Revert to Defaults") && !t.contains("Confirm delete"), "{t}");
    // "x" was clicked on our filter: it asks first and keeps the filter
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("saved_filters_confirm"), id.clone()));
    let t = render(&mut app, &ctx);
    assert!(t.contains("Confirm delete") && t.contains("Cancel"), "{t}");
    assert_eq!(app.saved_filters.filters.len(), n);
    // Revert asks too
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("saved_filters_confirm"), "__revert__".to_string()));
    let t = render(&mut app, &ctx);
    assert!(t.contains("Confirm revert") && !t.contains("Revert to Defaults"), "{t}");
    assert!(app.saved_filters.filters.iter().any(|f| f.id == id), "nothing reverted yet");
}
