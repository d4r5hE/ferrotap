//! The inspector with an 8 MB body (what a large download keeps): every tab renders within a
//! generous frame budget (debug build), repeatedly.
use eframe::egui;
use ferrotap::model::SessionRecord;
use ferrotap::ui::{inspector_panel, InspectorTab};
use std::time::{Duration, Instant};

fn worst_frame(s: &SessionRecord, tab: InspectorTab) -> Duration {
    worst_frame2(s, InspectorTab::Headers, tab)
}

fn worst_frame2(s: &SessionRecord, req: InspectorTab, tab: InspectorTab) -> Duration {
    let ctx = egui::Context::default();
    let mut st = ferrotap::inspect::State::default();
    let (mut req_tab, mut resp_tab) = (req, tab);
    let input = || egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1400.0, 900.0))), ..Default::default() };
    let mut worst = Duration::ZERO;
    for i in 0..4 {
        let t = Instant::now();
        let mut out = ctx.run_ui(input(), |ui| inspector_panel(ui, &mut req_tab, &mut resp_tab, &mut st, s));
        out.textures_delta.clear();
        // the first frame may build caches; the steady state matters
        if i > 0 {
            worst = worst.max(t.elapsed());
        }
    }
    worst
}

#[test]
fn eight_mb_bodies_keep_the_inspector_responsive() {
    let n = 8 * 1024 * 1024;
    let mut text = SessionRecord { id: 1, method: "GET".into(), url: "https://big.test/data.json".into(), status: 200, ..Default::default() };
    text.response_headers.insert("content-type".into(), "application/json".into());
    let mut json = String::from("[");
    while json.len() < n {
        json.push_str("{\"id\":12345,\"name\":\"some item name\",\"ok\":true},");
    }
    json.push_str("{}]");
    text.set_response_body(json.as_bytes());
    let mut bin = SessionRecord { id: 2, method: "GET".into(), url: "https://big.test/file.bin".into(), status: 200, ..Default::default() };
    bin.response_headers.insert("content-type".into(), "application/octet-stream".into());
    bin.set_response_body(&(0..n).map(|i| (i % 251) as u8).collect::<Vec<_>>());

    let mut slow = Vec::new();
    for (s, tabs) in [
        (&text, vec![InspectorTab::Headers, InspectorTab::Raw, InspectorTab::Text, InspectorTab::Json, InspectorTab::Hex, InspectorTab::Sizes, InspectorTab::Preview]),
        (&bin, vec![InspectorTab::Raw, InspectorTab::Text, InspectorTab::Hex, InspectorTab::Image]),
    ] {
        for tab in tabs {
            let d = worst_frame(s, tab);
            eprintln!("{} {tab:?}: {d:?}", s.url);
            if d >= Duration::from_millis(250) {
                slow.push(format!("{} {tab:?}: {d:?}", s.url));
            }
        }
    }
    // a big upload and a big response shown side by side, both formatted
    let mut both = text.clone();
    both.method = "POST".into();
    both.request_headers.insert("content-type".into(), "application/json".into());
    both.set_request_body(json.as_bytes());
    let d = worst_frame2(&both, InspectorTab::Json, InspectorTab::Json);
    eprintln!("request + response JSON: {d:?}");
    if d >= Duration::from_millis(250) {
        slow.push(format!("request + response JSON: {d:?}"));
    }
    assert!(slow.is_empty(), "slow frames (body re-parsed every frame?): {slow:?}");
}
