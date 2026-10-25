//! G3 HTML preview: readable rendering, <base> injection, the Preview tab.
use eframe::egui;
use ferrotap::htmlpreview::*;
use ferrotap::model::SessionRecord;
use ferrotap::ui::{inspector_panel, InspectorTab};

const PAGE: &str = r#"<!doctype html><html><head><title>Shop &amp; more</title>
<style>body { color: red }</style><script>alert("x")</script></head>
<body><h1>Welcome</h1><p>Hello   <b>world</b>.</p><!-- hidden -->
<ul><li>One</li><li>Two</li></ul>
<a href="/cart">Your cart</a><img src="logo.png" alt="Logo"><div>Fin</div></body></html>"#;

#[test]
fn readable_blocks() {
    let b = render(PAGE);
    assert_eq!(b, vec![
        Block::Title("Shop & more".into()),
        Block::Heading(1, "Welcome".into()),
        Block::Text("Hello world.".into()),
        Block::Item("One".into()),
        Block::Item("Two".into()),
        Block::Link("Your cart".into(), "/cart".into()),
        Block::Image("Logo".into(), "logo.png".into()),
        Block::Text("Fin".into()),
    ]);
    assert!(!format!("{b:?}").contains("alert"), "scripts dropped");
}

#[test]
fn base_injection_and_file() {
    assert!(with_base("<html><head><title>x</title>", "https://a.test/p/").starts_with("<html><head><base href=\"https://a.test/p/\"><title>"));
    assert!(with_base("<p>no head</p>", "https://a.test/").starts_with("<base href=\"https://a.test/\"><p>"));
    assert_eq!(with_base("<head><base href=\"/x/\"></head>", "https://a.test/"), "<head><base href=\"/x/\"></head>");
    let p = write_preview(424242, "<head></head><p>hi</p>", "https://a.test/").unwrap();
    assert!(std::fs::read_to_string(&p).unwrap().contains("<base href=\"https://a.test/\">"));
    let _ = std::fs::remove_file(p);
}

#[test]
fn preview_tab_renders() {
    let mut s = SessionRecord { id: 3, method: "GET".into(), url: "https://shop.test/".into(), status: 200, ..Default::default() };
    s.response_headers.insert("content-type".into(), "text/html; charset=utf-8".into());
    s.set_response_body(PAGE.as_bytes());
    let ctx = egui::Context::default();
    let mut st = ferrotap::inspect::State::default();
    let (mut rt, mut pt) = (InspectorTab::Headers, InspectorTab::Preview);
    let mut text = String::new();
    for _ in 0..2 {
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| inspector_panel(ui, &mut rt, &mut pt, &mut st, &s));
        });
        out.textures_delta.clear();
        text.clear();
        for sh in &out.shapes {
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
            walk(&sh.shape, &mut text);
        }
    }
    for want in ["Preview", "Open in browser", "Welcome", "Your cart", "[image \"Logo\": logo.png]"] {
        assert!(text.contains(want), "missing {want} in\n{text}");
    }
}

#[test]
fn huge_pages_are_shortened_for_the_view() {
    let small = render_for_view(PAGE);
    assert!(!small.1, "a normal page is shown in full");
    assert_eq!(small.0, render(PAGE));
    let big = format!("<html><body><p>{}</p>{}</body></html>", "word ".repeat(100_000), "<p>para</p>".repeat(10_000));
    let (blocks, short) = render_for_view(&big);
    assert!(short);
    let total: usize = blocks.iter().map(|b| match b {
        Block::Title(s) | Block::Heading(_, s) | Block::Text(s) | Block::Item(s) => s.len(),
        Block::Link(s, h) | Block::Image(s, h) => s.len() + h.len(),
    }).sum();
    assert!(total <= PREVIEW_MAX + 4 * blocks.len(), "{total}");
    assert!(blocks.iter().all(|b| !matches!(b, Block::Text(s) if s.len() > BLOCK_MAX + 4)));
}
