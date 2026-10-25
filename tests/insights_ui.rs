// The insights windows render headlessly over sessions, every window open, without the real app or its data.
use ferrotap::insights::State;
use ferrotap::insights_ui::draw_windows;
use ferrotap::model::*;
use std::collections::BTreeMap;

fn sess(id: usize, url: &str, status: u16, ct: &str, ms: u128) -> SessionRecord {
    let mut resp = BTreeMap::new();
    resp.insert("Content-Type".to_string(), ct.to_string());
    resp.insert("Set-Cookie".to_string(), format!("c{id}=v; Secure"));
    let mut req = BTreeMap::new();
    req.insert("Cookie".to_string(), "a=1".to_string());
    SessionRecord {
        id,
        method: "GET".into(),
        url: url.into(),
        status,
        request_headers: req,
        response_headers: resp,
        response_body: "x".repeat(3000),
        start_time: format!("2026-10-16T12:00:0{}.000+00:00", id % 10),
        duration_ms: ms,
        ..Default::default()
    }
}

fn frame(ctx: &egui::Context, st: &mut State, rows: &[&SessionRecord], sel: Option<&SessionRecord>) -> Option<usize> {
    let mut picked = None;
    let out = ctx.run_ui(egui::RawInput::default(), |ui| {
        picked = draw_windows(ui.ctx(), st, rows, sel);
    });
    out.drop_without_applying_deltas();
    picked
}

#[test]
fn all_windows_render_and_stay_open() {
    let v = vec![
        sess(1, "https://site.com/", 200, "text/html", 120),
        sess(2, "https://site.com/app.js", 200, "application/javascript", 1500),
        sess(3, "https://cdn.other.net/x.png", 404, "image/png", 30),
        sess(4, "https://site.com/app.js", 0, "", 5),
    ];
    let rows: Vec<&SessionRecord> = v.iter().collect();
    let ctx = egui::Context::default();
    let mut st = State::default();
    assert!(!st.any_open());
    st.show_overview = true;
    st.show_waterfall = true;
    st.show_errors = true;
    st.show_requests = true;
    st.show_sizes = true;
    st.show_hygiene = true;
    st.show_cookies = true;
    for _ in 0..3 {
        assert_eq!(frame(&ctx, &mut st, &rows, Some(&v[1])), None);
    }
    assert!(st.any_open() && st.show_overview && st.show_cookies);

    // selected-session scope, an out-of-range page index and an empty capture are all fine
    st.overview_selected_only = true;
    st.page = 99;
    frame(&ctx, &mut st, &rows, Some(&v[1]));
    assert!(st.page < 2);
    frame(&ctx, &mut st, &rows, None);
    frame(&ctx, &mut st, &[], None);
}
