//! G3 image viewer: sniffing, decoding, and the Image tab in the response inspector.
use eframe::egui;
use ferrotap::imageview::*;
use ferrotap::model::SessionRecord;
use ferrotap::ui::{inspector_panel, InspectorTab};

fn png(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(w, h, image::Rgba([200, 10, 10, 255]));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

#[test]
fn sniff_and_decode() {
    let p = png(3, 2);
    assert_eq!(sniff(&p), Some("PNG"));
    assert_eq!(sniff(&[0xff, 0xd8, 0xff, 0xe0]), Some("JPEG"));
    assert_eq!(sniff(b"GIF89a...."), Some("GIF"));
    assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some("WebP"));
    assert_eq!(sniff(b"hello"), None);
    assert!(is_image(b"<svg/>", "image/svg+xml"));
    assert!(!is_image(b"{}", "application/json"));
    let (w, h, px) = decode(&p).unwrap();
    assert_eq!((w, h, px.len()), (3, 2, 24));
    assert_eq!(&px[..4], &[200, 10, 10, 255]);
    assert!(decode(b"not an image").is_err());
}

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
fn image_tab_renders() {
    let mut s = SessionRecord { id: 8, method: "GET".into(), url: "https://i.test/a.png".into(), status: 200, ..Default::default() };
    s.response_headers.insert("content-type".into(), "image/png".into());
    s.set_response_body(&png(40, 20));
    assert!(s.response_body_b64.is_some(), "binary body kept exactly");
    let ctx = egui::Context::default();
    let mut st = ferrotap::inspect::State::default();
    let (mut rt, mut pt) = (InspectorTab::Headers, InspectorTab::Image);
    let mut text = String::new();
    let mut saw_image = false;
    for _ in 0..2 {
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| inspector_panel(ui, &mut rt, &mut pt, &mut st, &s));
        });
        text.clear();
        saw_image |= !out.textures_delta.set.is_empty();
        out.textures_delta.clear();
        for sh in &out.shapes {
            collect_text(&sh.shape, &mut text);
        }
    }
    assert!(text.contains("PNG image, 40 x 20 px"), "{text}");
    assert!(text.contains("Image"), "tab offered: {text}");
    assert!(saw_image, "a texture was uploaded");
    assert_eq!(pt, InspectorTab::Image);
}
