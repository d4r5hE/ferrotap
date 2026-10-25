//! G1 "edit a captured request / response in place (then replay it)" and "set a session's cached
//! response" (an Auto Responder rule built from the edited session). Opened from the grid's
//! context menu ("Edit session...").
use std::collections::BTreeMap;

use crate::model::SessionRecord;

/// Header map -> editable "Name: value" lines (Set-Cookie values joined by newlines become lines).
pub fn headers_to_text(h: &BTreeMap<String, String>) -> String {
    let mut out = Vec::new();
    for (k, v) in h {
        for line in v.split('\n') {
            out.push(format!("{k}: {line}"));
        }
    }
    out.join("\n")
}

/// "Name: value" lines -> header map; repeated names joined like the proxy does.
pub fn headers_from_text(t: &str) -> BTreeMap<String, String> {
    let mut h: BTreeMap<String, String> = BTreeMap::new();
    for l in t.lines() {
        let Some((k, v)) = l.split_once(':') else { continue };
        let (k, v) = (k.trim().to_ascii_lowercase(), v.trim().to_string());
        if k.is_empty() {
            continue;
        }
        match h.get_mut(&k) {
            Some(prev) => {
                prev.push_str(if k == "set-cookie" { "\n" } else { ", " });
                prev.push_str(&v);
            }
            None => {
                h.insert(k, v);
            }
        }
    }
    h
}

/// Bodies larger than this are not put in a text box (laying them out is very slow); they are
/// kept as captured.
pub const EDIT_MAX: usize = 1_000_000;

fn too_big(ui: &mut eframe::egui::Ui, len: usize) {
    ui.label(eframe::egui::RichText::new(format!("Large body ({:.1} MB): kept as captured. To change it, save it to a file and use Rules > Map Local.", len as f64 / 1e6)).italics());
}

#[derive(Default, Clone, PartialEq, Debug)]
pub struct Draft {
    pub id: usize,
    pub method: String,
    pub url: String,
    pub req_headers: String,
    pub req_body: String,
    /// The request body is binary (kept as captured, not editable as text).
    pub req_binary: bool,
    pub status: u16,
    pub resp_headers: String,
    pub resp_body: String,
    pub resp_binary: bool,
}

impl Draft {
    pub fn from_session(s: &SessionRecord) -> Self {
        Draft {
            id: s.id,
            method: s.method.clone(),
            url: s.url.clone(),
            req_headers: headers_to_text(&s.request_headers),
            req_body: s.request_body.clone(),
            req_binary: s.request_body_b64.is_some(),
            status: s.status,
            resp_headers: headers_to_text(&s.response_headers),
            resp_body: s.response_body.clone(),
            resp_binary: s.response_body_b64.is_some(),
        }
    }

    /// The session with the draft's changes applied (binary bodies stay as captured).
    pub fn apply_to(&self, s: &SessionRecord) -> SessionRecord {
        let mut r = s.clone();
        r.method = self.method.trim().to_ascii_uppercase();
        r.url = self.url.trim().to_string();
        r.request_headers = headers_from_text(&self.req_headers);
        if !self.req_binary {
            r.set_request_body(self.req_body.as_bytes());
            fix_length(&mut r.request_headers, self.req_body.len());
        }
        r.status = self.status;
        r.response_headers = headers_from_text(&self.resp_headers);
        if !self.resp_binary {
            r.set_response_body(self.resp_body.as_bytes());
            r.response_decoded_size = Some(self.resp_body.len());
            fix_length(&mut r.response_headers, self.resp_body.len());
        }
        if !r.comment.contains("edited") {
            r.comment = if r.comment.is_empty() { "edited".into() } else { format!("{} (edited)", r.comment) };
        }
        r
    }
}

/// Keep Content-Length in step with an edited text body (when the header is present).
fn fix_length(h: &mut BTreeMap<String, String>, len: usize) {
    if let Some(v) = h.get_mut("content-length") {
        *v = len.to_string();
    }
}

#[derive(Default)]
pub struct EditUi {
    pub draft: Option<Draft>,
    pub msg: String,
}

