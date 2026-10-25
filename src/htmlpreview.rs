//! G3 HTML preview: a readable rendering of an HTML response inside the inspector (headings,
//! paragraphs, lists, links, images; scripts and styles dropped), and "Open in browser" for the
//! full page (written to a temp file with a <base> so relative links and images resolve).
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Title(String),
    Heading(u8, String),
    Text(String),
    Item(String),
    Link(String, String),
    Image(String, String),
}

/// Longest text one preview block shows, and the most text the whole preview shows (laying out
/// megabytes of text in the UI takes gigabytes of memory).
pub const BLOCK_MAX: usize = 20_000;
pub const PREVIEW_MAX: usize = 400_000;

fn cut(s: &mut String, max: usize) -> bool {
    if s.len() <= max {
        return false;
    }
    let mut i = max;
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    s.truncate(i);
    s.push('\u{2026}');
    true
}

/// `render`, shortened for display: long blocks are cut and the total is capped. The flag says
/// whether anything was left out.
pub fn render_for_view(html: &str) -> (Vec<Block>, bool) {
    let mut out = Vec::new();
    let (mut total, mut short) = (0usize, false);
    for mut b in render(html).into_iter() {
        if out.len() >= 5000 || total >= PREVIEW_MAX {
            short = true;
            break;
        }
        let room = BLOCK_MAX.min(PREVIEW_MAX - total);
        let n = match &mut b {
            Block::Title(s) | Block::Heading(_, s) | Block::Text(s) | Block::Item(s) => {
                short |= cut(s, room);
                s.len()
            }
            Block::Link(s, h) | Block::Image(s, h) => {
                short |= cut(s, room);
                s.len() + h.len()
            }
        };
        total += n;
        out.push(b);
    }
    (out, short)
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let key = format!("{name}=");
    let i = lower.find(&key)? + key.len();
    let rest = &tag[i..];
    let v = match rest.chars().next()? {
        q @ ('"' | '\'') => rest[1..].split(q).next()?.to_string(),
        _ => rest.split(|c: char| c.is_whitespace() || c == '>').next()?.to_string(),
    };
    Some(crate::inspect::html_decode(&v))
}

