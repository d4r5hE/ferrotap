//! Clear > Keep: only the most recent N sessions stay; marked and selected ones are never dropped.
use eframe::egui;
use ferrotap::model::{SessionEvent, SessionMark, SessionModel, SessionRecord};

fn rec(id: usize) -> SessionRecord {
    SessionRecord { id, method: "GET".into(), url: format!("https://k.test/{id}"), status: 200, ..Default::default() }
}

#[test]
fn trim_drops_oldest_but_keeps_marked_and_selected() {
    let mut m = SessionModel::default();
    for i in 1..=10 {
        m.apply(SessionEvent::New(rec(i)));
    }
    m.mark_session(2, SessionMark::parse("red"));
    m.selected = Some(3);
    assert_eq!(m.trim_to(5), 5);
    let ids: Vec<usize> = m.rows.iter().map(|r| r.id).collect();
    assert_eq!(ids, vec![2, 3, 8, 9, 10]);
    assert_eq!(m.trim_to(5), 0);
    // only protected ones left over the limit: nothing more can go
    assert_eq!(m.trim_to(1), 3);
    assert_eq!(m.rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![2, 3]);
}

#[test]
fn app_trims_as_traffic_arrives() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("FERROTAP_UI_STATE", dir.path().join("ui_state.json"));
    std::env::set_var("FERROTAP_SETTINGS", dir.path().join("settings.json"));
    let (tx, rx) = std::sync::mpsc::channel();
    let mut app = ferrotap::ui::GridApp::new(rx, "test");
    app.set_keep_sessions(100);
    for i in 1..=250 {
        tx.send(SessionEvent::New(rec(i))).unwrap();
    }
    let ctx = egui::Context::default();
    let mut frame = eframe::Frame::_new_kittest();
    let mut out = ctx.run_ui(egui::RawInput::default(), |ui| eframe::App::ui(&mut app, ui, &mut frame));
    out.textures_delta.clear();
    assert_eq!(app.session_count(), 100);
    app.set_keep_sessions(0);
    for i in 251..=300 {
        tx.send(SessionEvent::New(rec(i))).unwrap();
    }
    let _ = ctx.run_ui(egui::RawInput::default(), |ui| eframe::App::ui(&mut app, ui, &mut frame));
    assert_eq!(app.session_count(), 150);
}

#[test]
fn removing_many_sessions_is_fast() {
    let mut m = SessionModel::default();
    for i in 1..=100_000 {
        m.apply(SessionEvent::New(rec(i)));
    }
    let ids: Vec<usize> = (1..=100_000).filter(|i| i % 2 == 0).collect();
    let t = std::time::Instant::now();
    assert_eq!(m.remove_ids(&ids), 50_000);
    let took = t.elapsed();
    assert!(took < std::time::Duration::from_millis(500), "removing 50k took {took:?}");
    assert!(m.rows.iter().all(|r| r.id % 2 == 1));
}
