//! G13 HTTP/3 fallback and HTTP/2 view. Browsers learn about QUIC (HTTP/3) from `Alt-Svc`
//! headers and would then bypass the proxy over UDP; Ferrotap removes those offers so clients
//! stay on HTTP/2 / HTTP/1.1 through the proxy (Connections > Keep clients on the proxy).
//! The Protocols window shows, per host, the client's and the server's HTTP versions and flags
//! fallbacks (HTTP/2 from the client, HTTP/1.1 from the server).
use std::collections::BTreeMap;

use crate::model::SessionRecord;

/// `Alt-Svc` without its HTTP/3 / QUIC alternatives. None = nothing left (drop the header).
/// Returns the value unchanged when it offers no HTTP/3.
pub fn strip_h3(alt_svc: &str) -> Option<String> {
    let v = alt_svc.trim();
    if v.eq_ignore_ascii_case("clear") {
        return Some(v.to_string());
    }
    // split on commas outside quotes
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut q = false;
    for c in v.chars() {
        match c {
            '"' => {
                q = !q;
                cur.push(c);
            }
            ',' if !q => parts.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    parts.push(cur);
    let keep: Vec<String> = parts
        .into_iter()
        .map(|p| p.trim().to_string())
        .filter(|p| {
            let id = p.split('=').next().unwrap_or("").trim().to_ascii_lowercase();
            !p.is_empty() && !(id.starts_with("h3") || id.starts_with("quic"))
        })
        .collect();
    if keep.is_empty() { None } else { Some(keep.join(", ")) }
}

/// Does this Alt-Svc value offer HTTP/3?
pub fn offers_h3(alt_svc: &str) -> bool {
    strip_h3(alt_svc).as_deref() != Some(alt_svc.trim())
}

/// One row of the Protocols view.
#[derive(Debug, Clone, PartialEq)]
pub struct ProtoRow {
    pub host: String,
    pub client: String,
    pub server: String,
    pub sessions: usize,
    /// The client spoke HTTP/2 but the server answered over HTTP/1.x.
    pub fallback: bool,
    /// The server offered HTTP/3 (an Alt-Svc offer was seen or removed).
    pub h3_offered: bool,
}

pub fn protocol_report(rows: &[&SessionRecord]) -> Vec<ProtoRow> {
    let mut m: BTreeMap<(String, String, String), (usize, bool)> = BTreeMap::new();
    for s in rows.iter().filter(|s| !s.method.eq_ignore_ascii_case("CONNECT")) {
        let client = if s.http_version.is_empty() { "?".to_string() } else { s.http_version.clone() };
        let server = if s.response_http_version.is_empty() { "?".to_string() } else { s.response_http_version.clone() };
        let h3 = s.comment.contains("HTTP/3 offer removed") || s.response_header("alt-svc").is_some_and(offers_h3);
        let e = m.entry((s.host().to_string(), client, server)).or_default();
        e.0 += 1;
        e.1 |= h3;
    }
    m.into_iter()
        .map(|((host, client, server), (n, h3))| ProtoRow { fallback: client.starts_with("HTTP/2") && server.starts_with("HTTP/1"), host, client, server, sessions: n, h3_offered: h3 })
        .collect()
}

impl crate::ui::GridApp {
    pub fn protocols_window(&mut self, ctx: &eframe::egui::Context) {
        use eframe::egui::{self, Color32, RichText};
        if !self.ext.show_protocols {
            return;
        }
        let rows: Vec<&SessionRecord> = self.model.rows.iter().collect();
        let report = protocol_report(&rows);
        let mut open = true;
        egui::Window::new("Protocols (HTTP/2, HTTP/3)").open(&mut open).default_width(640.0).show(ctx, |ui| {
            let on = crate::conn::current().strip_h3;
            ui.label(RichText::new(if on {
                "HTTP/3 offers (Alt-Svc) are removed, so browsers stay on the proxy over HTTP/2 or HTTP/1.1."
            } else {
                "HTTP/3 offers are passed on: browsers may switch to QUIC and bypass the proxy (Connections > Keep clients on the proxy)."
            }).small().color(Color32::GRAY));
            let fb = report.iter().filter(|r| r.fallback).count();
            ui.label(format!("{} host / version pairs; {fb} with an HTTP/2 -> HTTP/1.1 fallback", report.len()));
            egui::ScrollArea::vertical().show(ui, |ui| {
                egui::Grid::new("proto_grid").striped(true).show(ui, |ui| {
                    for h in ["Host", "Client", "Server", "Sessions", "Notes"] {
                        ui.strong(h);
                    }
                    ui.end_row();
                    for r in &report {
                        ui.label(&r.host);
                        ui.label(&r.client);
                        ui.label(&r.server);
                        ui.label(r.sessions.to_string());
                        let mut notes = Vec::new();
                        if r.fallback {
                            notes.push("fallback to HTTP/1.1");
                        }
                        if r.h3_offered {
                            notes.push("offers HTTP/3");
                        }
                        if r.fallback {
                            ui.colored_label(Color32::from_rgb(230, 160, 40), notes.join(", "));
                        } else {
                            ui.label(notes.join(", "));
                        }
                        ui.end_row();
                    }
                });
            });
        });
        self.ext.show_protocols = open;
    }
}
