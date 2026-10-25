//! Headers tab: "show auto-generated" toggle and Save (download headers).
use eframe::egui;
use ferrotap::model::SessionRecord;
use ferrotap::ui::{inspector_panel, is_auto_generated_header, InspectorTab};

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

#[test]
fn auto_generated_headers_can_be_hidden() {
    assert!(is_auto_generated_header("Content-Length") && is_auto_generated_header("x-ferrotap-error"));
    assert!(!is_auto_generated_header("Authorization"));
    let mut s = SessionRecord { id: 4, method: "GET".into(), url: "https://h.test/".into(), status: 200, ..Default::default() };
    s.request_headers.insert("host".into(), "h.test".into());
    s.request_headers.insert("x-custom-token".into(), "abc".into());
    s.request_headers.insert("content-length".into(), "0".into());
    let ctx = egui::Context::default();
    let mut st = ferrotap::inspect::State::default();
    let (mut rt, mut pt) = (InspectorTab::Headers, InspectorTab::Headers);
    let mut render = |ctx: &egui::Context| {
        let mut text = String::new();
        for _ in 0..2 {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                egui::CentralPanel::default().show(ui, |ui| inspector_panel(ui, &mut rt, &mut pt, &mut st, &s));
            });
            out.textures_delta.clear();
            text.clear();
            for sh in &out.shapes {
                collect_text(&sh.shape, &mut text);
            }
        }
        text
    };
    let t = render(&ctx);
    assert!(t.contains("show auto-generated") && t.contains("Save..."), "{t}");
    assert!(t.contains("x-custom-token") && t.contains("content-length"), "{t}");
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("ferrotap_show_auto_headers"), false));
    let t = render(&ctx);
    assert!(t.contains("x-custom-token"), "{t}");
    assert!(!t.contains("content-length") && !t.contains("\nhost\n"), "{t}");
}
