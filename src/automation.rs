//! G16 automation: headless capture mode, local REST API to control the app, record a browsing session into a replayable collection, plugin folder.
//! Owned by one builder; the app calls `automation_menu` from the toolbar and `automation_windows` every frame.
//!
//! REST API wiring: `auto_api::ApiServer` runs on its own thread and works on `State::shared`
//! (a snapshot of sessions / capture flag / rules). Every change it makes is also sent as an `ApiCmd`;
//! `automation_windows` drains those commands each frame, applies them to the GridApp and refreshes
//! the snapshot - both under the snapshot lock, so the API never sees a half-applied state.
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::auto_api::{random_token, ApiCmd, ApiServer, Shared, SharedState};
use crate::model::{parse_query, Collection, KvRow, SavedRequest, SessionEvent, SessionRecord};
use crate::ui::GridApp;

/// Default port offered for the REST API (only used when the user enables it).
pub const DEFAULT_API_PORT: u16 = 8898;

/// This module's UI state (kept in `GridApp::ext.auto`).
pub struct State {
    pub show_api: bool,
    pub api_port_text: String,
    pub token: String,
    pub api: Option<ApiServer>,
    pub api_msg: String,
    shared: SharedState,
    cmd_rx: Option<Receiver<ApiCmd>>,
    last_fp: (usize, usize, bool, usize),
    last_sync: Option<Instant>,
    args_checked: bool,
    pub show_coll: bool,
    pub coll_name: String,
    /// Also add response checks, turning the recording into a test.
    pub coll_as_test: bool,
    pub coll_msg: String,
}

impl Default for State {
    fn default() -> Self {
        State {
            show_api: false,
            coll_as_test: false,
            api_port_text: DEFAULT_API_PORT.to_string(),
            token: random_token(),
            api: None,
            api_msg: String::new(),
            shared: Arc::new(Mutex::new(Shared::default())),
            cmd_rx: None,
            last_fp: (usize::MAX, 0, false, 0),
            last_sync: None,
            args_checked: false,
            show_coll: false,
            coll_name: "Recorded session".into(),
            coll_msg: String::new(),
        }
    }
}

/// Request headers that should not be replayed verbatim (the Composer adds its own).
fn skip_header(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with(':')
        || matches!(
            n.as_str(),
            "content-length" | "connection" | "proxy-connection" | "keep-alive" | "transfer-encoding" | "te" | "upgrade" | "proxy-authorization"
        )
}

/// Turn recorded sessions into a Composer collection (one SavedRequest per session, in id order).
/// Request names are "NNN METHOD host/path" so they stay unique and sort in recording order.
pub fn sessions_to_collection(name: &str, sessions: &[SessionRecord]) -> Collection {
    let mut sorted: Vec<&SessionRecord> = sessions.iter().collect();
    sorted.sort_by_key(|s| s.id);
    let mut c = Collection::new(name);
    for (i, s) in sorted.iter().enumerate() {
        let (base, params) = parse_query(&s.url);
        let short = format!("{}{}", s.host(), s.path().split('?').next().unwrap_or(""));
        let short: String = short.chars().take(80).collect();
        let headers: Vec<KvRow> = s.request_headers.iter().filter(|(k, _)| !skip_header(k)).map(|(k, v)| KvRow::new(k, v)).collect();
        c.requests.push(SavedRequest {
            name: format!("{:03} {} {}", i + 1, s.method, short),
            method: s.method.clone(),
            url: base,
            params,
            headers,
            body: s.request_body.clone(),
            ..Default::default()
        });
    }
    c
}

