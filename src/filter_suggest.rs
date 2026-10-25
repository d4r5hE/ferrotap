//! G2 autocomplete for the Filter Builder: value suggestions for every field, taken from the
//! captured sessions (most frequent first), plus header / cookie name suggestions.
use std::collections::HashMap;

use crate::g2::FilterField;
use crate::model::SessionRecord;

/// Sessions scanned for suggestions (the newest ones).
pub const SCAN_LIMIT: usize = 5000;

fn header_values<'a>(h: &'a std::collections::BTreeMap<String, String>, name: &str) -> Vec<&'a str> {
    h.iter().filter(|(k, _)| k.eq_ignore_ascii_case(name)).flat_map(|(_, v)| v.split('\n')).collect()
}

fn cookies(s: &SessionRecord) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = s.request_headers.iter().filter(|(k, _)| k.eq_ignore_ascii_case("cookie")).flat_map(|(_, v)| crate::inspect::parse_cookie_header(v)).collect();
    v.extend(crate::inspect::response_set_cookies(&s.response_headers).into_iter().map(|c| (c.name, c.value)));
    v
}

/// The values of `field` in one session.
pub fn values_of(s: &SessionRecord, field: &FilterField) -> Vec<String> {
    let one = |x: &str| if x.is_empty() { Vec::new() } else { vec![x.to_string()] };
    match field {
        FilterField::Method => one(&s.method),
        FilterField::Host => one(s.host()),
        FilterField::Path => one(s.path().split('?').next().unwrap_or("")),
        FilterField::Url => one(&s.url),
        FilterField::Status => if s.status > 0 { vec![s.status.to_string()] } else { Vec::new() },
        FilterField::ContentType => one(s.content_type().split(';').next().unwrap_or("").trim()),
        FilterField::ReqHeader(n) if n.trim().is_empty() => s.request_headers.keys().cloned().collect(),
        FilterField::ReqHeader(n) => header_values(&s.request_headers, n.trim()).into_iter().map(str::to_string).collect(),
        FilterField::ResHeader(n) if n.trim().is_empty() => s.response_headers.keys().cloned().collect(),
        FilterField::ResHeader(n) => header_values(&s.response_headers, n.trim()).into_iter().map(str::to_string).collect(),
        FilterField::Cookie(n) if n.trim().is_empty() => cookies(s).into_iter().map(|c| c.0).collect(),
        FilterField::Cookie(n) => cookies(s).into_iter().filter(|c| c.0 == n.trim()).map(|c| c.1).collect(),
        FilterField::ProcessName => s.process_name.iter().cloned().collect(),
        FilterField::Pid => s.pid.map(|p| p.to_string()).into_iter().collect(),
        FilterField::ClientIp => s.client_ip.iter().cloned().collect(),
        FilterField::RemoteIp => s.remote_ip.iter().cloned().collect(),
        FilterField::TlsVersion => s.tls_version.iter().cloned().collect(),
        FilterField::QueryParam(n) if n.trim().is_empty() => crate::inspect::parse_query(&s.url).into_iter().map(|(k, _)| k).collect(),
        FilterField::QueryParam(n) => crate::inspect::parse_query(&s.url).into_iter().filter(|(k, _)| k == n.trim()).map(|(_, v)| v).collect(),
        FilterField::Comment => one(&s.comment),
        FilterField::HttpVersion => one(&s.http_version),
        FilterField::ResponseHttpVersion => one(&s.response_http_version),
        FilterField::DurationMs | FilterField::Duration => vec![s.duration_ms.to_string()],
        FilterField::ResponseSize | FilterField::ContentLength => vec![s.response_raw_size.unwrap_or(s.response_body.len()).to_string()],
        FilterField::ConnectMs => s.connect_ms.map(|v| v.to_string()).into_iter().collect(),
        FilterField::TlsMs => s.tls_ms.map(|v| v.to_string()).into_iter().collect(),
        FilterField::TtfbMs => s.ttfb_ms.map(|v| v.to_string()).into_iter().collect(),
        // bodies are too large to suggest from
        _ => Vec::new(),
    }
}

/// Up to `max` suggestions for `field` containing `typed` (case-insensitive), most frequent first;
/// values starting with the typed text rank before ones that only contain it.
pub fn suggest(rows: &[SessionRecord], field: &FilterField, typed: &str, max: usize) -> Vec<String> {
    let t = typed.trim().to_ascii_lowercase();
    let mut counts: HashMap<String, usize> = HashMap::new();
    for s in rows.iter().rev().take(SCAN_LIMIT) {
        for v in values_of(s, field) {
            let v: String = v.chars().take(200).collect();
            if t.is_empty() || v.to_ascii_lowercase().contains(&t) {
                *counts.entry(v).or_default() += 1;
            }
        }
    }
    let mut v: Vec<(String, usize)> = counts.into_iter().filter(|(k, _)| !k.eq_ignore_ascii_case(typed.trim()) || t.is_empty()).collect();
    v.sort_by(|a, b| {
        let pa = !a.0.to_ascii_lowercase().starts_with(&t);
        let pb = !b.0.to_ascii_lowercase().starts_with(&t);
        pa.cmp(&pb).then(b.1.cmp(&a.1)).then(a.0.cmp(&b.0))
    });
    v.into_iter().take(max).map(|(k, _)| k).collect()
}

/// Name suggestions for the header / cookie name box of a builder row.
pub fn suggest_names(rows: &[SessionRecord], field: &FilterField, typed: &str, max: usize) -> Vec<String> {
    let name_field = match field {
        FilterField::ReqHeader(_) => FilterField::ReqHeader(String::new()),
        FilterField::ResHeader(_) => FilterField::ResHeader(String::new()),
        FilterField::Cookie(_) => FilterField::Cookie(String::new()),
        FilterField::QueryParam(_) => FilterField::QueryParam(String::new()),
        _ => return Vec::new(),
    };
    suggest(rows, &name_field, typed, max)
}

/// Show `items` under `anchor` while `open`; returns the clicked one.
pub fn popup(ui: &eframe::egui::Ui, id: eframe::egui::Id, anchor: eframe::egui::Rect, items: &[String]) -> (Option<String>, bool) {
    use eframe::egui;
    if items.is_empty() {
        return (None, false);
    }
    let mut picked = None;
    let r = egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(anchor.left_bottom()).show(ui.ctx(), |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            ui.set_min_width(anchor.width().max(160.0));
            for it in items {
                if ui.selectable_label(false, it.as_str()).clicked() {
                    picked = Some(it.clone());
                }
            }
        });
    });
    (picked, r.response.hovered() || r.response.contains_pointer())
}
