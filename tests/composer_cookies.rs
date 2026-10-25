//! G7 Composer cookies editor and raw params / cookies text.
use ferrotap::composer_more::*;
use ferrotap::model::KvRow;

fn kv(k: &str, v: &str) -> KvRow {
    KvRow { enabled: true, key: k.into(), value: v.into() }
}

#[test]
fn raw_pairs_round_trip() {
    let rows = vec![kv("a", "1"), KvRow { enabled: false, key: "b".into(), value: "x=y".into() }, kv("flag", "")];
    let t = pairs_to_raw(&rows);
    assert_eq!(t, "a=1\n# b=x=y\nflag=");
    assert_eq!(raw_to_pairs(&t), rows);
    assert_eq!(raw_to_pairs("  \n=nokey\nsolo"), vec![kv("solo", "")]);
}

#[test]
fn cookie_header_sync() {
    let mut h = vec![kv("Accept", "*/*"), kv("Cookie", "sid=42; theme=dark"), kv("X-A", "1")];
    let mut c = cookies_from_headers(&h);
    assert_eq!(c, vec![kv("sid", "42"), kv("theme", "dark")]);
    c[1].enabled = false;
    c.push(kv("lang", "en"));
    set_cookie_header(&mut h, &c);
    assert_eq!(h[1], kv("Cookie", "sid=42; lang=en"), "header keeps its place");
    assert_eq!(h.len(), 3);
    set_cookie_header(&mut h, &[]);
    assert!(!h.iter().any(|r| r.key == "Cookie"), "no cookies -> no header");
    set_cookie_header(&mut h, &[kv("n", "1")]);
    assert_eq!(h.last().unwrap(), &kv("Cookie", "n=1"));
}

#[test]
fn composer_tab_edits_reach_the_request() {
    let (_tx, rx) = std::sync::mpsc::channel();
    if std::env::var_os("FERROTAP_UI_STATE").is_none() { std::env::set_var("FERROTAP_UI_STATE", std::env::temp_dir().join(format!("ferrotap_ui_test_{}.json", std::process::id()))); }
    let mut app = ferrotap::ui::GridApp::new(rx, "test");
    app.comp_hdr_rows = vec![kv("Cookie", "a=1")];
    let ctx = eframe::egui::Context::default();
    ctx.run_ui(eframe::egui::RawInput::default(), |ui| app.composer_cookies(ui)).textures_delta.clear();
    assert_eq!(app.ext.composer.cookies, vec![kv("a", "1")]);
    // header changed elsewhere (Headers tab): the cookie table reloads
    app.comp_hdr_rows[0].value = "a=2; b=3".into();
    ctx.run_ui(eframe::egui::RawInput::default(), |ui| app.composer_cookies(ui)).textures_delta.clear();
    assert_eq!(app.ext.composer.cookies.len(), 2);
    // raw params mode drives comp_params
    app.ext.composer.params_raw = true;
    app.ext.composer.params_raw_text = "q=rust\n# off=1".into();
    app.comp_params = raw_to_pairs(&app.ext.composer.params_raw_text);
    assert_eq!(app.comp_params.len(), 2);
    assert!(!app.comp_params[1].enabled);
}
