//! G3 image viewer: detect image bodies (by magic bytes or Content-Type), decode them (PNG, JPEG,
//! GIF, WebP, BMP, ICO) and show them with size facts; the texture is cached per session.
use eframe::egui::{self, Color32, RichText};

/// Image format from the first bytes (SVG is text and reported separately).
pub fn sniff(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        Some("PNG")
    } else if b.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("JPEG")
    } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        Some("GIF")
    } else if b.len() >= 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some("WebP")
    } else if b.starts_with(b"BM") && b.len() > 26 {
        Some("BMP")
    } else if b.starts_with(&[0, 0, 1, 0]) && b.len() > 22 {
        Some("ICO")
    } else {
        None
    }
}

/// Should the Image tab be offered for this body / content type?
pub fn is_image(bytes: &[u8], content_type: &str) -> bool {
    sniff(bytes).is_some() || content_type.trim().to_ascii_lowercase().starts_with("image/")
}

/// Largest image decoded (pixels), to keep memory in check.
pub const MAX_PIXELS: u64 = 40_000_000;

/// Decode to RGBA: (width, height, pixels).
pub fn decode(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().map_err(|e| e.to_string())?;
    let (w, h) = reader.into_dimensions().map_err(|e| format!("not a supported image: {e}"))?;
    if w as u64 * h as u64 > MAX_PIXELS {
        return Err(format!("{w} x {h} is too large to show"));
    }
    let img = image::load_from_memory(bytes).map_err(|e| format!("cannot decode: {e}"))?;
    let rgba = img.to_rgba8();
    Ok((rgba.width(), rgba.height(), rgba.into_raw()))
}

/// Decoded image cached for one session body.
pub struct Cached {
    pub key: (usize, usize),
    pub result: Result<(egui::TextureHandle, u32, u32), String>,
}

#[derive(Default)]
pub struct ImageState {
    pub cached: Option<Cached>,
    pub fit: bool,
}

/// The Image tab: format, dimensions, size, then the picture (fit to the panel or 1:1). `side`
/// ("req"/"resp") keeps the request and response textures distinct when both are images.
pub fn image_view(ui: &mut egui::Ui, st: &mut ImageState, session_id: usize, side: &str, bytes: &[u8], content_type: &str) {
    let fmt = sniff(bytes);
    let key = (session_id, bytes.len());
    if st.cached.as_ref().map(|c| c.key) != Some(key) {
        let result = decode(bytes).map(|(w, h, px)| {
            let ci = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &px);
            (ui.ctx().load_texture(format!("ferrotap_img_{side}_{session_id}"), ci, egui::TextureOptions::LINEAR), w, h)
        });
        st.cached = Some(Cached { key, result });
    }
    let Some(c) = &st.cached else { return };
    match &c.result {
        Ok((tex, w, h)) => {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{} image, {w} x {h} px, {} bytes", fmt.unwrap_or("Image"), bytes.len())).strong());
                ui.checkbox(&mut st.fit, "Fit to panel");
            });
            egui::ScrollArea::both().auto_shrink([false, false]).id_salt("img_scroll").show(ui, |ui| {
                let size = egui::vec2(*w as f32, *h as f32);
                let shown = if st.fit {
                    let avail = ui.available_size();
                    let k = (avail.x / size.x).min(avail.y.max(50.0) / size.y).min(1.0);
                    size * k
                } else {
                    size
                };
                ui.add(egui::Image::new((tex.id(), shown)));
            });
        }
        Err(e) => {
            let svg = content_type.to_ascii_lowercase().contains("svg");
            let msg = if svg { "SVG is vector text: see the XML/HTML tab (SVG is not rendered here).".to_string() } else { format!("Cannot show this image: {e}") };
            ui.label(RichText::new(msg).color(Color32::GRAY));
        }
    }
}