/// Checks that a replay of `s` must pass: the captured status, the Content-Type, the top-level
/// keys of a JSON object body (up to 8) and a generous time budget (3x the capture, at least 2 s).
pub fn tests_for_session(s: &SessionRecord) -> Vec<crate::api_tools::Assertion> {
    use crate::api_tools::{AssertKind, Assertion};
    let mut v = Vec::new();
    if s.status > 0 {
        v.push(Assertion::new(AssertKind::StatusEquals, "", &s.status.to_string()));
    }
    if let Some(ct) = s.response_header("content-type") {
        let base = ct.split(';').next().unwrap_or("").trim();
        if !base.is_empty() {
            v.push(Assertion::new(AssertKind::HeaderExists, "content-type", ""));
        }
    }
    if let Ok(serde_json::Value::Object(o)) = serde_json::from_str::<serde_json::Value>(&s.response_body) {
        for k in o.keys().filter(|k| !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')).take(8) {
            v.push(Assertion::new(AssertKind::JsonPathExists, &format!("$.{k}"), ""));
        }
    }
    let budget = (s.duration_ms as u64).saturating_mul(3).max(2000);
    v.push(Assertion::new(AssertKind::TimeBelow, "", &budget.to_string()));
    v
}

/// The recording as a test: the collection with `tests_for_session` checks on every request.
pub fn sessions_to_test(name: &str, sessions: &[SessionRecord]) -> Collection {
    let mut sorted: Vec<&SessionRecord> = sessions.iter().collect();
    sorted.sort_by_key(|s| s.id);
    let mut c = sessions_to_collection(name, sessions);
    for (r, s) in c.requests.iter_mut().zip(sorted) {
        r.tests = tests_for_session(s);
    }
    c
}

/// Parse `--api-port <n>` / `--api-port=<n>` from the process arguments (for the GUI binary).
pub fn api_port_from_args(args: &[String]) -> Option<u16> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--api-port" {
            return it.next().and_then(|v| v.parse().ok());
        }
        if let Some(v) = a.strip_prefix("--api-port=") {
            return v.parse().ok();
        }
    }
    None
}