fn clean(s: &str) -> String {
    crate::inspect::html_decode(&s.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// Readable blocks of an HTML document.
pub fn render(html: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut skip: Option<String> = None;
    let mut heading: Option<u8> = None;
    let mut in_li = false;
    let mut link: Option<String> = None;
    let mut title = false;
    let flush = |text: &mut String, out: &mut Vec<Block>, heading: Option<u8>, li: bool, title: bool| {
        let t = clean(text);
        text.clear();
        if t.is_empty() {
            return;
        }
        out.push(if title { Block::Title(t) } else if let Some(h) = heading { Block::Heading(h, t) } else if li { Block::Item(t) } else { Block::Text(t) });
    };
    let mut rest = html;
    while let Some(lt) = rest.find('<') {
        if skip.is_none() {
            text.push_str(&rest[..lt]);
        }
        rest = &rest[lt..];
        if rest.starts_with("<!--") {
            rest = rest.find("-->").map_or("", |e| &rest[e + 3..]);
            continue;
        }
        let Some(gt) = rest.find('>') else { break };
        let tag = &rest[1..gt];
        rest = &rest[gt + 1..];
        let closing = tag.starts_with('/');
        let name: String = tag.trim_start_matches('/').chars().take_while(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
        if let Some(s) = &skip {
            if closing && &name == s {
                skip = None;
            }
            continue;
        }
        match (name.as_str(), closing) {
            ("script" | "style" | "noscript" | "svg" | "template", false) => {
                flush(&mut text, &mut out, heading, in_li, title);
                skip = Some(name.clone());
            }
            ("title", false) => {
                flush(&mut text, &mut out, heading, in_li, title);
                title = true;
            }
            ("title", true) => {
                flush(&mut text, &mut out, heading, in_li, title);
                title = false;
            }
            ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", false) => {
                flush(&mut text, &mut out, heading, in_li, title);
                heading = name[1..].parse().ok();
            }
            ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", true) => {
                flush(&mut text, &mut out, heading, in_li, title);
                heading = None;
            }
            ("li", false) => {
                flush(&mut text, &mut out, heading, in_li, title);
                in_li = true;
            }
            ("li", true) => {
                flush(&mut text, &mut out, heading, in_li, title);
                in_li = false;
            }
            ("a", false) => {
                flush(&mut text, &mut out, heading, in_li, title);
                link = attr(tag, "href");
            }
            ("a", true) => {
                let t = clean(&text);
                text.clear();
                if let Some(h) = link.take() {
                    out.push(Block::Link(if t.is_empty() { h.clone() } else { t }, h));
                } else if !t.is_empty() {
                    out.push(Block::Text(t));
                }
            }
            ("img", _) => {
                if let Some(src) = attr(tag, "src") {
                    out.push(Block::Image(attr(tag, "alt").unwrap_or_default(), src));
                }
            }
            ("p" | "div" | "br" | "tr" | "section" | "article" | "header" | "footer" | "ul" | "ol" | "table" | "form" | "main" | "nav", _) => {
                flush(&mut text, &mut out, heading, in_li, title);
            }
            _ => {}
        }
    }
    if skip.is_none() {
        text.push_str(rest);
    }
    flush(&mut text, &mut out, heading, in_li, title);
    out
}

/// The page with a <base href> so relative links / images load from the original site.
pub fn with_base(html: &str, url: &str) -> String {
    let base = format!("<base href=\"{}\">", url.replace('"', "%22"));
    let lower = html.to_ascii_lowercase();
    if lower.contains("<base ") {
        return html.to_string();
    }
    match lower.find("<head>").map(|i| i + 6).or_else(|| lower.find("<head ").and_then(|i| lower[i..].find('>').map(|j| i + j + 1))) {
        Some(i) => format!("{}{base}{}", &html[..i], &html[i..]),
        None => format!("{base}{html}"),
    }
}

/// Write the page to a temp file for the browser.
pub fn write_preview(session_id: usize, html: &str, url: &str) -> Result<PathBuf, String> {
    let p = std::env::temp_dir().join(format!("ferrotap_preview_{session_id}.html"));
    std::fs::write(&p, with_base(html, url)).map_err(|e| e.to_string())?;
    Ok(p)
}

/// Open a file with its default program (the browser for .html).
pub fn open_with_os(p: &std::path::Path) -> Result<(), String> {
    let mut c = if cfg!(windows) {
        std::process::Command::new("explorer.exe")
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else {
        std::process::Command::new("xdg-open")
    };
    c.arg(p).spawn().map(|_| ()).map_err(|e| format!("cannot open {}: {e}", p.display()))
}

/// The Preview tab.
pub fn preview_view(ui: &mut eframe::egui::Ui, s: &crate::model::SessionRecord, html: &str) {
    use eframe::egui::{self, Color32, RichText};
    ui.horizontal(|ui| {
        if ui.button("Open in browser").on_hover_text("The full page in your default browser (scripts run there)").clicked() {
            let msg = write_preview(s.id, html, &s.url).and_then(|p| open_with_os(&p));
            if let Err(e) = msg {
                ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("preview_err"), e));
            }
        }
        ui.label(RichText::new("Readable view below: no scripts, styles or remote images are loaded.").small().color(Color32::GRAY));
    });
    if let Some(e) = ui.ctx().data(|d| d.get_temp::<String>(egui::Id::new("preview_err"))) {
        ui.colored_label(Color32::from_rgb(230, 90, 90), e);
    }
    ui.separator();
    egui::ScrollArea::vertical().id_salt("html_preview").auto_shrink([false, false]).show(ui, |ui| {
        // Parsing a big page every frame is slow: keep the blocks of the last page shown.
        let key = egui::Id::new(("html_preview_blocks", s.id, html.len()));
        let blocks: std::sync::Arc<(Vec<Block>, bool)> = ui.ctx().data(|d| d.get_temp(key)).unwrap_or_else(|| {
            let b = std::sync::Arc::new(render_for_view(html));
            ui.ctx().data_mut(|d| d.insert_temp(key, b.clone()));
            b
        });
        if blocks.1 {
            ui.label(RichText::new("Long page: the preview is shortened. Open in browser shows all of it.").italics().color(Color32::GRAY));
        }
        for b in blocks.0.iter().cloned() {
            match b {
                Block::Title(t) => {
                    ui.label(RichText::new(t).heading().strong());
                }
                Block::Heading(l, t) => {
                    let size = [22.0, 19.0, 17.0, 15.0, 14.0, 13.0][(l.clamp(1, 6) - 1) as usize];
                    ui.label(RichText::new(t).size(size).strong());
                }
                Block::Text(t) => {
                    ui.label(t);
                }
                Block::Item(t) => {
                    ui.label(format!("  \u{2022} {t}"));
                }
                Block::Link(t, h) => {
                    ui.hyperlink_to(t, h);
                }
                Block::Image(alt, src) => {
                    ui.label(RichText::new(format!("[image{}: {src}]", if alt.is_empty() { String::new() } else { format!(" \"{alt}\"") })).italics().color(Color32::GRAY));
                }
            }
        }
    });
}
