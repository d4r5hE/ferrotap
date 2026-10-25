// G10 windows render headlessly (settings, shortcuts, home, toasts) and keys map to actions; no app, no real settings.
use ferrotap::model::SessionRecord;
use ferrotap::qol::*;

fn temp_state() -> (tempfile::TempDir, State) {
    let dir = tempfile::tempdir().unwrap();
    let st = State::with_path(dir.path().join("settings.json"));
    (dir, st)
}

#[test]
fn settings_shortcuts_and_toasts_render() {
    let (_d, mut st) = temp_state();
    st.show_settings = true;
    st.show_shortcuts = true;
    st.toast("hello", false);
    let r = SessionRecord { id: 3, method: "GET".into(), url: "https://h/x?token=abc".into(), ..Default::default() };
    let ctx = egui::Context::default();
    for _ in 0..3 {
        let out = ctx.run_ui(egui::RawInput::default(), |ui| {
            draw_settings(ui.ctx(), &mut st, Some(&r));
            draw_shortcuts(ui.ctx(), &mut st);
            draw_toasts(ui.ctx(), &mut st);
        });
        out.drop_without_applying_deltas();
    }
    assert!(st.show_settings && st.show_shortcuts);
    // The privacy edit buffers are filled from the settings
    assert!(st.privacy_text[2].contains("token"));
    assert_eq!(st.toasts.len(), 1);
}

#[test]
fn home_shows_only_without_sessions_until_dismissed() {
    let (_d, mut st) = temp_state();
    let ctx = egui::Context::default();
    let shown = |st: &mut State, has: bool| {
        let mut n = 0;
        for _ in 0..2 {
            let out = ctx.run_ui(egui::RawInput::default(), |ui| {
                let _ = draw_home(ui.ctx(), st, has);
            });
            n = out.shapes.len();
            out.drop_without_applying_deltas();
        }
        n
    };
    let with_home = shown(&mut st, false);
    let without = shown(&mut st, true);
    assert!(with_home > without, "home window drawn when there are no sessions");
    st.home_dismissed = true;
    assert_eq!(shown(&mut st, false), without);
    st.show_home = true; // Home button / Ctrl+H
    assert!(shown(&mut st, true) > without);
    st.show_home = false;
    st.home_dismissed = false;
    st.settings.show_home = false;
    assert_eq!(shown(&mut st, false), without);
}

fn key(k: egui::Key, ctrl: bool) -> egui::Event {
    let m = if ctrl { egui::Modifiers::COMMAND } else { egui::Modifiers::NONE };
    egui::Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: m }
}

#[test]
fn keys_map_to_actions() {
    let ctx = egui::Context::default();
    let run = |events: Vec<egui::Event>, ctrl: bool| {
        let mut got = Vec::new();
        let _ = ctrl;
        let input = egui::RawInput { events, ..Default::default() };
        ctx.run_ui(input, |ui| got = pressed_actions(ui.ctx())).drop_without_applying_deltas();
        got
    };
    assert_eq!(run(vec![key(egui::Key::F12, false)], false), vec![Action::ToggleCapture]);
    assert_eq!(run(vec![key(egui::Key::E, true)], true), vec![Action::Composer]);
    assert_eq!(run(vec![key(egui::Key::R, true)], true), vec![Action::Rules]);
    assert_eq!(run(vec![key(egui::Key::F, true)], true), vec![Action::Find]);
    assert_eq!(run(vec![key(egui::Key::D, true)], true), vec![Action::Decode]);
    assert_eq!(run(vec![key(egui::Key::Slash, true)], true), vec![Action::Shortcuts]);
    assert_eq!(run(vec![key(egui::Key::F1, false)], false), vec![Action::Shortcuts]);
    assert_eq!(run(vec![key(egui::Key::ArrowDown, false)], false), vec![Action::NextSession]);
    assert_eq!(run(vec![egui::Event::Cut], true), vec![Action::ClearAll]);
    // Plain E (no Ctrl) and the grid's own Ctrl+A do nothing here
    assert!(run(vec![key(egui::Key::E, false)], false).is_empty());
    assert!(run(vec![key(egui::Key::A, true)], true).is_empty());
}