impl GridApp {
    /// Toolbar entry (a button or a menu) for this module.
    pub fn automation_menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("Automation", |ui| {
            let api_label = if self.ext.auto.api.is_some() { "REST API / MCP (on)..." } else { "REST API / MCP (off)..." };
            if ui.button(api_label).clicked() {
                self.ext.auto.show_api = true;
                ui.close();
            }
            let n = self.selected_ids().len();
            if ui.add_enabled(n > 0, egui::Button::new(format!("Save {n} selected as collection..."))).clicked() {
                self.ext.auto.show_coll = true;
                self.ext.auto.coll_msg.clear();
                ui.close();
            }
        });
    }

    /// Start the REST API on `port` (0 = any free port). Returns the bound port.
    pub fn automation_start_api(&mut self, port: u16) -> Result<u16, String> {
        self.automation_stop_api();
        let (tx, rx) = channel();
        let shared: SharedState = Arc::new(Mutex::new(Shared::default()));
        {
            let mut sh = shared.lock().unwrap();
            self.automation_fill_snapshot(&mut sh);
        }
        let srv = ApiServer::start(port, self.ext.auto.token.clone(), shared.clone(), tx)?;
        let p = srv.port;
        let a = &mut self.ext.auto;
        a.shared = shared;
        a.cmd_rx = Some(rx);
        a.api = Some(srv);
        a.api_port_text = p.to_string();
        a.api_msg = format!("listening on http://127.0.0.1:{p}");
        Ok(p)
    }

    pub fn automation_stop_api(&mut self) {
        let a = &mut self.ext.auto;
        if let Some(mut s) = a.api.take() {
            s.stop();
            a.api_msg = "stopped".into();
        }
        a.cmd_rx = None;
    }

    fn automation_fill_snapshot(&self, sh: &mut Shared) {
        sh.sessions = self.model.rows.clone();
        sh.capturing = self.capturing;
        sh.rules = self.rules.clone();
        sh.proxy_port = crate::conn::active_port();
        sh.mode = "gui".into();
    }

    /// Apply queued API commands and refresh the API's snapshot (called every frame).
    pub fn automation_sync(&mut self) {
        let Some(rx) = self.ext.auto.cmd_rx.take() else { return };
        let shared = self.ext.auto.shared.clone();
        let mut sh = match shared.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        let mut changed = false;
        while let Ok(c) = rx.try_recv() {
            changed = true;
            match c {
                ApiCmd::StartCapture => self.capturing = true,
                ApiCmd::StopCapture => self.capturing = false,
                ApiCmd::ClearSessions => {
                    self.model.apply(SessionEvent::Clear);
                    self.ext.sel.clear();
                }
                ApiCmd::SetRules(r) => {
                    self.rules = r;
                    self.rules_persist();
                }
                ApiCmd::Mark(ids, m) => {
                    for id in ids {
                        self.model.mark_session(id, m);
                    }
                }
                ApiCmd::DeleteSessions(ids) => {
                    self.model.remove_ids(&ids);
                    self.ext.sel.retain_existing(&self.model.rows);
                }
                ApiCmd::SelectSessions(ids) => {
                    self.model.selected = ids.first().copied();
                    self.ext.sel.set(ids);
                }
                ApiCmd::Launch(what) => {
                    if what == "terminal" { self.open_terminal() } else { self.open_clean_browser() }
                }
                ApiCmd::SystemProxy(on) => {
                    if self.proxy_is_on() != on {
                        self.toggle_system_proxy();
                    }
                }
                ApiCmd::OpenHttpsSettings => self.show_https = true,
                ApiCmd::SetFilter(t) => self.model.filter = t,
                ApiCmd::ReloadAutosave => self.ext.data.autosave_loaded = false,
            }
        }
        self.ext.auto.cmd_rx = Some(rx);
        let fp = (self.model.rows.len(), self.model.rows.last().map_or(0, |r| r.id), self.capturing, self.rules.len());
        let stale = self.ext.auto.last_sync.is_none_or(|t| t.elapsed() > Duration::from_secs(2));
        if changed || stale || fp != self.ext.auto.last_fp {
            self.automation_fill_snapshot(&mut sh);
            self.ext.auto.last_fp = fp;
            self.ext.auto.last_sync = Some(Instant::now());
        }
    }

    /// Windows this module draws each frame.
    pub fn automation_windows(&mut self, ctx: &egui::Context) {
        if !self.ext.auto.args_checked {
            self.ext.auto.args_checked = true;
            let args: Vec<String> = std::env::args().collect();
            if let Some(p) = api_port_from_args(&args) {
                match self.automation_start_api(p) {
                    Ok(p) => eprintln!("REST API on http://127.0.0.1:{p}  token: {}", self.ext.auto.token),
                    Err(e) => self.ext.auto.api_msg = e,
                }
            }
        }
        self.automation_sync();
        if self.ext.auto.show_api {
            self.automation_api_window(ctx);
        }
        if self.ext.auto.show_coll {
            self.automation_collection_window(ctx);
        }
    }

    fn automation_api_window(&mut self, ctx: &egui::Context) {
        let mut open = true;
        let mut want_on: Option<bool> = None;
        let mut regen = false;
        let on = self.ext.auto.api.is_some();
        egui::Window::new("Automation settings: REST API and MCP").open(&mut open).default_width(560.0).show(ctx, |ui| {
            let a = &mut self.ext.auto;
            ui.label("Local REST API to control this window (127.0.0.1 only, needs the token). Off by default.");
            ui.horizontal(|ui| {
                let mut en = on;
                if ui.checkbox(&mut en, "Enable REST API").changed() {
                    want_on = Some(en);
                }
                ui.label("Port:");
                ui.add_enabled(!on, egui::TextEdit::singleline(&mut a.api_port_text).desired_width(60.0));
            });
            ui.horizontal(|ui| {
                ui.label("Token:");
                ui.monospace(&a.token);
                if ui.small_button("Copy").clicked() {
                    ui.ctx().copy_text(a.token.clone());
                }
                if ui.add_enabled(!on, egui::Button::new("New token")).clicked() {
                    regen = true;
                }
            });
            if !a.api_msg.is_empty() {
                ui.label(&a.api_msg);
            }
            ui.separator();
            let port = a.api.as_ref().map(|s| s.port.to_string()).unwrap_or_else(|| a.api_port_text.clone());
            ui.label("Endpoints: GET /status, POST /capture/start|stop, GET /sessions?host=&status=&method=&limit=, GET /sessions/{id}, DELETE /sessions, POST /sessions/export, GET|POST /rules");
            let curl = format!("curl -H \"Authorization: Bearer {}\" http://127.0.0.1:{port}/status", a.token);
            ui.horizontal(|ui| {
                ui.monospace(&curl);
                if ui.small_button("Copy").clicked() {
                    ui.ctx().copy_text(curl.clone());
                }
            });
            ui.separator();
            ui.label("MCP client configuration (stdio server `ferrotap_mcp`, talks to this API):");
            let mcp = format!(
                "{{\"mcpServers\": {{\"ferrotap\": {{\"command\": \"ferrotap_mcp\", \"args\": [\"--api-port\", \"{port}\", \"--token\", \"{}\"]}}}}}}",
                a.token
            );
            ui.horizontal(|ui| {
                ui.monospace(&mcp);
                if ui.small_button("Copy").clicked() {
                    ui.ctx().copy_text(mcp.clone());
                }
            });
        });
        if regen {
            self.ext.auto.token = random_token();
        }
        match want_on {
            Some(true) => {
                let p = self.ext.auto.api_port_text.trim().parse::<u16>();
                match p {
                    Ok(p) => {
                        if let Err(e) = self.automation_start_api(p) {
                            self.ext.auto.api_msg = e;
                        }
                    }
                    Err(_) => self.ext.auto.api_msg = "port must be a number 0-65535".into(),
                }
            }
            Some(false) => self.automation_stop_api(),
            None => {}
        }
        if !open {
            self.ext.auto.show_api = false;
        }
    }

    /// Save the selected sessions as a Composer collection (the "record a browsing session" flow).
    pub fn automation_save_collection(&mut self, name: &str) -> Result<std::path::PathBuf, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("enter a collection name".into());
        }
        let ids: std::collections::HashSet<usize> = self.selected_ids().into_iter().collect();
        let chosen: Vec<SessionRecord> = self.model.rows.iter().filter(|r| ids.contains(&r.id)).cloned().collect();
        if chosen.is_empty() {
            return Err("select one or more sessions first".into());
        }
        let c = if self.ext.auto.coll_as_test { sessions_to_test(name, &chosen) } else { sessions_to_collection(name, &chosen) };
        let p = crate::model::save_collection(&self.comp_dir, &c)?;
        self.comp_collections = crate::model::load_collections(&self.comp_dir);
        Ok(p)
    }

    fn automation_collection_window(&mut self, ctx: &egui::Context) {
        let mut open = true;
        let mut save = false;
        let n = self.selected_ids().len();
        egui::Window::new("Save sessions as collection").open(&mut open).show(ctx, |ui| {
            ui.label(format!("{n} selected session(s) become requests in a Composer collection (replay them from the Composer)."));
            ui.horizontal(|ui| {
                ui.label("Name:");
                ui.text_edit_singleline(&mut self.ext.auto.coll_name);
                if ui.button("Save").clicked() {
                    save = true;
                }
            });
            ui.checkbox(&mut self.ext.auto.coll_as_test, "As a test: check status, content type, JSON keys and time on every replay")
                .on_hover_text("Run it with Composer > Run collection; each request reports pass / fail");
            if !self.ext.auto.coll_msg.is_empty() {
                ui.label(&self.ext.auto.coll_msg);
            }
        });
        if save {
            let name = self.ext.auto.coll_name.clone();
            self.ext.auto.coll_msg = match self.automation_save_collection(&name) {
                Ok(p) => format!("saved {n} request(s) to {}", p.display()),
                Err(e) => e,
            };
        }
        if !open {
            self.ext.auto.show_coll = false;
        }
    }
}