impl crate::ui::GridApp {
    /// Open the editor for session `id`.
    pub fn session_edit_open(&mut self, id: usize) {
        if let Some(s) = self.model.rows.iter().find(|r| r.id == id) {
            self.ext.edit.draft = Some(Draft::from_session(s));
            self.ext.edit.msg.clear();
        }
    }

    /// Write the draft back into its session; returns the updated session.
    pub fn session_edit_save(&mut self) -> Option<SessionRecord> {
        let d = self.ext.edit.draft.clone()?;
        let row = self.model.rows.iter_mut().find(|r| r.id == d.id)?;
        *row = d.apply_to(row);
        Some(row.clone())
    }

    pub fn session_edit_window(&mut self, ctx: &eframe::egui::Context) {
        use eframe::egui::{self, Color32, RichText};
        let Some(d) = self.ext.edit.draft.as_mut() else { return };
        let mut open = true;
        let (mut save, mut replay, mut cache_rule) = (false, false, false);
        let msg = self.ext.edit.msg.clone();
        egui::Window::new(format!("Edit session #{}", d.id)).open(&mut open).default_width(720.0).vscroll(true).show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Save").on_hover_text("Change the captured session in place").clicked() {
                    save = true;
                }
                if ui.button("Save and replay").on_hover_text("Save, then send the edited request through the proxy (a new session)").clicked() {
                    save = true;
                    replay = true;
                }
                if ui.button("Serve this response for this URL").on_hover_text("Add an Auto Responder rule that answers this method + URL with the edited response").clicked() {
                    save = true;
                    cache_rule = true;
                }
                ui.label(RichText::new(&msg).color(Color32::GRAY));
            });
            ui.separator();
            ui.columns(2, |cols| {
                let ui = &mut cols[0];
                ui.strong("Request");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut d.method).desired_width(70.0));
                    ui.add(egui::TextEdit::singleline(&mut d.url).desired_width(260.0));
                });
                ui.label("Headers (Name: value per line)");
                ui.add(egui::TextEdit::multiline(&mut d.req_headers).code_editor().desired_rows(8).desired_width(f32::INFINITY));
                ui.label("Body");
                if d.req_binary {
                    ui.label(RichText::new("Binary body: kept as captured (not editable as text).").italics());
                } else if d.req_body.len() > EDIT_MAX {
                    too_big(ui, d.req_body.len());
                } else {
                    ui.add(egui::TextEdit::multiline(&mut d.req_body).code_editor().desired_rows(8).desired_width(f32::INFINITY));
                }
                let ui = &mut cols[1];
                ui.strong("Response");
                ui.horizontal(|ui| {
                    ui.label("Status");
                    ui.add(egui::DragValue::new(&mut d.status).range(0..=999));
                });
                ui.label("Headers (Name: value per line)");
                ui.add(egui::TextEdit::multiline(&mut d.resp_headers).code_editor().desired_rows(8).desired_width(f32::INFINITY));
                ui.label("Body");
                if d.resp_binary {
                    ui.label(RichText::new("Binary body: kept as captured (not editable as text).").italics());
                } else if d.resp_body.len() > EDIT_MAX {
                    too_big(ui, d.resp_body.len());
                } else {
                    ui.add(egui::TextEdit::multiline(&mut d.resp_body).code_editor().desired_rows(8).desired_width(f32::INFINITY));
                }
            });
        });
        if save {
            match self.session_edit_save() {
                Some(s) => {
                    self.ext.edit.msg = format!("Saved session #{}", s.id);
                    if replay {
                        let proxy = crate::ui::our_proxy_server();
                        let req = crate::g1_replay::session_to_request(&s);
                        std::thread::spawn(move || {
                            let _ = crate::model::composer_send(&req, &proxy);
                        });
                        self.ext.edit.msg = format!("Saved; replaying #{} (the result arrives as a new session)", s.id);
                    }
                    if cache_rule {
                        let mut r = crate::rules_ext::rule_from_session(&s);
                        r.name = format!("Cached response of #{}", s.id);
                        self.rules_add(r);
                        self.rules_persist();
                        self.ext.edit.msg = format!("Saved; matching requests now get this response (rule \"Cached response of #{}\")", s.id);
                    }
                }
                None => self.ext.edit.msg = "That session is gone (cleared?)".into(),
            }
        }
        if !open {
            self.ext.edit.draft = None;
        }
    }
}
