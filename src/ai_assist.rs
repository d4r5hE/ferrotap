//! G11 AI assistant panel, using a local model through Ollama (nothing leaves the PC).
//! It answers questions about the selected sessions; their details are masked with the
//! Settings > Privacy rules and cut to a sensible size first.
use std::sync::mpsc;

use serde_json::{json, Value};

use crate::model::{ComposerRequest, SessionRecord};

pub const DEFAULT_URL: &str = "http://127.0.0.1:11434";
const MAX_SESSIONS: usize = 20;
const MAX_BODY: usize = 2000;

fn addr_of(base: &str) -> Result<(String, String), String> {
    let b = base.trim().trim_end_matches('/');
    let rest = b.strip_prefix("http://").ok_or("the Ollama address must start with http://")?;
    let host = rest.split('/').next().unwrap_or("");
    let addr = if host.contains(':') { host.to_string() } else { format!("{host}:80") };
    use std::net::ToSocketAddrs;
    let sa = addr.to_socket_addrs().map_err(|e| format!("{host}: {e}"))?.next().ok_or("no address")?;
    Ok((b.to_string(), sa.to_string()))
}

fn call(base: &str, method: &str, path: &str, body: Option<Value>, timeout: u64) -> Result<Value, String> {
    let (b, addr) = addr_of(base)?;
    let mut headers = vec![];
    if body.is_some() {
        headers.push(("Content-Type".to_string(), "application/json".to_string()));
    }
    let req = ComposerRequest { method: method.into(), url: format!("{b}{path}"), headers, body: body.map(|v| v.to_string()).unwrap_or_default() };
    // straight to Ollama, not through the capture proxy
    let r = crate::model::composer_send_timeout(&req, &addr, timeout);
    if !r.error.is_empty() {
        return Err(format!("Ollama: {}", r.error));
    }
    if r.status != 200 {
        return Err(format!("Ollama answered {}: {}", r.status, r.body.chars().take(300).collect::<String>()));
    }
    serde_json::from_str(&r.body).map_err(|e| format!("Ollama sent something unexpected: {e}"))
}

/// Model names available in Ollama.
pub fn list_models(base: &str) -> Result<Vec<String>, String> {
    let v = call(base, "GET", "/api/tags", None, 5)?;
    Ok(v["models"].as_array().map(|a| a.iter().filter_map(|m| m["name"].as_str().map(str::to_string)).collect()).unwrap_or_default())
}

/// The sessions as text for the model: method, URL, status, timings, headers and (cut) bodies,
/// masked with the privacy settings.
pub fn context(sessions: &[SessionRecord], privacy: &crate::qol::PrivacySettings) -> String {
    let mut s = String::new();
    for r in sessions.iter().take(MAX_SESSIONS) {
        let r = crate::qol::sanitize(r, privacy);
        s.push_str(&format!("### Session #{}: {} {} -> {} ({} ms)\n", r.id, r.method, r.url, r.status, r.duration_ms));
        if !r.comment.is_empty() {
            s.push_str(&format!("Note: {}\n", r.comment));
        }
        for (k, v) in &r.request_headers {
            s.push_str(&format!("> {k}: {}\n", v.chars().take(200).collect::<String>()));
        }
        if !r.request_body.is_empty() {
            s.push_str(&format!("> body: {}\n", r.request_body.chars().take(MAX_BODY).collect::<String>()));
        }
        for (k, v) in &r.response_headers {
            s.push_str(&format!("< {k}: {}\n", v.chars().take(200).collect::<String>()));
        }
        if r.response_body_b64.is_some() {
            s.push_str(&format!("< body: (binary, {} bytes)\n", r.response_bytes().len()));
        } else if !r.response_body.is_empty() {
            s.push_str(&format!("< body: {}\n", r.response_body.chars().take(MAX_BODY).collect::<String>()));
        }
        s.push('\n');
    }
    if sessions.len() > MAX_SESSIONS {
        s.push_str(&format!("({} more sessions not included)\n", sessions.len() - MAX_SESSIONS));
    }
    s
}

pub const SYSTEM: &str = "You are the assistant inside Ferrotap, an HTTP(S) debugging proxy. Answer questions about the \
captured HTTP sessions given below: explain errors, slow requests, headers, caching, cookies, authentication and API \
behaviour. Be concise and refer to sessions by their # number. Some values are masked as [REDACTED].";

/// Ask the model; returns its answer.
pub fn ask(base: &str, model: &str, question: &str, ctx: &str) -> Result<String, String> {
    let body = json!({
        "model": model,
        "stream": false,
        "messages": [
            {"role": "system", "content": SYSTEM},
            {"role": "user", "content": format!("Captured sessions:\n\n{ctx}\nQuestion: {question}")},
        ],
    });
    let v = call(base, "POST", "/api/chat", Some(body), 300)?;
    let a = v["message"]["content"].as_str().unwrap_or("").trim().to_string();
    if a.is_empty() { Err("the model returned no answer".into()) } else { Ok(a) }
}

