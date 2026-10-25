// G10: settings file, sanitize(), shortcut table, theme mapping, status text. Uses temp dirs only.
use ferrotap::model::SessionRecord;
use ferrotap::qol::*;
use std::collections::{BTreeMap, HashSet};

#[test]
fn settings_round_trip_in_temp_dir() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sub").join("settings.json");
    // Missing file -> defaults
    assert_eq!(Settings::load(&path), Settings::default());
    let mut s = Settings::default();
    s.theme = ThemeChoice::Light;
    s.show_home = false;
    s.privacy.headers = vec!["X-Secret".into()];
    s.privacy.cookies = vec!["sid".into()];
    s.privacy.query_params = vec!["key".into()];
    s.privacy.body_regexes = vec![r#""password":"[^"]*""#.into()];
    s.privacy.replacement = "***".into();
    s.notify.errors = true;
    s.notify.capture = false;
    s.save(&path).unwrap();
    assert_eq!(Settings::load(&path), s);
    // A partial / older file still loads, with defaults for the missing fields
    std::fs::write(&path, r#"{"theme":"Dark"}"#).unwrap();
    let l = Settings::load(&path);
    assert_eq!(l.theme, ThemeChoice::Dark);
    assert_eq!(l.privacy, PrivacySettings::default());
    // Garbage -> defaults
    std::fs::write(&path, "not json").unwrap();
    assert_eq!(Settings::load(&path), Settings::default());
}

#[test]
fn settings_path_env_override() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("x.json");
    std::env::set_var("FERROTAP_SETTINGS", &p);
    assert_eq!(Settings::default_path(), p);
    let st = State::with_path(Settings::default_path());
    assert_eq!(st.settings, Settings::default());
    std::env::remove_var("FERROTAP_SETTINGS");
}

fn j(r: &SessionRecord) -> String {
    serde_json::to_string(r).unwrap()
}

fn rec() -> SessionRecord {
    let mut req = BTreeMap::new();
    req.insert("Authorization".to_string(), "Bearer abc123".to_string());
    req.insert("Cookie".to_string(), "sid=s3cret; theme=dark".to_string());
    req.insert("Referer".to_string(), "https://a.test/p?token=zzz&x=1".to_string());
    req.insert("Accept".to_string(), "*/*".to_string());
    let mut resp = BTreeMap::new();
    resp.insert("Set-Cookie".to_string(), "sid=newval; Path=/; HttpOnly\nlang=en; Path=/".to_string());
    resp.insert("x-api-key".to_string(), "k".to_string());
    SessionRecord {
        id: 7,
        method: "POST".into(),
        url: "https://api.test/login?user=bob&token=t0k3n&TOKEN2=x#frag".into(),
        status: 200,
        request_headers: req,
        response_headers: resp,
        request_body: r#"{"user":"bob","password":"hunter2"}"#.into(),
        response_body: r#"{"ok":true,"card":"4111-1111-1111-1111"}"#.into(),
        ..Default::default()
    }
}

