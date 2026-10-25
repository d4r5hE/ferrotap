//! G2 traffic tabs: open from the selection, add, move, close, next / previous; the bar renders.
use eframe::egui;
use ferrotap::g2::TrafficTabsManager;

#[test]
fn tab_operations() {
    let mut t = TrafficTabsManager::new();
    assert_eq!(t.active_ids(), None, "main tab shows everything");
    let a = t.tab_from("A", vec![1, 2]);
    assert_eq!(t.active_tab_index, a);
    assert_eq!(t.active_ids(), Some(&[1usize, 2][..]));
    t.add_to(a, &[2, 3]);
    assert_eq!(t.tabs[a].session_ids, vec![1, 2, 3]);
    let b = t.tab_from("B", vec![9]);
    t.move_tab(b, -1);
    assert_eq!(t.tabs[1].name, "B");
    assert_eq!(t.active_tab_index, 1, "the active tab follows the move");
    t.move_tab(1, -1);
    assert_eq!(t.tabs[1].name, "B", "the main tab stays first");
    t.next();
    assert_eq!(t.tabs[t.active_tab_index].name, "A");
    t.next();
    assert_eq!(t.active_tab_index, 0, "wraps to the main tab");
    t.prev();
    assert_eq!(t.active_tab_index, 2);
    t.close_tab(2);
    assert_eq!(t.tabs.len(), 2);
    t.close_tab(0);
    assert_eq!(t.tabs.len(), 2, "the main tab cannot be closed");
}

#[test]
fn bar_renders() {
    let (_tx, rx) = std::sync::mpsc::channel();
    if std::env::var_os("FERROTAP_UI_STATE").is_none() { std::env::set_var("FERROTAP_UI_STATE", std::env::temp_dir().join(format!("ferrotap_ui_test_{}.json", std::process::id()))); }
    let mut app = ferrotap::ui::GridApp::new(rx, "test");
    app.tabs_manager.tab_from("Checkout flow", vec![1, 2, 3]);
    let ctx = egui::Context::default();
    let mut text = String::new();
    for _ in 0..2 {
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| app.traffic_tabs_bar(ui));
        out.textures_delta.clear();
        text.clear();
        for s in &out.shapes {
            if let egui::Shape::Text(t) = &s.shape {
                text.push_str(t.galley.text());
                text.push('\n');
            }
        }
    }
    for want in ["Main Capture", "Checkout flow (3)", "+ Tab from selection", "Inspector: below"] {
        assert!(text.contains(want), "missing {want} in {text}");
    }
}