#[derive(Default)]
pub struct AiUi {
    pub show: bool,
    pub url: String,
    pub models: Option<Result<Vec<String>, String>>,
    pub model: String,
    pub question: String,
    pub answer: Option<Result<String, String>>,
    pub busy: bool,
    rx: Option<mpsc::Receiver<Result<String, String>>>,
}

impl crate::ui::GridApp {
    pub fn ai_menu(&mut self, ui: &mut eframe::egui::Ui) {
        if ui.selectable_label(self.ext.ai.show, "Assistant").on_hover_text("Ask a local model (Ollama) about the selected sessions").clicked() {
            self.ext.ai.show = !self.ext.ai.show;
        }
    }

    pub fn ai_window(&mut self, ctx: &eframe::egui::Context) {
        use eframe::egui::{self, Color32, RichText};
        let st = &mut self.ext.ai;
        if !st.show {
            return;
        }
        if st.url.is_empty() {
            st.url = DEFAULT_URL.into();
        }
        if let Some(rx) = &st.rx {
            match rx.try_recv() {
                Ok(r) => {
                    st.answer = Some(r);
                    st.busy = false;
                    st.rx = None;
                }
                Err(mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(200)),
                Err(_) => {
                    st.busy = false;
                    st.rx = None;
                }
            }
        }
        let ids: std::collections::HashSet<usize> = self.selected_ids().into_iter().collect();
        let chosen: Vec<SessionRecord> = self.model.rows.iter().filter(|r| ids.contains(&r.id)).cloned().collect();
        let st = &mut self.ext.ai;
        let (mut refresh, mut go) = (st.models.is_none(), false);
        let mut open = true;
        egui::Window::new("Assistant").open(&mut open).default_width(620.0).default_height(520.0).show(ctx, |ui| {
            ui.label(RichText::new("Runs on your own Ollama: the selected sessions (masked with Settings > Privacy) go only to that address.").small().color(Color32::GRAY));
            ui.horizontal(|ui| {
                ui.label("Ollama");
                ui.add(egui::TextEdit::singleline(&mut st.url).desired_width(200.0));
                ui.label("Model");
                let models = match &st.models {
                    Some(Ok(m)) => m.clone(),
                    _ => Vec::new(),
                };
                if st.model.is_empty() {
                    if let Some(m) = models.first() {
                        st.model = m.clone();
                    }
                }
                egui::ComboBox::from_id_salt("ai_model").selected_text(if st.model.is_empty() { "(none)".to_string() } else { st.model.clone() }).show_ui(ui, |ui| {
                    for m in &models {
                        ui.selectable_value(&mut st.model, m.clone(), m);
                    }
                });
                if ui.small_button("Refresh").clicked() {
                    refresh = true;
                }
            });
            if let Some(Err(e)) = &st.models {
                ui.colored_label(Color32::from_rgb(230, 160, 40), format!("{e} - is Ollama running?"));
            }
            ui.label(format!("{} session(s) selected", chosen.len()));
            ui.add(egui::TextEdit::multiline(&mut st.question).hint_text("Why does #12 fail?  Which requests are slow and why?").desired_rows(3).desired_width(f32::INFINITY));
            ui.horizontal(|ui| {
                let can = !st.busy && !st.model.is_empty() && !st.question.trim().is_empty() && !chosen.is_empty();
                if ui.add_enabled(can, egui::Button::new(if st.busy { "Thinking..." } else { "Ask" })).clicked() {
                    go = true;
                }
                if chosen.is_empty() {
                    ui.label(RichText::new("Select sessions in the grid first.").small().color(Color32::GRAY));
                }
            });
            ui.separator();
            egui::ScrollArea::vertical().id_salt("ai_answer").show(ui, |ui| match &st.answer {
                Some(Ok(a)) => {
                    ui.label(a);
                }
                Some(Err(e)) => {
                    ui.colored_label(Color32::from_rgb(230, 90, 90), e);
                }
                None => {}
            });
        });
        if refresh {
            st.models = Some(list_models(&st.url));
        }
        if go {
            let (url, model, q) = (st.url.clone(), st.model.clone(), st.question.clone());
            let ctx_text = context(&chosen, &crate::qol::privacy_settings());
            let (tx, rx) = mpsc::channel();
            st.rx = Some(rx);
            st.busy = true;
            st.answer = None;
            std::thread::spawn(move || {
                let _ = tx.send(ask(&url, &model, &q, &ctx_text));
            });
        }
        st.show = open;
    }
}