#[test]
fn sanitize_masks_headers_cookies_query_and_bodies() {
    let p = PrivacySettings {
        headers: vec!["authorization".into(), "X-API-Key".into()],
        cookies: vec!["SID".into()],
        query_params: vec!["token".into()],
        body_regexes: vec![r#""password":"[^"]*""#.into(), r"\d{4}-\d{4}-\d{4}-\d{4}".into(), "(".into()],
        replacement: "[X]".into(),
    };
    let r = rec();
    let s = sanitize(&r, &p);
    assert_eq!(j(&r), j(&rec()), "input unchanged");
    assert_eq!(s.request_headers["Authorization"], "[X]");
    assert_eq!(s.response_headers["x-api-key"], "[X]");
    assert_eq!(s.request_headers["Accept"], "*/*");
    assert_eq!(s.request_headers["Cookie"], "sid=[X]; theme=dark");
    assert_eq!(s.response_headers["Set-Cookie"], "sid=[X]; Path=/; HttpOnly\nlang=en; Path=/");
    assert_eq!(s.url, "https://api.test/login?user=bob&token=[X]&TOKEN2=x#frag");
    assert_eq!(s.request_headers["Referer"], "https://a.test/p?token=[X]&x=1");
    assert_eq!(s.request_body, r#"{"user":"bob",[X]}"#);
    assert_eq!(s.response_body, r#"{"ok":true,"card":"[X]"}"#);
    assert_eq!(invalid_regexes(&p).len(), 1);
    // Replacement text is literal (no $1 expansion)
    let p2 = PrivacySettings { body_regexes: vec!["(hunter)2".into()], replacement: "$1".into(), ..p };
    assert!(sanitize(&r, &p2).request_body.contains(r#""password":"$1""#));
    // Nothing configured -> identical
    let none = PrivacySettings { headers: vec![], cookies: vec![], query_params: vec![], body_regexes: vec![], replacement: "x".into() };
    assert_eq!(j(&sanitize(&r, &none)), j(&r));
    let text = session_text(&s);
    assert!(text.contains("POST https://api.test/login") && !text.contains("hunter2") && !text.contains("abc123"));
}

#[test]
fn mask_query_edge_cases() {
    let t = vec!["a".to_string()];
    assert_eq!(mask_query("https://h/p", &t, "R"), "https://h/p");
    assert_eq!(mask_query("https://h/p?a", &t, "R"), "https://h/p?a");
    assert_eq!(mask_query("https://h/p?a=1&a=2&b=3", &t, "R"), "https://h/p?a=R&a=R&b=3");
    assert_eq!(mask_query("https://h/p#x?a=1", &t, "R"), "https://h/p#x?a=1");
}

#[test]
fn shortcut_table_has_no_duplicate_keys() {
    let mut seen = HashSet::new();
    for s in SHORTCUTS {
        assert!(seen.insert((format!("{:?}", s.key), s.ctrl)), "duplicate shortcut {}", shortcut_text(s));
    }
    // The required actions all have a key
    for a in [Action::ToggleCapture, Action::ClearAll, Action::Find, Action::Composer, Action::Rules, Action::Decode,
              Action::NextSession, Action::PrevSession, Action::Shortcuts, Action::Settings] {
        assert!(!shortcut_for(a).is_empty(), "{a:?}");
    }
    assert_eq!(shortcut_for(Action::ToggleCapture), "F12");
    assert_eq!(shortcut_for(Action::ClearAll), "Ctrl+X");
    assert_eq!(shortcut_for(Action::Composer), "Ctrl+E");
    assert_eq!(shortcut_for(Action::Shortcuts), "F1");
    assert!(SHORTCUTS.iter().any(|s| shortcut_text(s) == "Ctrl+/"));
    // The grid's own keys are listed but marked built-in
    assert!(SHORTCUTS.iter().filter(|s| s.builtin).count() == 3);
}

#[test]
fn theme_choice_mapping() {
    use egui::{Theme, ThemePreference};
    assert_eq!(ThemeChoice::System.preference(), ThemePreference::System);
    assert_eq!(ThemeChoice::Light.preference(), ThemePreference::Light);
    assert_eq!(ThemeChoice::Dark.preference(), ThemePreference::Dark);
    assert_eq!(ThemeChoice::System.resolve(Some(Theme::Light)), Theme::Light);
    assert_eq!(ThemeChoice::System.resolve(Some(Theme::Dark)), Theme::Dark);
    assert_eq!(ThemeChoice::System.resolve(None), Theme::Dark);
    assert_eq!(ThemeChoice::Light.resolve(Some(Theme::Dark)), Theme::Light);
    assert_eq!(ThemeChoice::Dark.resolve(Some(Theme::Light)), Theme::Dark);
    assert_eq!(ThemeChoice::default(), ThemeChoice::System);
}

#[test]
fn status_bar_text() {
    let i = AppInfo { total: 12, visible: 5, selected: 2, capturing: true, port: 8897, proxy_on: false, active_rules: 3, active_breakpoints: 1, paused: 0 };
    let t = status_text(&i);
    assert_eq!(t, "12 sessions  |  5 visible  |  2 selected  |  capturing  |  port 8897  |  system proxy off  |  rules: 3 active  |  breakpoints: 1 active");
    let t2 = status_text(&AppInfo { capturing: false, paused: 2, proxy_on: true, ..i });
    assert!(t2.contains("paused  |") && t2.contains("system proxy ON") && t2.ends_with("(2 paused)"));
}

#[test]
fn step_selection_moves_in_grid_order() {
    let order = [4, 9, 2];
    assert_eq!(step_selection(&order, None, true), Some(4));
    assert_eq!(step_selection(&order, None, false), Some(2));
    assert_eq!(step_selection(&order, Some(4), true), Some(9));
    assert_eq!(step_selection(&order, Some(2), true), Some(2));
    assert_eq!(step_selection(&order, Some(9), false), Some(4));
    assert_eq!(step_selection(&order, Some(4), false), Some(4));
    assert_eq!(step_selection(&[], Some(1), true), None);
}

#[test]
fn notifications_follow_settings() {
    let mut st = State::default();
    st.settings.notify = NotifySettings { capture: true, rule_hit: true, breakpoint_hit: true, errors: false };
    let base = AppInfo { capturing: true, ..Default::default() };
    st.notice_events(&base, &[]);
    assert!(st.toasts.is_empty(), "first frame only records the state");
    st.notice_events(&AppInfo { capturing: false, ..base.clone() }, &[]);
    assert_eq!(st.toasts.last().unwrap().text, "Capture paused");
    st.notice_events(&AppInfo { capturing: false, paused: 1, ..base.clone() }, &[]);
    assert!(st.toasts.last().unwrap().text.starts_with("Breakpoint hit"));
    let hit = SessionRecord { comment: "AutoResponder: r1".into(), status: 200, ..Default::default() };
    let fail = SessionRecord { status: 502, ..Default::default() };
    let n = st.toasts.len();
    st.notice_events(&AppInfo { capturing: false, paused: 1, ..base.clone() }, &[&hit, &fail]);
    assert_eq!(st.toasts.len(), n + 1, "rule hit shown, errors off");
    assert!(st.toasts.last().unwrap().text.starts_with("Rule hit"));
}

#[test]
fn toolbar_buttons_can_be_hidden() {
    use ferrotap::qol::{Settings, TOOLBAR_ITEMS};
    let mut s = Settings::default();
    assert!(TOOLBAR_ITEMS.iter().all(|(id, _)| s.toolbar_shows(id)));
    s.hidden_toolbar = vec!["terminal".into(), "rules".into()];
    assert!(!s.toolbar_shows("terminal") && !s.toolbar_shows("rules") && s.toolbar_shows("composer"));
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("settings.json");
    s.save(&p).unwrap();
    assert_eq!(Settings::load(&p).hidden_toolbar, vec!["terminal", "rules"]);
    // files written before the option existed still load, with every button shown
    std::fs::write(&p, r#"{"theme":"Dark","show_home":false}"#).unwrap();
    assert!(Settings::load(&p).hidden_toolbar.is_empty());
}
