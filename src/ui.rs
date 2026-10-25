//! Native egui/eframe session grid: toolbar, live session table, request/response inspector.
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use crate::system_proxy::{self, SystemProxyGuard, SystemProxyManager, DEFAULT_PROXY_SERVER};
use std::time::Duration;

use eframe::egui::{self, Color32, Panel, RichText};
use egui_extras::{Column, TableBuilder};

use crate::model::{ColumnState, GridColumn, SessionEvent, SessionMark, SessionModel, SessionRecord, SortCol};
use crate::prefs::UiPrefs;
use crate::g2::{
    AutocompleteIndex, ColumnSource, ConditionOp, CustomColumnDef,
    FilterCondition, FilterField, AdvancedFilterRule, GroupBy,
    SavedFiltersManager, StreamingState, TrafficGroupsManager, TrafficTabsManager,
};

const MARKS: [SessionMark; 7] = [SessionMark::Red, SessionMark::Blue, SessionMark::Green, SessionMark::Orange, SessionMark::Purple, SessionMark::Copper, SessionMark::Strikeout];

#[derive(PartialEq, Clone, Copy, Debug)]
pub enum InspectorTab {
    Headers,
    Raw,
    Text,
    Json,
    Params,
    Cookies,
    Forms,
    Xml,
    Js,
    Trailers,
    Timings,
    Sizes,
    Hex,
    Image,
    Plugins,
    Preview,
}

pub struct GridApp {
    // Group G2 features
    pub custom_columns: Vec<CustomColumnDef>,
    pub new_col_title: String,
    pub new_col_kind: usize,
    pub new_col_arg: String,
    pub saved_filters: SavedFiltersManager,
    pub filter_rule: Option<AdvancedFilterRule>,
    pub show_filter_builder: bool,
    pub filter_builder_field: FilterField,
    pub filter_builder_op: ConditionOp,
    pub filter_builder_val: String,
    pub filter_builder_rows: Vec<FilterCondition>,
    pub filter_builder_any: bool,
    pub filter_builder_name: String,
    pub filter_builder_sel: String,
    pub show_groups_panel: bool,
    pub show_saved_filters: bool,
    pub editing_filter_id: Option<String>,
    pub editing_filter_name: String,
    pub groups_manager: TrafficGroupsManager,
    pub autocomplete: AutocompleteIndex,
    pub tabs_manager: TrafficTabsManager,
    pub streaming_state: StreamingState,
    pub layout_split_horizontal: bool,

    pub(crate) model: SessionModel,
    rx: Receiver<SessionEvent>,
    pub(crate) source_label: String,
    pub(crate) capturing: bool,
    auto_scroll: bool,
    req_tab: InspectorTab,
    resp_tab: InspectorTab,
    last_len: usize,
    proxy_mgr: Arc<SystemProxyManager>,
    proxy_on: bool,
    launch_msg: String,
    _proxy_guard: SystemProxyGuard,
    autotest: Vec<String>,
    frame_no: u32,
    log_path: Option<String>,
    pub ca: crate::ca_trust::CaManager,
    pub show_https: bool,
    pub confirm_trust: bool,
    pub export_path: String,
    pub new_pattern: String,
    pub https_msg: String,
    prefs: UiPrefs,
    prefs_path: std::path::PathBuf,
    last_saved: String,
    mark_filter: Option<SessionMark>,
    show_columns: bool,
    comment_edit: Option<(usize, String)>,
    test_dir: Option<std::path::PathBuf>,
    dump_in: u8,
    pub show_composer: bool,
    ctx_for_hook: Option<egui::Context>,
    pub comp_method: String,
    pub comp_url: String,
    pub comp_hdr_rows: Vec<crate::model::KvRow>,
    pub comp_params: Vec<crate::model::KvRow>,
    pub comp_tab: u8,
    pub comp_collections: Vec<crate::model::Collection>,
    pub comp_history: Vec<crate::model::HistoryEntry>,
    pub comp_dir: std::path::PathBuf,
    pub comp_loaded: bool,
    pub comp_save_coll: String,
    pub comp_save_name: String,
    pub comp_msg: String,
    comp_last: Option<crate::model::SavedRequest>,
    pub comp_body: String,
    pub comp_resp: Option<crate::model::ComposerResponse>,
    pub comp_sending: bool,
    comp_tx: std::sync::mpsc::Sender<crate::model::ComposerResponse>,
    comp_rx: Receiver<crate::model::ComposerResponse>,
    pub rules: Vec<crate::model::Rule>,
    pub show_rules: bool,
    pub show_breakpoints: bool,
    pub bp_rules: Vec<crate::model::BreakpointRule>,
    pub bp_rules_sel: Option<usize>,
    pub bp_msg: String,
    pub bp_edit_url: String,
    pub bp_edit_method: String,
    pub bp_edit_status: String,
    pub bp_edit_headers: String,
    pub bp_edit_body: String,
    pub bp_active_id: Option<u64>,
    rules_sel: Option<usize>,
    pub rules_msg: String,
    rules_hdr_text: String,
    rules_hdr_for: Option<usize>,
    /// State of the feature modules (inspect, data_io, insights, rules_ext)
    pub ext: crate::ext::ExtState,
}

impl GridApp {
    /// The inspector in its own native window (embedded as a floating window where the platform
    /// cannot open more windows). Closing it docks the inspector back.
    pub fn inspector_popout(&mut self, ctx: &egui::Context) {
        let mut dock = false;
        let sel = self.model.selected_row().cloned();
        let (req_tab, resp_tab, st) = (&mut self.req_tab, &mut self.resp_tab, &mut self.ext.inspect);
        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("ferrotap_inspector"),
            egui::ViewportBuilder::default().with_title("Ferrotap - Inspector").with_inner_size([1000.0, 640.0]),
            |ui, _class| {
                egui::CentralPanel::default().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if ui.small_button("Dock").on_hover_text("Put the inspector back under the grid").clicked() {
                            dock = true;
                        }
                        if let Some(s) = &sel {
                            ui.label(RichText::new(format!("#{} {} {}", s.id, s.method, s.url)).small().color(Color32::GRAY));
                        }
                    });
                    match &sel {
                        Some(s) => inspector_panel(ui, req_tab, resp_tab, st, s),
                        None => {
                            ui.label(RichText::new("Select a session in the main window.").italics().color(Color32::GRAY));
                        }
                    }
                });
                if ui.ctx().input(|i| i.viewport().close_requested()) {
                    dock = true;
                }
            },
        );
        if dock {
            self.ext.popout_inspector = false;
        }
    }
}

/// host:port of our proxy for the system proxy, the clean browser and the terminal.
pub fn our_proxy_server() -> String {
    format!("127.0.0.1:{}", crate::conn::active_port())
}

impl GridApp {
    pub fn new(rx: Receiver<SessionEvent>, source_label: impl Into<String>) -> Self {
        let _ = system_proxy::global_manager().recover_stale();
        let autotest: Vec<String> = std::env::var("FERROTAP_AUTOTEST")
            .map(|v| v.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).rev().collect())
            .unwrap_or_default();
        let prefs_path = UiPrefs::default_path();
        let prefs = UiPrefs::load(&prefs_path);
        let mut model = SessionModel::default();
        prefs.to_model(&mut model);
        let last_saved = serde_json::to_string(&prefs).unwrap_or_default();
        let (comp_tx, comp_rx) = std::sync::mpsc::channel();
        // Startup: build the groups manager once, from the restored model, so the panel is ready on frame 1.
        let mut groups_manager = TrafficGroupsManager::new();
        groups_manager.rebuild(&model.rows);
        let custom_columns = prefs.custom_columns.clone();
        groups_manager.pinned_keys = prefs.pinned_groups.iter().cloned().collect();
        // Saved filters, and the filter that was active when the app last closed
        let saved_filters = SavedFiltersManager::load(&SavedFiltersManager::default_path());
        let filter_rule = saved_filters.restored_rule();
        Self {
            prefs,
            prefs_path,
            last_saved,
            mark_filter: None,
            show_columns: false,
            comment_edit: None,
            test_dir: std::env::var("FERROTAP_TEST_DIR").ok().map(std::path::PathBuf::from),
            dump_in: 0,
            autotest,
            frame_no: 0,
            log_path: std::env::var("FERROTAP_SESSION_LOG").ok(),
            model,
            rx,
            source_label: source_label.into(),
            custom_columns,
            new_col_title: String::new(),
            new_col_kind: 0,
            new_col_arg: String::new(),
            saved_filters,
            filter_rule,
            show_filter_builder: false,
            filter_builder_field: FilterField::Host,
            filter_builder_op: ConditionOp::Contains,
            filter_builder_val: String::new(),
            filter_builder_rows: Vec::new(),
            filter_builder_any: false,
            filter_builder_name: String::new(),
            filter_builder_sel: String::new(),
            show_groups_panel: false,
            show_saved_filters: false,
            editing_filter_id: None,
            editing_filter_name: String::new(),
            groups_manager,
            autocomplete: AutocompleteIndex::default(),
            tabs_manager: TrafficTabsManager::default(),
            streaming_state: StreamingState::default(),
            layout_split_horizontal: false,

            capturing: true,
            auto_scroll: true,
            req_tab: InspectorTab::Headers,
            resp_tab: InspectorTab::Headers,
            last_len: 0,
            proxy_mgr: system_proxy::global_manager(),
            proxy_on: system_proxy::global_manager().get_state().map(|s| s.enabled && (s.server == DEFAULT_PROXY_SERVER || s.server == our_proxy_server())).unwrap_or(false),
            launch_msg: String::new(),
            _proxy_guard: SystemProxyGuard::new(system_proxy::global_manager()),
            ca: crate::ca_trust::CaManager::new(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ca")),
            show_https: false,
            confirm_trust: false,
            export_path: r"D:\Projects\Ferrotap\data\export\ferrotap-ca.cer".into(),
            new_pattern: String::new(),
            https_msg: String::new(),
            show_composer: false,
            ctx_for_hook: None,
            comp_method: "GET".into(),
            comp_url: String::new(),
            comp_hdr_rows: Vec::new(),
            comp_params: Vec::new(),
            comp_tab: 0,
            comp_collections: Vec::new(),
            comp_history: Vec::new(),
            comp_dir: crate::model::collections_dir(),
            comp_loaded: false,
            comp_save_coll: "My Collection".into(),
            comp_save_name: String::new(),
            comp_msg: String::new(),
            comp_last: None,
            comp_body: String::new(),
            comp_resp: None,
            comp_sending: false,
            comp_tx,
            comp_rx,
            rules: { let r = crate::model::load_rules(&crate::model::rules_dir()); crate::model::set_global_rules(r.clone()); r },
            show_rules: false,
            show_breakpoints: false,
            bp_rules: crate::model::global_breakpoints(),
            bp_rules_sel: None,
            bp_msg: String::new(),
            bp_edit_url: String::new(),
            bp_edit_method: String::new(),
            bp_edit_status: String::new(),
            bp_edit_headers: String::new(),
            bp_edit_body: String::new(),
            bp_active_id: None,
            rules_sel: None,
            rules_msg: String::new(),
            rules_hdr_text: String::new(),
            rules_hdr_for: None,
            ext: Default::default(),
        }
    }

    /// The selected sessions, or just the focused one when nothing is multi-selected.
    pub fn selected_ids(&self) -> Vec<usize> {
        if !self.ext.sel.is_empty() {
            self.ext.sel.to_vec()
        } else {
            self.model.selected.into_iter().collect()
        }
    }

    /// Toolbar "System Proxy" button handler.
    pub fn proxy_is_on(&self) -> bool {
        self.proxy_on
    }

    pub fn toggle_system_proxy(&mut self) {
        let mgr = self.proxy_mgr.clone();
        let res = if self.proxy_on {
            mgr.restore().and_then(|restored| if restored { Ok(()) } else { mgr.disable() })
        } else {
            mgr.enable(&our_proxy_server(), &crate::conn::current().system_override())
        };
        match res {
            Ok(()) => {
                self.proxy_on = !self.proxy_on;
                self.launch_msg = if self.proxy_on { "System proxy enabled".into() } else { "System proxy restored".into() };
            }
            Err(e) => self.launch_msg = format!("Proxy error: {e}"),
        }
    }

    /// Toolbar "Open Clean Browser" button handler.
    pub fn open_clean_browser(&mut self) {
        // The default browser from the Intercept window (or the first one found), fresh profile.
        self.launch_msg = match self.launch_default_browser() {
            Ok(m) => m,
            Err(e) => e,
        };
    }

    /// Toolbar "Open Terminal" button handler.
    pub fn open_terminal(&mut self) {
        self.launch_msg = match self.launch_default_terminal() {
            Ok(m) => m,
            Err(e) => e,
        };
    }

    /// HTTPS settings window: CA export, explicit-confirmation trust install, do-not-decrypt list editor.
    pub fn https_window(&mut self, ctx: &egui::Context) {
        if !self.show_https { return; }
        let mut open = true;
        let bl = crate::bypass::global();
        // certutil is slow (and trusting waits on a Windows dialog): it runs on a thread, and the
        // CA facts are refreshed only when asked or after a change.
        if let Some(rx) = &self.ext.conn.ca_job {
            match rx.try_recv() {
                Ok(crate::conn::CaJob::Facts(t, i, w)) => {
                    self.ext.conn.ca_facts = Some((t, i, w));
                    self.ext.conn.ca_job = None;
                }
                Ok(crate::conn::CaJob::Done(m)) => {
                    self.https_msg = m;
                    self.ext.conn.ca_facts = None;
                    self.ext.conn.ca_job = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(100)),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.ext.conn.ca_job = None,
            }
        }
        if self.ext.conn.ca_facts.is_none() && self.ext.conn.ca_job.is_none() {
            let ca = self.ca.clone();
            self.ext.conn.ca_job = Some(crate::conn::ca_job(ctx, move || crate::conn::CaJob::Facts(ca.is_trusted(), ca.info(), ca.expiry_warning(30))));
        }
        let ca_busy = self.ext.conn.ca_job.is_some();
        let mut ca_action: Option<&'static str> = None;
        egui::Window::new("HTTPS Settings").open(&mut open).default_width(480.0).show(ctx, |ui| {
            ui.label(RichText::new("Root CA").strong());
            let cs = &mut self.ext.conn;
            let checking = cs.ca_facts.is_none();
            let (trusted, info, warn) = cs.ca_facts.clone().unwrap_or((false, Err(String::new()), None));
            if checking {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Checking the CA...");
                });
            } else { match &info {
                Ok(i) => {
                    ui.label(format!("Subject: {}", i.subject));
                    ui.label(format!("Valid: {} to {} ({} days left)", &i.not_before[..10], &i.not_after[..10], i.days_left.max(0)));
                    ui.label(format!("SHA-1: {}", i.sha1_thumbprint));
                }
                Err(e) => { ui.colored_label(Color32::from_rgb(230, 90, 90), format!("Cannot read the CA: {e}")); }
            } }
            if let Some(w) = &warn { ui.colored_label(Color32::from_rgb(230, 160, 40), w); }
            ui.horizontal(|ui| {
                ui.label(format!("Trusted in CurrentUser store: {}", if trusted { "yes" } else { "no" }));
                if ui.small_button("Refresh").clicked() { ca_action = Some("refresh"); }
            });
            ui.collapsing("Regenerate or use a custom CA", |ui| {
                ui.label(RichText::new("The old CA files are kept in ca\\old-<date>. Restart Ferrotap afterwards, then trust the new CA.").small().color(Color32::GRAY));
                if trusted {
                    ui.label(RichText::new("Remove the current CA from the trusted store first (so no stale trusted CA is left behind).").small().color(Color32::from_rgb(230, 160, 40)));
                }
                ui.add_enabled_ui(!trusted, |ui| {
                    if !cs.confirm_regen {
                        if ui.button("Regenerate the root CA...").clicked() { cs.confirm_regen = true; }
                    } else {
                        ui.horizontal(|ui| {
                            ui.label("Create a new root CA (10 years)? Devices that trust the old one must trust the new one.");
                            if ui.button("Yes, regenerate").clicked() { ca_action = Some("regen"); cs.confirm_regen = false; }
                            if ui.button("Cancel").clicked() { cs.confirm_regen = false; }
                        });
                    }
                    ui.horizontal(|ui| {
                        ui.label("Certificate (PEM)");
                        ui.add(egui::TextEdit::singleline(&mut cs.custom_cert).desired_width(240.0));
                        if ui.small_button("Browse...").clicked() { ca_action = Some("pick_cert"); }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Private key (PEM)");
                        ui.add(egui::TextEdit::singleline(&mut cs.custom_key).desired_width(240.0));
                        if ui.small_button("Browse...").clicked() { ca_action = Some("pick_key"); }
                    });
                    if ui.button("Use this custom CA").clicked() { ca_action = Some("custom"); }
                });
            });
            ui.horizontal(|ui| {
                ui.label("Export to:");
                ui.text_edit_singleline(&mut self.export_path);
            });
            ui.label(RichText::new("Format by extension: .pem/.crt = PEM, .p12/.pfx = PKCS#12, else DER").small().color(Color32::GRAY));
            if ui.button("Export root CA").clicked() {
                let p = self.export_path.trim().to_string();
                let l = p.to_lowercase();
                let r = if l.ends_with(".pem") || l.ends_with(".crt") { self.ca.export_pem(&p) }
                    else if l.ends_with(".p12") || l.ends_with(".pfx") { self.ca.export_pkcs12(&p, None) }
                    else { self.ca.export_der(&p) };
                self.https_msg = match r { Ok(f) => format!("Exported: {}", f.display()), Err(e) => format!("Export failed: {e}") };
            }
            ui.separator();
            if ca_busy && !checking {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Waiting for Windows (answer its certificate dialog if one is open)...");
                });
            } else if !self.confirm_trust {
                if ui.add_enabled(!ca_busy, egui::Button::new("Trust root CA (Windows)...")).clicked() { self.confirm_trust = true; }
            } else {
                ui.colored_label(Color32::from_rgb(230, 160, 40), "Install Ferrotap Local Root CA into the CurrentUser Trusted Root store? Anyone with the CA key could then impersonate sites for this user.");
                ui.horizontal(|ui| {
                    if ui.add_enabled(!ca_busy, egui::Button::new("Yes, trust it")).clicked() {
                        self.confirm_trust = false;
                        ca_action = Some("trust");
                    }
                    if ui.button("Cancel").clicked() { self.confirm_trust = false; self.https_msg = "Trust install cancelled".into(); }
                });
            }
            if !ca_busy && self.ext.conn.ca_facts.as_ref().is_some_and(|f| f.0) && ui.button("Remove the CA from the trusted store").clicked() {
                ca_action = Some("untrust");
            }
            ui.separator();
            ui.label(RichText::new("Do not decrypt these hosts (raw TLS tunnel)").strong());
            ui.label(RichText::new("example.com, *.example.com, api*.corp.io").small().color(Color32::GRAY));
            ui.horizontal(|ui| {
                let r = ui.add(egui::TextEdit::singleline(&mut self.new_pattern).hint_text("host or pattern"));
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (ui.button("Add").clicked() || enter) && !self.new_pattern.trim().is_empty() {
                    self.https_msg = match bl.add(&self.new_pattern) { Ok(()) => format!("Added {}", self.new_pattern.trim()), Err(e) => e };
                    self.new_pattern.clear();
                }
            });
            let mut del: Option<String> = None;
            for p in bl.patterns() {
                ui.horizontal(|ui| {
                    ui.monospace(&p);
                    if ui.small_button("x").clicked() { del = Some(p.clone()); }
                });
            }
            if let Some(d) = del { let _ = bl.remove(&d); }
            ui.label(format!("Tunnelled so far: {}", bl.tunneled.load(std::sync::atomic::Ordering::Relaxed)));
            if !self.https_msg.is_empty() { ui.label(RichText::new(&self.https_msg).color(Color32::GRAY)); }
        });
        let done = |r: Result<Option<std::path::PathBuf>, String>, what: &str| match r {
            Ok(Some(b)) => format!("{what}. Old CA kept in {}. Restart Ferrotap, then trust the new CA.", b.display()),
            Ok(None) => format!("{what}. Restart Ferrotap, then trust the new CA."),
            Err(e) => format!("{what} failed: {e}"),
        };
        match ca_action {
            Some("refresh") => if !ca_busy { self.ext.conn.ca_facts = None },
            Some("trust") => {
                let ca = self.ca.clone();
                self.https_msg = "Trusting the CA...".into();
                self.ext.conn.ca_job = Some(crate::conn::ca_job(ctx, move || crate::conn::CaJob::Done(match ca.install_trust(|_| true) { Ok(()) => "CA trusted".into(), Err(e) => format!("{e}") })));
            }
            Some("untrust") => {
                let ca = self.ca.clone();
                self.ext.conn.ca_job = Some(crate::conn::ca_job(ctx, move || crate::conn::CaJob::Done(match ca.uninstall_trust() { Ok(()) => "CA removed from the trusted store".into(), Err(e) => format!("{e}") })));
            }
            Some("regen") => {
                self.https_msg = done(self.ca.regenerate(3650), "New root CA created");
                self.ext.conn.ca_facts = None;
            }
            Some("custom") => {
                let (c, k) = (self.ext.conn.custom_cert.trim().trim_matches('"').to_string(), self.ext.conn.custom_key.trim().trim_matches('"').to_string());
                self.https_msg = done(self.ca.import_custom(std::path::Path::new(&c), std::path::Path::new(&k)), "Custom CA installed");
                self.ext.conn.ca_facts = None;
            }
            Some("pick_cert") => self.ext.pick.open_file("ca_cert", "Choose the CA certificate (PEM)", &self.ext.conn.custom_cert, &["pem", "crt", "cer"]),
            Some("pick_key") => self.ext.pick.open_file("ca_key", "Choose the CA private key (PEM)", &self.ext.conn.custom_key, &["pem", "key"]),
            _ => {}
        }
        if let Some(p) = self.ext.pick.take("ca_cert") { self.ext.conn.custom_cert = p.display().to_string(); }
        if let Some(p) = self.ext.pick.take("ca_key") { self.ext.conn.custom_key = p.display().to_string(); }
        if !open { self.show_https = false; self.ext.conn.ca_facts = None; }
    }

/// Label for the Keep: N sessions setting.
fn keep_label(n: usize) -> String {
    if n == 0 { "all sessions".to_string() } else { format!("most recent {n}") }
}

    /// Column picker: show/hide each grid column (persisted).
    
    /// Filter builder window: condition rows (field/operator/value), Any/All, apply/clear, save/load/delete named filters.
fn filter_field_label(field: &FilterField) -> &'static str {
    field.name()
}

    pub fn filter_builder_window(&mut self, ctx: &egui::Context) {
        if !self.show_filter_builder { return; }
        if self.filter_builder_rows.is_empty() {
            self.filter_builder_rows.push(FilterCondition::new(FilterField::Host, ConditionOp::Contains, ""));
        }
        let fields = [
            FilterField::Method,
            FilterField::Host,
            FilterField::Path,
            FilterField::Url,
            FilterField::Status,
            FilterField::ContentType,
            FilterField::ReqHeader(String::new()),
            FilterField::ResHeader(String::new()),
            FilterField::Cookie(String::new()),
            FilterField::RequestBody,
            FilterField::ResponseBody,
            FilterField::DurationMs,
            FilterField::ResponseSize,
            FilterField::ProcessName,
            FilterField::Pid,
            FilterField::ClientIp,
            FilterField::RemoteIp,
            FilterField::ConnectMs,
            FilterField::TlsMs,
            FilterField::TtfbMs,
            FilterField::TlsVersion,
            FilterField::QueryParam(String::new()),
            FilterField::Comment,
            FilterField::HttpVersion,
            FilterField::ResponseHttpVersion,
        ];
        let ops = [ConditionOp::Equals, ConditionOp::NotEquals, ConditionOp::Contains, ConditionOp::NotContains, ConditionOp::StartsWith, ConditionOp::EndsWith, ConditionOp::GreaterThan, ConditionOp::LessThan, ConditionOp::RegexMatch, ConditionOp::Exists, ConditionOp::DoesNotExist];
        let mut open = true;
        egui::Window::new("Filter Builder").open(&mut open).resizable(true).default_width(620.0).show(ctx, |ui| {
            ui.heading("Filter Builder");
            ui.horizontal(|ui| {
                ui.label("Match:");
                ui.selectable_value(&mut self.filter_builder_any, false, "All conditions");
                ui.selectable_value(&mut self.filter_builder_any, true, "Any condition");
            });
            ui.separator();
            let mut remove: Option<usize> = None;
            let rows_all = &self.model.rows;
            let sugg = &mut self.ext.fb_suggest;
            for (i, row) in self.filter_builder_rows.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt(("fb_field", i)).width(130.0).selected_text(Self::filter_field_label(&row.field)).show_ui(ui, |ui| {
                        for f in fields.iter() {
                            let is_sel = std::mem::discriminant(&row.field) == std::mem::discriminant(f);
                            if ui.selectable_label(is_sel, Self::filter_field_label(f)).clicked() && !is_sel {
                                row.field = f.clone();
                            }
                        }
                    });
                    let name_box = match &mut row.field {
                        FilterField::ReqHeader(ref mut h) | FilterField::ResHeader(ref mut h) => {
                            Some(ui.add(egui::TextEdit::singleline(h).desired_width(90.0).hint_text("header")))
                        }
                        FilterField::Cookie(ref mut c) => Some(ui.add(egui::TextEdit::singleline(c).desired_width(90.0).hint_text("cookie"))),
                        FilterField::QueryParam(ref mut c) => Some(ui.add(egui::TextEdit::singleline(c).desired_width(90.0).hint_text("parameter"))),
                        _ => None,
                    };
                    if let Some(nr) = name_box {
                        if nr.has_focus() { *sugg = Some((i, true)); }
                        if *sugg == Some((i, true)) {
                            let typed = match &row.field { FilterField::ReqHeader(h) | FilterField::ResHeader(h) | FilterField::Cookie(h) | FilterField::QueryParam(h) => h.clone(), _ => String::new() };
                            let items = crate::filter_suggest::suggest_names(rows_all, &row.field, &typed, 8);
                            let (picked, hovered) = crate::filter_suggest::popup(ui, egui::Id::new(("fb_name_sugg", i)), nr.rect, &items);
                            if let Some(p) = picked {
                                if let FilterField::ReqHeader(h) | FilterField::ResHeader(h) | FilterField::Cookie(h) | FilterField::QueryParam(h) = &mut row.field { *h = p; }
                                *sugg = None;
                            } else if !nr.has_focus() && !hovered {
                                *sugg = None;
                            }
                        }
                    }
                    egui::ComboBox::from_id_salt(("fb_op", i)).width(110.0).selected_text(row.op.label()).show_ui(ui, |ui| {
                        for o in ops.iter() { ui.selectable_value(&mut row.op, o.clone(), o.label()); }
                    });
                    let vr = ui.add(egui::TextEdit::singleline(&mut row.value).desired_width(150.0).hint_text("value"));
                    if vr.has_focus() { *sugg = Some((i, false)); }
                    if *sugg == Some((i, false)) {
                        let items = crate::filter_suggest::suggest(rows_all, &row.field, &row.value, 8);
                        let (picked, hovered) = crate::filter_suggest::popup(ui, egui::Id::new(("fb_value_sugg", i)), vr.rect, &items);
                        if let Some(p) = picked {
                            row.value = p;
                            *sugg = None;
                        } else if !vr.has_focus() && !hovered {
                            *sugg = None;
                        }
                    }
                    if ui.button("-").on_hover_text("Remove this condition").clicked() { remove = Some(i); }
                });
            }
            if let Some(i) = remove { self.filter_builder_rows.remove(i); }
            if ui.button("+ Add condition").clicked() {
                self.filter_builder_rows.push(FilterCondition::new(FilterField::Host, ConditionOp::Contains, ""));
            }
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Apply").clicked() {
                    let rule = self.filter_builder_rule();
                    self.launch_msg = format!("Filter applied: {}", rule.describe());
                    self.filter_rule = Some(rule.clone());
                    self.saved_filters.active_rule = Some(rule);
                    let _ = self.saved_filters.save(&SavedFiltersManager::default_path());
                }
                if ui.button("Clear").clicked() {
                    self.filter_rule = None;
                    self.saved_filters.active_rule = None;
                    self.saved_filters.active_filter_id = None;
                    let _ = self.saved_filters.save(&SavedFiltersManager::default_path());
                    self.launch_msg = "Filter cleared".to_string();
                }
            });
            if let Some(ref rule) = self.filter_rule {
                ui.label(RichText::new(format!("Active: {}", rule.describe())).color(Color32::LIGHT_GREEN));
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("Name:");
                ui.add(egui::TextEdit::singleline(&mut self.filter_builder_name).desired_width(160.0));
                if ui.button("Save").clicked() {
                    let name = self.filter_builder_name.trim().to_string();
                    if name.is_empty() {
                        self.launch_msg = "Enter a name to save the filter".to_string();
                    } else {
                        let mut rule = self.filter_builder_rule();
                        rule.name = name.clone();
                        self.saved_filters.add(&name, rule);
                        let _ = self.saved_filters.save(&SavedFiltersManager::default_path());
                        self.launch_msg = format!("Saved filter '{}'", name);
                    }
                }
            });
            let saved: Vec<(String, String, AdvancedFilterRule)> = self.saved_filters.filters.iter().map(|f| (f.id.clone(), f.name.clone(), f.rule.clone())).collect();
            ui.horizontal(|ui| {
                ui.label("Saved:");
                let sel_text = saved.iter().find(|s| s.0 == self.filter_builder_sel).map(|s| s.1.clone()).unwrap_or_else(|| "(choose)".to_string());
                egui::ComboBox::from_id_salt("fb_saved").width(160.0).selected_text(sel_text).show_ui(ui, |ui| {
                    for s in saved.iter() { ui.selectable_value(&mut self.filter_builder_sel, s.0.clone(), s.1.clone()); }
                });
                if ui.button("Load").clicked() {
                    if let Some(s) = saved.iter().find(|s| s.0 == self.filter_builder_sel) {
                        self.filter_builder_rows = s.2.conditions.clone();
                        self.filter_builder_any = s.2.combinator == crate::g2::Combinator::Any;
                        self.filter_builder_name = s.1.clone();
                        self.launch_msg = format!("Loaded filter '{}'", s.1);
                    }
                }
                if ui.button("Delete").clicked() {
                    let id = self.filter_builder_sel.clone();
                    if self.saved_filters.delete(&id) {
                        self.filter_builder_sel.clear();
                        let _ = self.saved_filters.save(&SavedFiltersManager::default_path());
                        self.launch_msg = "Saved filter deleted".to_string();
                    }
                }
            });
        });
        if !open { self.show_filter_builder = false; }
    }

    /// Build the rule described by the builder rows and the Any/All switch.
    pub fn filter_builder_rule(&self) -> AdvancedFilterRule {
        AdvancedFilterRule {
            name: if self.filter_builder_name.trim().is_empty() { "Custom Filter".to_string() } else { self.filter_builder_name.trim().to_string() },
            combinator: if self.filter_builder_any { crate::g2::Combinator::Any } else { crate::g2::Combinator::All },
            conditions: self.filter_builder_rows.clone(),
            enabled: true,
        }
    }

    /// Saved filters management dialog.
    pub fn saved_filters_window(&mut self, ctx: &egui::Context) {
        if !self.show_saved_filters { return; }
        let mut open = true;
        self.saved_filters_window_inner(ctx, &mut open);
        if !open { self.show_saved_filters = false; }
    }

    fn saved_filters_window_inner(&mut self, ctx: &egui::Context, open: &mut bool) {
        egui::Window::new("Saved Filters").open(open).resizable(true).default_width(380.0).show(ctx, |ui| {
            ui.heading("Preset Filters");
            ui.separator();

            ui.horizontal(|ui| {
                if ui.button("Reset (Disable All)").clicked() {
                    self.saved_filters.reset_filters();
                    self.filter_rule = None;
                    let _ = self.saved_filters.save(&SavedFiltersManager::default_path());
                    self.launch_msg = "Reset active filters".to_string();
                }
                let confirm_id = egui::Id::new("saved_filters_confirm");
                let pending: Option<String> = ui.ctx().data(|d| d.get_temp(confirm_id));
                let revert = if pending.as_deref() == Some("__revert__") {
                    let yes = ui.button(RichText::new("Confirm revert").color(Color32::LIGHT_RED)).on_hover_text("Replaces every saved filter with the defaults").clicked();
                    if ui.button("Cancel").clicked() {
                        ui.ctx().data_mut(|d| d.remove::<String>(confirm_id));
                    }
                    yes
                } else {
                    if ui.button("Revert to Defaults").clicked() {
                        ui.ctx().data_mut(|d| d.insert_temp(confirm_id, "__revert__".to_string()));
                    }
                    false
                };
                if revert {
                    ui.ctx().data_mut(|d| d.remove::<String>(confirm_id));
                    let defaults = SavedFiltersManager::default_presets();
                    self.saved_filters.revert(defaults);
                    self.filter_rule = None;
                    let _ = self.saved_filters.save(&SavedFiltersManager::default_path());
                    self.launch_msg = "Reverted to default filters".to_string();
                }
            });
            ui.separator();

            let filters: Vec<(String, String, bool, AdvancedFilterRule)> = self.saved_filters.filters.iter()
                .map(|f| (f.id.clone(), f.name.clone(), f.enabled, f.rule.clone()))
                .collect();

            if filters.is_empty() {
                ui.label(RichText::new("No saved filter presets").italics().color(Color32::GRAY));
            } else {
                let mut remove_id = None;
                let mut duplicate_id = None;
                let mut toggle_id = None;
                let mut save_rename: Option<(String, String)> = None;

                egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                    for (id, name, enabled, rule) in filters {
                        ui.group(|ui| {
                            ui.horizontal(|ui| {
                                let mut is_en = enabled;
                                if ui.checkbox(&mut is_en, "").clicked() {
                                    toggle_id = Some(id.clone());
                                }

                                if self.editing_filter_id.as_deref() == Some(&id) {
                                    ui.add(egui::TextEdit::singleline(&mut self.editing_filter_name).desired_width(120.0));
                                    if ui.button("OK").clicked() {
                                        save_rename = Some((id.clone(), self.editing_filter_name.clone()));
                                    }
                                    if ui.button("Cancel").clicked() {
                                        self.editing_filter_id = None;
                                    }
                                } else {
                                    let label_text = if enabled {
                                        RichText::new(&name).strong().color(Color32::from_rgb(100, 220, 100))
                                    } else {
                                        RichText::new(&name).color(Color32::GRAY)
                                    };
                                    ui.label(label_text);
                                    if ui.small_button("Rename").clicked() {
                                        self.editing_filter_id = Some(id.clone());
                                        self.editing_filter_name = name.clone();
                                    }
                                }

                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    let confirm_id = egui::Id::new("saved_filters_confirm");
                                    let pending: Option<String> = ui.ctx().data(|d| d.get_temp(confirm_id));
                                    if pending.as_deref() == Some(id.as_str()) {
                                        if ui.button("Cancel").clicked() {
                                            ui.ctx().data_mut(|d| d.remove::<String>(confirm_id));
                                        }
                                        if ui.button(RichText::new("Confirm delete").color(Color32::LIGHT_RED)).clicked() {
                                            remove_id = Some(id.clone());
                                            ui.ctx().data_mut(|d| d.remove::<String>(confirm_id));
                                        }
                                    } else if ui.button("x").on_hover_text("Delete this filter").clicked() {
                                        ui.ctx().data_mut(|d| d.insert_temp(confirm_id, id.clone()));
                                    }
                                    if ui.button("Duplicate").clicked() {
                                        duplicate_id = Some(id.clone());
                                    }
                                    if ui.button("Apply").clicked() {
                                        self.filter_rule = Some(rule.clone());
                                        self.saved_filters.active_filter_id = Some(id.clone());
                                        self.saved_filters.active_rule = Some(rule.clone());
                                        let _ = self.saved_filters.save(&SavedFiltersManager::default_path());
                                        self.launch_msg = format!("Applied filter: {}", name);
                                    }
                                });
                            });
                        });
                    }
                });

                if let Some((id, new_name)) = save_rename {
                    self.saved_filters.rename(&id, &new_name);
                    let _ = self.saved_filters.save(&SavedFiltersManager::default_path());
                    self.editing_filter_id = None;
                }
                if let Some(id) = toggle_id {
                    self.saved_filters.toggle_enabled(&id);
                    let _ = self.saved_filters.save(&SavedFiltersManager::default_path());
                }
                if let Some(id) = duplicate_id {
                    self.saved_filters.duplicate(&id);
                    let _ = self.saved_filters.save(&SavedFiltersManager::default_path());
                }
                if let Some(id) = remove_id {
                    self.saved_filters.remove(&id);
                    let _ = self.saved_filters.save(&SavedFiltersManager::default_path());
                }
            }
        });
    }

    /// Traffic groups hierarchy panel.
    /// Traffic groups as a docked side panel: pick a group to show only its sessions, pin groups to the top,
    /// remove a group's sessions. Groups are rebuilt only when the sessions change.
    pub fn groups_side_panel(&mut self, ui: &mut egui::Ui) {
        if !self.show_groups_panel { return; }
        self.groups_manager.refresh(&self.model.rows);
        let mut remove_key: Option<String> = None;
        let mut pin_key: Option<String> = None;
        Panel::left("traffic_groups").resizable(true).default_size(230.0).min_size(160.0).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.strong("Traffic groups");
                if ui.small_button("\u{2715}").on_hover_text("Hide the groups panel").clicked() {
                    self.show_groups_panel = false;
                }
            });
            ui.horizontal(|ui| {
                ui.label("Group by");
                let cur = self.groups_manager.group_by.unwrap_or(GroupBy::Host);
                egui::ComboBox::from_id_salt("tg_groupby").selected_text(cur.label()).show_ui(ui, |ui| {
                    for gb in GroupBy::ALL {
                        if ui.selectable_label(cur == gb, gb.label()).clicked() {
                            self.groups_manager.set_group_by(gb);
                            self.groups_manager.selected_group = None;
                        }
                    }
                });
            });
            if self.groups_manager.selected_group.is_some() && ui.small_button("Show all sessions").clicked() {
                self.groups_manager.selected_group = None;
            }
            ui.separator();
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                for g in &self.groups_manager.groups {
                    let is_sel = self.groups_manager.selected_group.as_deref() == Some(g.key.as_str());
                    ui.horizontal(|ui| {
                        let pin = if g.pinned { "\u{1f4cc}" } else { "\u{00b7}" };
                        if ui.small_button(pin).on_hover_text(if g.pinned { "Unpin" } else { "Pin to the top" }).clicked() {
                            pin_key = Some(g.key.clone());
                        }
                        let r = ui.selectable_label(is_sel, format!("{} ({})", g.key, g.session_ids.len()));
                        if r.clicked() {
                            self.groups_manager.selected_group = if is_sel { None } else { Some(g.key.clone()) };
                        }
                        r.context_menu(|ui| {
                            if ui.button(if g.pinned { "Unpin group" } else { "Pin group" }).clicked() {
                                pin_key = Some(g.key.clone());
                                ui.close();
                            }
                            if ui.button(format!("Remove these {} sessions", g.session_ids.len())).clicked() {
                                remove_key = Some(g.key.clone());
                                ui.close();
                            }
                        });
                    });
                }
            });
        });
        if let Some(k) = pin_key {
            self.groups_manager.toggle_pin(&k);
        }
        if let Some(k) = remove_key {
            let ids = self.groups_manager.ids_of(&k);
            self.model.remove_ids(&ids);
            if self.groups_manager.selected_group.as_deref() == Some(k.as_str()) {
                self.groups_manager.selected_group = None;
            }
        }
    }

    pub fn columns_window(&mut self, ctx: &egui::Context) {
        if !self.show_columns { return; }
        let mut open = true;
        egui::Window::new("Columns").open(&mut open).show(ctx, |ui| {
            // in display order, with up/down to reorder
            let order: Vec<GridColumn> = self.model.column_state.columns.iter().map(|c| c.col).collect();
            let mut mv: Option<(GridColumn, i32)> = None;
            for (i, c) in order.iter().copied().enumerate() {
                ui.horizontal(|ui| {
                    if ui.add_enabled(i > 0, egui::Button::new("\u{25b2}").small()).on_hover_text("Move up").clicked() {
                        mv = Some((c, -1));
                    }
                    if ui.add_enabled(i + 1 < order.len(), egui::Button::new("\u{25bc}").small())
                        .on_hover_text("Move down").clicked() {
                        mv = Some((c, 1));
                    }
                    let mut v = self.model.column_state.is_visible(c);
                    if ui.checkbox(&mut v, c.title()).changed() {
                        self.model.column_state.set_visible(c, v);
                    }
                });
            }
            if let Some((c, d)) = mv {
                self.model.column_state.move_col(c, d);
            }
            if ui.button("Reset to defaults").clicked() {
                self.model.column_state = ColumnState::default();
            }
            ui.separator();
            ui.label(RichText::new("Custom columns").strong());
            let mut remove: Option<usize> = None;
            for (i, cc) in self.custom_columns.iter().enumerate() {
                ui.horizontal(|ui| {
                    if ui.small_button("\u{2715}").on_hover_text("Remove this column").clicked() {
                        remove = Some(i);
                    }
                    ui.label(format!("{}  ({})", cc.title, custom_source_label(&cc.source)));
                });
            }
            if let Some(i) = remove {
                self.custom_columns.remove(i);
            }
            ui.label("Add column\u{2026}");
            ui.horizontal(|ui| {
                ui.label("Title");
                ui.text_edit_singleline(&mut self.new_col_title);
            });
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("new_col_kind")
                    .selected_text(CUSTOM_SOURCE_KINDS[self.new_col_kind].0)
                    .show_ui(ui, |ui| {
                        for (k, (label, _)) in CUSTOM_SOURCE_KINDS.iter().enumerate() {
                            ui.selectable_value(&mut self.new_col_kind, k, *label);
                        }
                    });
                ui.add(egui::TextEdit::singleline(&mut self.new_col_arg).desired_width(140.0)
                    .hint_text(CUSTOM_SOURCE_KINDS[self.new_col_kind].1));
            });
            let ready = !self.new_col_arg.trim().is_empty();
            if ui.add_enabled(ready, egui::Button::new("Add column")).clicked() {
                let arg = self.new_col_arg.trim().to_string();
                let source = custom_source(self.new_col_kind, &arg);
                let title = if self.new_col_title.trim().is_empty() { arg.clone() } else { self.new_col_title.trim().to_string() };
                let id = format!("cc-{}-{}", self.custom_columns.len(), arg);
                self.custom_columns.push(CustomColumnDef::new(&id, &title, source));
                self.new_col_title.clear();
                self.new_col_arg.clear();
            }
        });
        if !open { self.show_columns = false; }
    }

    /// Comment editor opened from the row context menu.
    pub fn comment_window(&mut self, ctx: &egui::Context) {
        let Some((id, mut text)) = self.comment_edit.take() else { return };
        let mut save = false;
        let mut cancel = false;
        egui::Window::new(format!("Comment on session #{id}")).collapsible(false).show(ctx, |ui| {
            ui.add(egui::TextEdit::multiline(&mut text).desired_rows(3).desired_width(320.0));
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() { save = true; }
                if ui.button("Cancel").clicked() { cancel = true; }
            });
        });
        if save {
            self.model.set_comment(id, text);
            self.sync_annotation(id);
        } else if !cancel {
            self.comment_edit = Some((id, text));
        }
    }

    fn sync_annotation(&mut self, id: usize) {
        if let Some(r) = self.model.rows.iter().find(|r| r.id == id) {
            self.prefs.sync_row(r);
        }
    }

    /// Run the native window (blocks until closed).
    pub fn run(self, title: &str) -> eframe::Result<()> {
        let options = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([1280.0, 780.0])
                .with_title(title),
            ..Default::default()
        };
        eframe::run_native(title, options, Box::new(move |_cc| Ok(Box::new(self))))
    }
}

fn status_color(s: u16) -> Color32 {
    match s {
        200..=299 => Color32::from_rgb(70, 170, 90),
        300..=399 => Color32::from_rgb(90, 150, 220),
        400..=499 => Color32::from_rgb(230, 160, 40),
        500..=599 => Color32::from_rgb(220, 70, 70),
        _ => Color32::GRAY,
    }
}

fn fmt_size(n: usize) -> String {
    if n >= 1_048_576 {
        format!("{:.1} MB", n as f64 / 1_048_576.0)
    } else if n >= 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{n} B")
    }
}

fn format_json(raw: &str) -> String {
    crate::inspect::pretty_json(raw)
}

fn raw_request(s: &SessionRecord) -> String {
    let path = if s.path().is_empty() { "/" } else { s.path() };
    let mut out = format!("{} {} {}\n", s.method, path, s.http_version);
    for (k, v) in &s.request_headers {
        out.push_str(&format!("{k}: {v}\n"));
    }
    out.push('\n');
    out.push_str(&s.request_body);
    out
}

fn raw_response(s: &SessionRecord) -> String {
    let proto = if s.response_http_version.is_empty() {
        if s.http_version.is_empty() { "HTTP/1.1" } else { &s.http_version }
    } else {
        &s.response_http_version
    };
    let mut out = format!("{} {}\n", proto, s.status);
    for (k, v) in &s.response_headers {
        out.push_str(&format!("{k}: {v}\n"));
    }
    out.push('\n');
    out.push_str(&s.response_body);
    out
}

/// Headers the HTTP stack / proxy adds on its own (hidden when "auto-generated" is off).
pub fn is_auto_generated_header(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    matches!(
        n.as_str(),
        "host" | "content-length" | "connection" | "proxy-connection" | "keep-alive" | "transfer-encoding" | "te" | "trailer" | "upgrade" | "accept-encoding" | "date"
    ) || n.starts_with("x-ferrotap")
}

fn headers_table(ui: &mut egui::Ui, title: &str, h: &std::collections::BTreeMap<String, String>, session_id: usize) {
    let show_id = egui::Id::new("ferrotap_show_auto_headers");
    let mut show_auto = ui.ctx().data(|d| d.get_temp::<bool>(show_id)).unwrap_or(true);
    let all_text: String = h.iter().flat_map(|(k, v)| v.split('\n').map(move |x| format!("{k}: {x}\n"))).collect();
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{title} ({})", h.len())).strong());
        if !h.is_empty() && ui.small_button("Copy all").on_hover_text("Copy every header as 'Name: value' lines").clicked() {
            ui.ctx().copy_text(all_text.clone());
        }
        if !h.is_empty() && ui.small_button("Save...").on_hover_text("Save the headers to a text file in Downloads").clicked() {
            let side = if title.starts_with("Request") { "request" } else { "response" };
            let path = crate::inspect::downloads_dir().join(format!("session{session_id}_{side}_headers.txt"));
            let msg = match crate::inspect::save_text(&path, &all_text) {
                Ok(_) => format!("Saved {}", path.display()),
                Err(e) => format!("Save failed: {e}"),
            };
            ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new(("hdr_save_msg", title)), msg));
        }
        if ui.checkbox(&mut show_auto, "show auto-generated").on_hover_text("Host, Content-Length, Connection, Accept-Encoding, Transfer-Encoding, ... added by the HTTP stack").changed() {
            ui.ctx().data_mut(|d| d.insert_temp(show_id, show_auto));
        }
        ui.label(RichText::new("(right-click a row to copy name / value / both)").small().color(Color32::GRAY));
    });
    if let Some(m) = ui.ctx().data(|d| d.get_temp::<String>(egui::Id::new(("hdr_save_msg", title)))) {
        ui.label(RichText::new(m).small().color(Color32::GRAY));
    }
    if h.is_empty() {
        ui.label(RichText::new("(No headers)").italics().color(Color32::GRAY));
        return;
    }
    let rows: Vec<(String, String)> = h.iter().filter(|(k, _)| show_auto || !is_auto_generated_header(k)).map(|(k, v)| (k.clone(), v.clone())).collect();
    egui::ScrollArea::both()
        .id_salt(format!("scroll_hdr_{title}"))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            crate::inspect::kv_grid(ui, &format!("grid_hdr_{title}"), ("Header Name", "Value"), &rows);
        });
}

/// Body views (Text / JSON / XML-HTML / JavaScript) share one toolbar: wrap, search, save.
fn text_body_view(ui: &mut egui::Ui, b: &mut crate::inspect::BodyUi, wrap: &mut bool, id_salt: &str, body: &str, save: (usize, String)) {
    crate::inspect::body_view(ui, b, wrap, id_salt, body, save);
}

type BytesSlot = Option<((usize, usize, usize), Arc<Vec<u8>>)>;

/// Body bytes decoded once per session (and again only if the body changes).
fn bytes_cached(slot: &mut BytesSlot, id: usize, text: &str, b64: &Option<String>, get: impl FnOnce() -> Vec<u8>) -> Arc<Vec<u8>> {
    let key = (id, text.len(), b64.as_ref().map_or(0, |b| b.len()));
    match slot {
        Some((k, b)) if *k == key => b.clone(),
        _ => {
            let b = Arc::new(get());
            *slot = Some((key, b.clone()));
            b
        }
    }
}

fn resp_bytes_cached(st: &mut crate::inspect::State, s: &SessionRecord) -> Arc<Vec<u8>> {
    bytes_cached(&mut st.resp_bytes, s.id, &s.response_body, &s.response_body_b64, || s.response_bytes().into_owned())
}

fn req_bytes_cached(st: &mut crate::inspect::State, s: &SessionRecord) -> Arc<Vec<u8>> {
    bytes_cached(&mut st.req_bytes, s.id, &s.request_body, &s.request_body_b64, || s.request_bytes().into_owned())
}

/// `format_body_for_tab`, remembered for the last session / tab / body (one slot per side).
fn format_cached(slot: &mut Option<(crate::inspect::FormatKey, Arc<String>)>, id: usize, tab: InspectorTab, body: &str, content_type: &str) -> Arc<String> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    body.as_bytes()[..body.len().min(4096)].hash(&mut h);
    body.as_bytes()[body.len().saturating_sub(4096)..].hash(&mut h);
    let key = (id, tab as u8, body.len(), h.finish());
    match slot {
        Some((k, f)) if *k == key => f.clone(),
        _ => {
            let f = Arc::new(format_body_for_tab(tab, body, content_type).unwrap_or_default());
            *slot = Some((key, f.clone()));
            f
        }
    }
}

/// Body text after the tab's formatter (`None` for non-body tabs).
fn format_body_for_tab(tab: InspectorTab, body: &str, content_type: &str) -> Option<String> {
    match tab {
        InspectorTab::Text => Some(body.to_string()),
        InspectorTab::Json => Some(format_json(body)),
        InspectorTab::Xml => Some(crate::inspect::pretty_markup(body, content_type.contains("html") || body.trim_start().to_ascii_lowercase().starts_with("<!doctype html"))),
        InspectorTab::Js => Some(crate::inspect::pretty_js(body)),
        _ => None,
    }
}

fn request_inspector(ui: &mut egui::Ui, tab: &mut InspectorTab, st: &mut crate::inspect::State, s: &SessionRecord) {
    ui.group(|ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new("REQUEST").strong().color(Color32::from_rgb(100, 180, 255)));
            ui.label(RichText::new(&s.method).strong());
            ui.label(RichText::new(s.path()).monospace());
            ui.label(RichText::new(&s.http_version).color(Color32::GRAY));
        });
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(tab, InspectorTab::Headers, format!("Headers ({})", s.request_headers.len()));
            ui.selectable_value(tab, InspectorTab::Params, format!("Params ({})", crate::inspect::parse_query(&s.url).len()));
            ui.selectable_value(tab, InspectorTab::Cookies, "Cookies");
            ui.selectable_value(tab, InspectorTab::Forms, "Web Forms");
            ui.selectable_value(tab, InspectorTab::Raw, "Raw");
            ui.selectable_value(tab, InspectorTab::Text, format!("Text ({})", fmt_size(s.request_body.len())));
            ui.selectable_value(tab, InspectorTab::Json, "JSON");
            ui.selectable_value(tab, InspectorTab::Xml, "XML/HTML");
            ui.selectable_value(tab, InspectorTab::Js, "JavaScript");
            ui.selectable_value(tab, InspectorTab::Hex, "Hex");
            if crate::imageview::is_image(&req_bytes_cached(st, s), s.request_header("content-type").unwrap_or("")) {
                ui.selectable_value(tab, InspectorTab::Image, "Image");
            }
        });
        ui.separator();
        let ct = s.request_header("content-type").unwrap_or("").to_ascii_lowercase();
        match *tab {
            InspectorTab::Headers => headers_table(ui, "Request Headers", &s.request_headers, s.id),
            InspectorTab::Params => crate::inspect::params_view(ui, s),
            InspectorTab::Cookies => crate::inspect::cookies_view(ui, s, false),
            InspectorTab::Forms => crate::inspect::forms_view(ui, s),
            InspectorTab::Raw => {
                let raw = raw_request(s);
                let name = format!("session{}_request.txt", s.id);
                text_body_view(ui, &mut st.req, &mut st.wrap, "req_raw", &raw, (s.id, name));
            }
            InspectorTab::Text | InspectorTab::Json | InspectorTab::Xml | InspectorTab::Js => {
                if s.request_body.is_empty() {
                    ui.label(RichText::new("(Empty request body)").italics().color(Color32::GRAY));
                } else {
                    let raw = req_bytes_cached(st, s);
                    let body = crate::inspect::encoding_bar_raw(ui, &mut st.req, s.request_header("content-encoding"), Some(ct.as_str()), &s.request_body, &raw);
                    let shown = format_cached(&mut st.formatted_req, s.id, *tab, &body, &ct);
                    let name = crate::inspect::suggested_file_name(&s.url, &ct, s.id, "request");
                    text_body_view(ui, &mut st.req, &mut st.wrap, &format!("req_body_{:?}", tab), &shown, (s.id, name));
                }
            }
            InspectorTab::Hex => {
                let dump = crate::inspect::hex_dump(&req_bytes_cached(st, s), crate::inspect::HEX_MAX);
                text_body_view(ui, &mut st.req, &mut st.wrap, "req_hex", &dump, (s.id, format!("session{}_request.hex.txt", s.id)));
            }
            InspectorTab::Image => {
                let raw = req_bytes_cached(st, s);
                let decoded = s.request_header("content-encoding").and_then(|ce| crate::inspect::decode_bytes(&raw, ce).ok()).map(|x| x.1);
                crate::imageview::image_view(ui, &mut st.req_image, s.id, "req", decoded.as_deref().unwrap_or(&raw), &ct);
            }
            _ => *tab = InspectorTab::Headers,
        }
    });
}

fn response_inspector(ui: &mut egui::Ui, tab: &mut InspectorTab, st: &mut crate::inspect::State, s: &SessionRecord) {
    ui.group(|ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new("RESPONSE").strong().color(Color32::from_rgb(255, 170, 80)));
            ui.label(RichText::new(format!("{} ", s.status)).strong().color(status_color(s.status)));
            let ct = s.content_type();
            if !ct.is_empty() {
                ui.label(RichText::new(ct).monospace());
            }
            ui.label(RichText::new(format!("{} ms", s.duration_ms)).color(Color32::GRAY));
            ui.label(RichText::new(fmt_size(s.body_size())).color(Color32::GRAY));
        });
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(tab, InspectorTab::Headers, format!("Headers ({})", s.response_headers.len()));
            ui.selectable_value(tab, InspectorTab::Cookies, "Cookies");
            ui.selectable_value(tab, InspectorTab::Raw, "Raw");
            ui.selectable_value(tab, InspectorTab::Text, format!("Text ({})", fmt_size(s.response_body.len())));
            ui.selectable_value(tab, InspectorTab::Json, "JSON");
            ui.selectable_value(tab, InspectorTab::Xml, "XML/HTML");
            ui.selectable_value(tab, InspectorTab::Js, "JavaScript");
            ui.selectable_value(tab, InspectorTab::Hex, "Hex");
            if crate::imageview::is_image(&resp_bytes_cached(st, s), s.content_type()) {
                ui.selectable_value(tab, InspectorTab::Image, "Image");
            }
            if crate::scripting::has_hook(|h| h.inspect) {
                ui.selectable_value(tab, InspectorTab::Plugins, "Plugins");
            }
            if s.content_type().to_ascii_lowercase().contains("html") {
                ui.selectable_value(tab, InspectorTab::Preview, "Preview");
            }
            ui.selectable_value(tab, InspectorTab::Trailers, format!("Trailers ({})", s.response_trailers.len()));
            ui.selectable_value(tab, InspectorTab::Timings, "Timings");
            ui.selectable_value(tab, InspectorTab::Sizes, "Sizes");
        });
        ui.separator();
        let ct = s.content_type().to_ascii_lowercase();
        match *tab {
            InspectorTab::Headers => headers_table(ui, "Response Headers", &s.response_headers, s.id),
            InspectorTab::Cookies => crate::inspect::cookies_view(ui, s, true),
            InspectorTab::Raw => {
                let raw = raw_response(s);
                let name = format!("session{}_response.txt", s.id);
                text_body_view(ui, &mut st.resp, &mut st.wrap, "resp_raw", &raw, (s.id, name));
            }
            InspectorTab::Text | InspectorTab::Json | InspectorTab::Xml | InspectorTab::Js => {
                if s.response_body.is_empty() {
                    ui.label(RichText::new("(Empty response body)").italics().color(Color32::GRAY));
                } else {
                    let raw = resp_bytes_cached(st, s);
                    let body = crate::inspect::encoding_bar_raw(ui, &mut st.resp, s.response_header("content-encoding"), Some(ct.as_str()), &s.response_body, &raw);
                    let shown = format_cached(&mut st.formatted, s.id, *tab, &body, &ct);
                    let name = crate::inspect::suggested_file_name(&s.url, &ct, s.id, "response");
                    text_body_view(ui, &mut st.resp, &mut st.wrap, &format!("resp_body_{:?}", tab), &shown, (s.id, name));
                }
            }
            InspectorTab::Hex => {
                let dump = crate::inspect::hex_dump(&resp_bytes_cached(st, s), crate::inspect::HEX_MAX);
                text_body_view(ui, &mut st.resp, &mut st.wrap, "resp_hex", &dump, (s.id, format!("session{}_response.hex.txt", s.id)));
            }
            InspectorTab::Preview => {
                let raw = resp_bytes_cached(st, s);
                let decoded = s.response_header("content-encoding").and_then(|ce| crate::inspect::decode_bytes(&raw, ce).ok()).map(|x| x.1);
                let html = String::from_utf8_lossy(decoded.as_deref().unwrap_or(&raw)).into_owned();
                crate::htmlpreview::preview_view(ui, s, &html);
            }
            InspectorTab::Plugins => {
                for (name, text) in crate::scripting::inspect_all(s) {
                    ui.label(RichText::new(name).strong());
                    ui.label(RichText::new(text).monospace());
                    ui.separator();
                }
            }
            InspectorTab::Image => {
                // Undo Content-Encoding first (an image is rarely compressed, but it can be).
                let raw = resp_bytes_cached(st, s);
                let decoded = s.response_header("content-encoding").and_then(|ce| crate::inspect::decode_bytes(&raw, ce).ok()).map(|x| x.1);
                crate::imageview::image_view(ui, &mut st.image, s.id, "resp", decoded.as_deref().unwrap_or(&raw), s.content_type());
            }
            InspectorTab::Trailers => crate::inspect::trailers_view(ui, s),
            InspectorTab::Timings => crate::inspect::timings_view(ui, s),
            InspectorTab::Sizes => crate::inspect::sizes_view(ui, s),
            _ => *tab = InspectorTab::Headers,
        }
    });
}

pub fn inspector_panel(ui: &mut egui::Ui, req_tab: &mut InspectorTab, resp_tab: &mut InspectorTab, st: &mut crate::inspect::State, s: &SessionRecord) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("#{} {} {}", s.id, s.method, s.url)).strong().size(15.0));
        ui.label(RichText::new(format!("[{}]", s.status)).strong().color(status_color(s.status)));
        ui.label(RichText::new(format!("Time: {}", s.time_of_day())).color(Color32::GRAY));
        ui.label(RichText::new(format!("Duration: {} ms", s.duration_ms)).color(Color32::GRAY));
    });
    ui.separator();
    ui.columns(2, |cols| {
        request_inspector(&mut cols[0], req_tab, st, s);
        response_inspector(&mut cols[1], resp_tab, st, s);
    });
}

impl eframe::App for GridApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        while let Ok(mut ev) = self.rx.try_recv() {
            if let SessionEvent::New(s) = &mut ev {
                self.prefs.apply_to(s);
            }
            if let (SessionEvent::New(s), Some(lp)) = (&ev, &self.log_path) {
                use std::io::Write;
                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(lp) {
                    let _ = writeln!(f, "{} {} {} {}", s.id, s.method, s.url, s.status);
                }
            }
            if self.capturing {
                self.model.apply(ev);
            }
        }
        if self.prefs.keep_sessions > 0 {
            self.model.trim_to(self.prefs.keep_sessions);
        }
        ui.ctx().request_repaint_after(Duration::from_millis(100));
        // Test hook (env FERROTAP_AUTOTEST=proxy_on,browser,terminal,proxy_off): runs the same handlers as the buttons.
        self.frame_no += 1;
        if self.frame_no.is_multiple_of(5) {
            if let Some(step) = self.autotest.pop() {
                match step.as_str() {
                    "proxy_on" if !self.proxy_on => self.toggle_system_proxy(),
                    "proxy_off" if self.proxy_on => self.toggle_system_proxy(),
                    "browser" => self.open_clean_browser(),
                    "terminal" => self.open_terminal(),
                    _ => {}
                }
            }
        }

        if let Some(k) = self.ext.pending_clear.take() {
            let ids: Vec<usize> = match k {
                "selected" => self.selected_ids(),
                "visible" => self.ext.last_visible.clone(),
                _ => {
                    let shown: std::collections::BTreeSet<usize> = self.ext.last_visible.iter().copied().collect();
                    self.model.rows.iter().map(|r| r.id).filter(|id| !shown.contains(id)).collect()
                }
            };
            let n = self.model.remove_ids(&ids);
            self.launch_msg = format!("Cleared {n} session(s)");
        }
        self.ext.sel.retain_existing(&self.model.rows);
        self.ctx_for_hook = Some(ui.ctx().clone());
        self.test_hook();
        Panel::top("toolbar").show(ui, |ui| {
            ui.horizontal(|ui| {
                let shows = |id: &str| self.ext.qol.settings.toolbar_shows(id);
                let (s_capture, s_clear, s_auto, s_sys, s_browser, s_https, s_comp, s_rules, s_bp, s_term) = (
                    shows("capture"), shows("clear"), shows("autoscroll"), shows("sysproxy"), shows("browser"),
                    shows("https"), shows("composer"), shows("rules"), shows("breakpoints"), shows("terminal"),
                );
                let label = if self.capturing { "\u{25cf} Capturing" } else { "\u{25cb} Paused" };
                if s_capture && ui.selectable_label(self.capturing, label).clicked() {
                    self.capturing = !self.capturing;
                }
                if s_clear { ui.menu_button("Clear \u{25be}", |ui| {
                    for (key, label) in [("selected", "Selected sessions"), ("visible", "Visible sessions"),
                                         ("hidden", "Filtered-out (hidden) sessions")] {
                        if ui.button(label).clicked() {
                            self.ext.pending_clear = Some(key);
                            ui.close();
                        }
                    }
                    ui.separator();
                    for k in crate::g1::ClearKind::ALL_KINDS {
                        if ui.button(k.label()).clicked() {
                            if k == crate::g1::ClearKind::All {
                                self.model.apply(SessionEvent::Clear);
                            } else {
                                let n = self.model.clear_kind(k);
                                self.launch_msg = format!("Cleared {} session(s): {}", n, k.label());
                            }
                            ui.close();
                        }
                    }
                    ui.separator();
                    ui.menu_button(format!("Keep: {}", Self::keep_label(self.prefs.keep_sessions)), |ui| {
                        ui.label("Older sessions are removed as new ones arrive (marked ones stay)");
                        for n in [0usize, 100, 500, 1000, 10_000] {
                            if ui.selectable_label(self.prefs.keep_sessions == n, Self::keep_label(n)).clicked() {
                                self.prefs.keep_sessions = n;
                                ui.close();
                            }
                        }
                    });
                }); }
                if s_auto { ui.checkbox(&mut self.auto_scroll, "Auto-scroll"); }
                ui.separator();
                let proxy_label = if self.proxy_on { format!("System Proxy: ON ({})", crate::conn::active_port()) } else { "System Proxy: OFF".to_string() };
                if s_sys && ui.selectable_label(self.proxy_on, proxy_label).clicked() {
                    self.toggle_system_proxy();
                }
                if s_browser && ui.button("Open Clean Browser").clicked() {
                    self.open_clean_browser();
                }
                if s_https && ui.button("HTTPS Settings").clicked() { self.show_https = !self.show_https; }
                if s_comp && ui.selectable_label(self.show_composer, "Composer").clicked() { self.show_composer = !self.show_composer; }
                if s_rules && ui.selectable_label(self.show_rules, "Rules").clicked() { self.show_rules = !self.show_rules; }
                let paused_cnt = crate::model::breakpoint_hub().count();
                let bp_btn_label = if paused_cnt > 0 { format!("Breakpoints ({})", paused_cnt) } else { "Breakpoints".to_string() };
                if (s_bp || paused_cnt > 0) && ui.selectable_label(self.show_breakpoints, bp_btn_label).clicked() { self.show_breakpoints = !self.show_breakpoints; }
                if s_term && ui.button("Open Terminal").clicked() {
                    self.open_terminal();
                }
                if !self.launch_msg.is_empty() {
                    ui.label(RichText::new(&self.launch_msg).color(Color32::GRAY));
                }
                ui.separator();
                ui.label("Filter:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.model.filter)
                        .hint_text("text, method:POST, status:5xx, host:x, -host:ads, type:json")
                        .desired_width(380.0),
                )
                .on_hover_text(
                    "Space-separated terms, all required. Keys: method: status: url: host: path: scheme: ip: type: header: body: comment: mark: size: dur:\n\
                     status: takes a code, class (5xx), range (400-499) or list (401,403). size:/dur: take >N <N >=N <=N or N (bytes / ms).\n\
                     Prefix a term with - to exclude (e.g. -host:ads.com). Bare words match method/url/status/type/comment.\n\
                     Or start with ~ for the expression language (as in breakpoints): ~m GET & ~c 2xx, ~d \"api\\.\" | ~t json, ~b error.",
                );
                if ui.button("\u{2715}").clicked() {
                    self.model.filter.clear();
                }
            });
        });

        Panel::top("filters").show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("Method:");
                ui.add(egui::TextEdit::singleline(&mut self.prefs.method).hint_text("GET").desired_width(60.0));
                ui.label("Status:");
                ui.add(egui::TextEdit::singleline(&mut self.prefs.status).hint_text("404").desired_width(50.0));
                ui.label("URL contains:");
                ui.add(egui::TextEdit::singleline(&mut self.prefs.url).hint_text("/api/").desired_width(160.0));
                ui.label("Mark:");
                egui::ComboBox::from_id_salt("mark_filter")
                    .selected_text(self.mark_filter.map(|m| m.as_str()).unwrap_or("any"))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.mark_filter, None, "any");
                        for m in MARKS {
                            ui.selectable_value(&mut self.mark_filter, Some(m), m.as_str());
                        }
                    });
                ui.separator();
                ui.label("Search:");
                ui.add(egui::TextEdit::singleline(&mut self.model.search_query).hint_text("full text (url, headers, bodies, comments)").desired_width(260.0));
                ui.checkbox(&mut self.model.search_in_headers, "Headers");
                ui.checkbox(&mut self.model.search_in_bodies, "Bodies");
                ui.checkbox(&mut self.model.search_case_sensitive, "Match case");
                if ui.button("Reset filters").clicked() {
                    self.prefs.method.clear();
                    self.prefs.status.clear();
                    self.prefs.url.clear();
                    self.mark_filter = None;
                    self.model.filter.clear();
                    self.model.search_query.clear();
                }
                ui.separator();
                if ui.button("Columns...").clicked() {
                    self.show_columns = !self.show_columns;
                }
                if ui.selectable_label(self.show_filter_builder, "Filter Builder...").clicked() {
                    self.show_filter_builder = !self.show_filter_builder;
                }
                if ui.selectable_label(self.show_saved_filters, "Saved Filters...").clicked() {
                    self.show_saved_filters = !self.show_saved_filters;
                }
                if ui.selectable_label(self.show_groups_panel, "Groups...").clicked() {
                    self.show_groups_panel = !self.show_groups_panel;
                }
                ui.separator();
                self.inspect_menu(ui);
                self.data_io_menu(ui);
                self.insights_menu(ui);
                self.rules_ext_menu(ui);
                if self.ext.qol.settings.toolbar_shows("connections") { self.conn_menu(ui); }
                if self.ext.qol.settings.toolbar_shows("intercept") { self.launch_menu(ui); }
                self.scripts_menu(ui);
                self.packets_menu(ui);
                self.popout_menu(ui);
                self.ai_menu(ui);
                self.api_tools_menu(ui);
                self.streams_menu(ui);
                self.llm_traffic_menu(ui);
                self.automation_menu(ui);
                self.qol_menu(ui);
            });
        });
        self.model.structured_filter = self.prefs.criteria(self.mark_filter);
        self.columns_window(ui.ctx());
        self.filter_builder_window(ui.ctx());
        self.saved_filters_window(ui.ctx());
        self.comment_window(ui.ctx());
        self.https_window(ui.ctx());
        self.run_window(ui.ctx(), "composer", |a, c| a.composer_window(c));
        self.run_window(ui.ctx(), "rules", |a, c| a.rules_window(c));
        self.compare_window(ui.ctx());
        self.inspect_windows(ui.ctx());
        self.data_io_windows(ui.ctx());
        self.run_window(ui.ctx(), "connections", |a, c| a.conn_windows(c));
        self.replay_adv_window(ui.ctx());
        self.run_window(ui.ctx(), "intercept", |a, c| a.launch_windows(c));
        self.session_edit_window(ui.ctx());
        self.run_window(ui.ctx(), "scripts", |a, c| a.scripts_windows(c));
        self.run_window(ui.ctx(), "packets", |a, c| a.packets_window(c));
        self.protocols_window(ui.ctx());
        self.android_window(ui.ctx());
        self.ai_window(ui.ctx());
        self.ext.pick.show(ui.ctx());
        self.insights_windows(ui.ctx());
        self.rules_ext_windows(ui.ctx());
        self.api_tools_windows(ui.ctx());
        self.run_window(ui.ctx(), "messages", |a, c| a.streams_windows(c));
        self.llm_traffic_windows(ui.ctx());
        self.automation_windows(ui.ctx());
        self.qol_windows(ui.ctx());
        let mut visible = self.model.visible();
        if let Some(ref rule) = self.filter_rule {
            visible.retain(|&idx| {
                let row = &self.model.rows[idx];
                rule.matches(row)
            });
        }
        if let Some(ref sel_grp) = self.groups_manager.selected_group {
            visible.retain(|&idx| {
                let row = &self.model.rows[idx];
                self.groups_manager.group_by.as_ref().unwrap_or(&GroupBy::Host).key_for(row) == *sel_grp
            });
        }
        if let Some(ids) = self.tabs_manager.active_ids() {
            let ids: std::collections::HashSet<usize> = ids.iter().copied().collect();
            visible.retain(|&idx| ids.contains(&self.model.rows[idx].id));
        }
        self.ext.last_visible = visible.iter().map(|&i| self.model.rows[i].id).collect();
        if !ui.ctx().egui_wants_keyboard_input() {
            let (all, del, esc) = ui.input(|i| (i.modifiers.command && i.key_pressed(egui::Key::A),
                                                 i.key_pressed(egui::Key::Delete), i.key_pressed(egui::Key::Escape)));
            if all {
                self.ext.sel.set(self.ext.last_visible.clone());
            }
            if del {
                let ids = self.selected_ids();
                if !ids.is_empty() {
                    let n = self.model.remove_ids(&ids);
                    self.launch_msg = format!("Removed {n} session(s)");
                    self.ext.sel.clear();
                    return; // the visible rows are stale now; the next frame redraws
                }
            }
            if esc {
                self.ext.sel.clear();
            }
        }
        // G10 (qol.rs): status bar, keyboard shortcuts, notifications; private fields are applied here.
        let qi = crate::qol::AppInfo {
            total: self.model.rows.len(),
            visible: visible.len(),
            selected: self.selected_ids().len(),
            capturing: self.capturing,
            port: crate::conn::active_port(),
            proxy_on: self.proxy_on,
            active_rules: self.rules.iter().filter(|r| r.enabled).count(),
            active_breakpoints: self.bp_rules.iter().filter(|r| r.enabled).count(),
            paused: crate::model::breakpoint_hub().count(),
        };
        let qa = self.qol_frame(ui.ctx(), &qi);
        if let Some(c) = qa.capture { self.capturing = c; }
        if let Some(r) = qa.rules { self.show_rules = r; }
        Panel::top("traffic_tabs").show(ui, |ui| self.traffic_tabs_bar(ui));
        Panel::bottom("status").show(ui, |ui| self.qol_status_bar(ui, &qi));
        if qa.clear_all {
            self.model.apply(SessionEvent::Clear);
            self.ext.sel.clear();
            self.launch_msg = "Cleared all sessions".into();
            return; // the visible rows are stale now; the next frame redraws
        }

        if self.ext.popout_inspector {
            self.inspector_popout(ui.ctx());
        } else {
            let panel = if self.layout_split_horizontal { Panel::right("inspector_side") } else { Panel::bottom("inspector") };
            panel
                .resizable(true)
                .default_size(if self.layout_split_horizontal { 520.0 } else { 300.0 })
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if ui.small_button("Pop out").on_hover_text("Show the inspector in its own window").clicked() {
                            self.ext.popout_inspector = true;
                        }
                    });
                    match self.model.selected_row().cloned() {
                        Some(s) => inspector_panel(ui, &mut self.req_tab, &mut self.resp_tab, &mut self.ext.inspect, &s),
                        None => {
                            ui.vertical_centered(|ui| {
                                ui.add_space(30.0);
                                ui.label(RichText::new("Select a session in the grid above to inspect its Request and Response.").italics().color(Color32::GRAY));
                            });
                        }
                    }
                });
        }

        let grew = self.model.rows.len() > self.last_len;
        self.last_len = self.model.rows.len();

        let cols: Vec<(GridColumn, f32)> = self.model.column_state.columns.iter().filter(|c| c.visible).map(|c| (c.col, c.width)).collect();
        let mut pending_mark: Option<(usize, Option<SessionMark>)> = None;
        let mut pending_comment: Option<usize> = None;
        let mut pending_clear_comment: Option<usize> = None;
        let mut pending_replay: Option<usize> = None;
        let mut pending_composer: Option<(usize, bool)> = None;
        let mut pending_replay_adv = false;
        let mut pending_edit: Option<usize> = None;
        let mut pending_copy: Option<String> = None;
        let mut pending_filter: Option<String> = None;
        let mut pending_remove: Option<usize> = None;
        let mut pending_select: Option<Vec<usize>> = None;
        let mut pending_compare: Option<(usize, usize)> = None;
        let mut pending_compare_left: Option<usize> = None;
        let mut pending_mark_many: Option<(Vec<usize>, Option<SessionMark>)> = None;
        let mut pending_remove_many: Option<Vec<usize>> = None;
        let multi: Vec<usize> = self.ext.sel.to_vec();
        let compare_left = self.ext.compare.left;
        let mut clicked_sort = None;
        let mut clicked_row: Option<(usize, bool, bool, bool)> = None;
        let sort = self.model.sort;

        self.groups_side_panel(ui);
        egui::CentralPanel::default().show(ui, |ui| {
            let mut table = TableBuilder::new(ui).striped(true).sense(egui::Sense::click()).resizable(true);
            for (c, w) in &cols {
                table = table.column(if *c == GridColumn::Url {
                    Column::remainder().at_least(200.0).clip(true)
                } else {
                    Column::initial(*w)
                });
            }
            for _ in &self.custom_columns {
                table = table.column(Column::initial(120.0));
            }
            if self.auto_scroll && grew && !visible.is_empty() {
                table = table.scroll_to_row(visible.len() - 1, Some(egui::Align::BOTTOM));
            }
            table
                .header(22.0, |mut header| {
                    for (c, _) in &cols {
                        header.col(|ui| {
                            let sc = sort_col(*c);
                            let arrow = match sort {
                                Some((x, true)) if x == sc => " \u{25b2}",
                                Some((x, false)) if x == sc => " \u{25bc}",
                                _ => "",
                            };
                            if ui.button(RichText::new(format!("{}{arrow}", c.title())).strong()).clicked() {
                                clicked_sort = Some(sc);
                            }
                        });
                    }
                    for cc in &self.custom_columns {
                        header.col(|ui| {
                            ui.label(RichText::new(cc.title.as_str()).strong());
                        });
                    }
                })
                .body(|body| {
                    body.rows(20.0, visible.len(), |mut row| {
                        let s = &self.model.rows[visible[row.index()]];
                        row.set_selected(self.model.selected == Some(s.id) || self.ext.sel.contains(s.id));
                        let mark = s.mark;
                        for (c, _) in &cols {
                            row.col(|ui| {
                                if let Some(bg) = mark.and_then(mark_bg) {
                                    ui.painter().rect_filled(ui.max_rect(), 0.0, bg);
                                }
                                let (text, color) = cell_text(s, *c);
                                let mut rt = RichText::new(text);
                                if let Some(col) = color { rt = rt.color(col); }
                                if mark == Some(SessionMark::Strikeout) { rt = rt.strikethrough(); }
                                ui.label(rt);
                            });
                        }
                        for cc in &self.custom_columns {
                            row.col(|ui| {
                                ui.label(cc.extract_value(s));
                            });
                        }
                        let resp = row.response();
                        if resp.clicked() || resp.secondary_clicked() {
                            let m = resp.ctx.input(|i| i.modifiers);
                            clicked_row = Some((s.id, m.command, m.shift, resp.secondary_clicked()));
                        }
                        resp.context_menu(|ui| {
                            // actions on several sessions when the clicked row is part of a multi-selection
                            if multi.len() > 1 && multi.contains(&s.id) {
                                ui.label(RichText::new(format!("{} sessions selected", multi.len())).strong());
                                ui.menu_button("Mark selected", |ui| {
                                    for m in MARKS {
                                        if ui.button(m.as_str()).clicked() {
                                            pending_mark_many = Some((multi.clone(), Some(m)));
                                            ui.close();
                                        }
                                    }
                                    if ui.button("Clear marks").clicked() {
                                        pending_mark_many = Some((multi.clone(), None));
                                        ui.close();
                                    }
                                });
                                if ui.button(format!("Copy {} URLs", multi.len())).clicked() {
                                    let urls: Vec<String> = multi.iter()
                                        .filter_map(|id| self.model.rows.iter().find(|r| r.id == *id).map(|r| r.url.clone())).collect();
                                    pending_copy = Some(urls.join("\n"));
                                    ui.close();
                                }
                                if multi.len() == 2 && ui.button("Compare these two").clicked() {
                                    pending_compare = Some((multi[0], multi[1]));
                                    ui.close();
                                }
                                if ui.button(format!("Remove {} selected", multi.len())).clicked() {
                                    pending_remove_many = Some(multi.clone());
                                    ui.close();
                                }
                                ui.separator();
                            }
                            ui.label(RichText::new(format!("Session #{}", s.id)).strong());
                            ui.menu_button("Select related", |ui| {
                                let rows = &self.model.rows;
                                let dup = crate::selection::duplicates_of(rows, s.id);
                                if ui.button(format!("Duplicates of this request ({})", dup.len())).clicked() {
                                    pending_select = Some(dup);
                                    ui.close();
                                }
                                let host = crate::selection::same_host(rows, s.id);
                                if ui.button(format!("Same host ({})", host.len())).clicked() {
                                    pending_select = Some(host);
                                    ui.close();
                                }
                                let kids = crate::selection::children_of(rows, s.id);
                                if ui.add_enabled(!kids.is_empty(), egui::Button::new(format!("Child requests ({})", kids.len()))).clicked() {
                                    pending_select = Some(kids);
                                    ui.close();
                                }
                                let parent = crate::selection::parent_of(rows, s.id);
                                if ui.add_enabled(parent.is_some(), egui::Button::new("Parent request")).clicked() {
                                    pending_select = parent.map(|p| vec![p]);
                                    ui.close();
                                }
                                if ui.button("All visible (Ctrl+A)").clicked() {
                                    pending_select = Some(Vec::new()); // empty = all visible
                                    ui.close();
                                }
                            });
                            match compare_left {
                                Some(l) if l != s.id => {
                                    if ui.button(format!("Compare with #{l}")).clicked() {
                                        pending_compare = Some((l, s.id));
                                        ui.close();
                                    }
                                }
                                _ => {
                                    if ui.button("Set as compare left").clicked() {
                                        pending_compare_left = Some(s.id);
                                        ui.close();
                                    }
                                }
                            }
                            for m in MARKS {
                                let label = if s.mark == Some(m) { format!("\u{2713} Mark {}", m.as_str()) } else { format!("Mark {}", m.as_str()) };
                                if ui.button(label).clicked() {
                                    pending_mark = Some((s.id, Some(m)));
                                    ui.close();
                                }
                            }
                            if ui.add_enabled(s.mark.is_some(), egui::Button::new("Clear mark")).clicked() {
                                pending_mark = Some((s.id, None));
                                ui.close();
                            }
                            ui.separator();
                            if ui.button("Comment...").clicked() {
                                pending_comment = Some(s.id);
                                ui.close();
                            }
                            if ui.add_enabled(!s.comment.is_empty(), egui::Button::new("Clear comment")).clicked() {
                                pending_clear_comment = Some(s.id);
                                ui.close();
                            }
                            ui.separator();
                            if ui.button("Filter to this host").on_hover_text("Show only this host in the grid").clicked() { pending_filter = Some(format!("host:{}", s.host())); ui.close(); }
                            if ui.button("Hide this host").on_hover_text("Exclude this host from the grid").clicked() { pending_filter = Some(format!("-host:{}", s.host())); ui.close(); }
                            ui.separator();
                            if ui.button("Copy URL").clicked() { pending_copy = Some(s.url.clone()); ui.close(); }
                            if ui.button("Copy row (tab separated)").clicked() { pending_copy = Some(crate::g1::copy_row(s)); ui.close(); }
                            if ui.button("Copy summary").clicked() { pending_copy = Some(crate::g1::copy_summary(s)); ui.close(); }
                            ui.menu_button("Copy as code", |ui| {
                                for lang in crate::codegen::LANGUAGES {
                                    if ui.button(lang).clicked() {
                                        pending_copy = Some(crate::codegen::generate(lang, s));
                                        ui.close();
                                    }
                                }
                            });
                            if ui.button("Copy as HAR").on_hover_text("This session as a one-entry HAR document").clicked() {
                                pending_copy = Some(serde_json::to_string_pretty(&crate::har::to_har(std::slice::from_ref(s))).unwrap_or_default());
                                ui.close();
                            }

                            if ui.button("Replay").clicked() { pending_replay = Some(s.id); ui.close(); }
                            if ui.button("Replay advanced...").on_hover_text("Send the selected sessions N times, in parallel or paced, with a results summary").clicked() { pending_replay_adv = true; ui.close(); }
                            if ui.button("Edit session...").on_hover_text("Change the captured request / response, replay it, or serve the response for this URL").clicked() { pending_edit = Some(s.id); ui.close(); }
                            if ui.button("Open in Composer").on_hover_text("Edit this request, then send it").clicked() { pending_composer = Some((s.id, false)); ui.close(); }
                            if ui.button("Save as Composer request").on_hover_text("Add it to the \"Saved from sessions\" collection and open it").clicked() { pending_composer = Some((s.id, true)); ui.close(); }
                            ui.separator();
                            if ui.button("Remove session").clicked() { pending_remove = Some(s.id); ui.close(); }
                        });
                    });
                });
        });
        if let Some(c) = clicked_sort {
            self.model.toggle_sort(c);
        }
        if let Some((id, ctrl, shift, secondary)) = clicked_row {
            if secondary {
                if !self.ext.sel.contains(id) {
                    self.ext.sel.only(id); // right-click outside the selection selects just that row
                }
            } else {
                let order = self.ext.last_visible.clone();
                self.ext.sel.click(id, &order, ctrl, shift);
            }
            self.model.selected = Some(id);
        }
        if let Some(ids) = pending_select {
            let ids = if ids.is_empty() { self.ext.last_visible.clone() } else { ids };
            self.model.selected = ids.first().copied();
            self.launch_msg = format!("Selected {} session(s)", ids.len());
            self.ext.sel.set(ids);
        }
        if let Some((ids, m)) = pending_mark_many {
            for id in ids {
                self.model.mark_session(id, m);
                self.sync_annotation(id);
            }
        }
        if let Some(l) = pending_compare_left {
            self.ext.compare.left = Some(l);
            self.launch_msg = format!("Session #{l} set for compare: right-click another session and pick Compare");
        }
        if let Some((a, b)) = pending_compare {
            self.compare_sessions(a, b);
        }
        if let Some(ids) = pending_remove_many {
            let n = self.model.remove_ids(&ids);
            self.ext.sel.clear();
            self.launch_msg = format!("Removed {n} session(s)");
        }
        if let Some((id, m)) = pending_mark {
            self.model.mark_session(id, m);
            self.sync_annotation(id);
        }
        if let Some(id) = pending_comment {
            let cur = self.model.rows.iter().find(|r| r.id == id).map(|r| r.comment.clone()).unwrap_or_default();
            self.comment_edit = Some((id, cur));
        }
        if pending_replay_adv { self.replay_adv_open(); }
        if let Some(id) = pending_edit { self.session_edit_open(id); }
        if let Some((id, save)) = pending_composer {
            self.session_to_composer(id, save);
        }
        if let Some(id) = pending_replay {
            if let Some(s) = self.model.rows.iter().find(|r| r.id == id).cloned() {
                if crate::g1_replay::replayable(&s) {
                    let proxy = format!("127.0.0.1:{}", crate::conn::active_port());
                    std::thread::spawn(move || { let _ = crate::g1_replay::replay_sessions(&[s], &proxy, crate::g1_replay::ReplayOptions::default()); });
                    self.launch_msg = "Replaying session through the proxy".to_string();
                } else {
                    self.launch_msg = "CONNECT tunnels cannot be replayed".to_string();
                }
            }
        }
        if let Some(text) = pending_copy {
            ui.ctx().copy_text(text);
            self.launch_msg = "Copied to clipboard".to_string();
        }
        if let Some(f) = pending_filter {
            self.model.filter = f;
        }
        if let Some(id) = pending_remove {
            let n = self.model.remove_ids(&[id]);
            self.launch_msg = format!("Removed {} session", n);
        }
        if let Some(id) = pending_clear_comment {
            self.model.set_comment(id, String::new());
            self.sync_annotation(id);
        }

        // Persist columns / filters / search / marks / comments whenever anything changed.
        self.prefs.from_model(&self.model);
        self.prefs.custom_columns = self.custom_columns.clone();
        let mut pins: Vec<String> = self.groups_manager.pinned_keys.iter().cloned().collect();
        pins.sort();
        self.prefs.pinned_groups = pins;
        let snap = serde_json::to_string(&self.prefs).unwrap_or_default();
        if snap != self.last_saved && self.prefs.save(&self.prefs_path).is_ok() {
            self.last_saved = snap;
        }
    }
}

fn sort_col(c: GridColumn) -> SortCol {
    match c {
        GridColumn::Id => SortCol::Id,
        GridColumn::Status => SortCol::Status,
        GridColumn::Protocol => SortCol::Protocol,
        GridColumn::Host => SortCol::Host,
        GridColumn::Url => SortCol::Url,
        GridColumn::Method => SortCol::Method,
        GridColumn::Size => SortCol::Size,
        GridColumn::ContentType => SortCol::ContentType,
        GridColumn::Time => SortCol::Time,
        GridColumn::Duration => SortCol::Duration,
        GridColumn::Comments => SortCol::Comments,
        GridColumn::ConnectTime => SortCol::ConnectTime,
        GridColumn::HandshakeTime => SortCol::HandshakeTime,
        GridColumn::Ttfb => SortCol::Ttfb,
        GridColumn::TlsVersion => SortCol::TlsVersion,
        GridColumn::HttpVersion => SortCol::HttpVersion,
        GridColumn::ProcessName => SortCol::ProcessName,
        GridColumn::Pid => SortCol::Pid,
        GridColumn::ClientIp => SortCol::ClientIp,
        GridColumn::RemoteIp => SortCol::RemoteIp,
        GridColumn::ReqSize => SortCol::ReqSize,
    }
}

fn mark_bg(m: SessionMark) -> Option<Color32> {
    let (r, g, b) = match m {
        SessionMark::Red => (220, 60, 60),
        SessionMark::Blue => (60, 120, 220),
        SessionMark::Green => (60, 180, 90),
        SessionMark::Orange => (240, 150, 40),
        SessionMark::Purple => (150, 80, 200),
        SessionMark::Copper => (184, 115, 51),
        SessionMark::Strikeout => return None,
    };
    Some(Color32::from_rgba_unmultiplied(r, g, b, 80))
}

pub fn fmt_opt_ms(v: Option<u64>) -> String {
    v.map(|x| format!("{} ms", x)).unwrap_or_default()
}

fn cell_text(s: &SessionRecord, c: GridColumn) -> (String, Option<Color32>) {
    match c {
        GridColumn::Id => (s.id.to_string(), None),
        GridColumn::Status => (s.status.to_string(), Some(status_color(s.status))),
        GridColumn::Protocol => (s.http_version.clone(), None),
        GridColumn::Host => (s.host().to_string(), None),
        GridColumn::Url => (s.path().to_string(), None),
        GridColumn::Method => (s.method.clone(), None),
        GridColumn::Size => (fmt_size(s.body_size()), None),
        GridColumn::ContentType => (s.content_type().to_string(), None),
        GridColumn::Time => (s.time_of_day().to_string(), None),
        GridColumn::Duration => (format!("{} ms", s.duration_ms), None),
        GridColumn::Comments => (s.comment.clone(), None),
        GridColumn::ConnectTime => (fmt_opt_ms(s.connect_ms), None),
        GridColumn::HandshakeTime => (fmt_opt_ms(s.tls_ms), None),
        GridColumn::Ttfb => (fmt_opt_ms(s.ttfb_ms), None),
        GridColumn::TlsVersion => (s.tls_version.clone().unwrap_or_default(), None),
        GridColumn::HttpVersion => (s.http_version.clone(), None),
        GridColumn::ProcessName => (s.process_name.clone().unwrap_or_default(), None),
        GridColumn::Pid => (s.pid.map(|p| p.to_string()).unwrap_or_default(), None),
        GridColumn::ClientIp => (s.client_ip.clone().unwrap_or_default(), None),
        GridColumn::RemoteIp => (s.remote_ip.clone().unwrap_or_default(), None),
        GridColumn::ReqSize => (fmt_size(s.request_body.len()), None),
    }
}


impl GridApp {
    /// Test hook (env FERROTAP_TEST_DIR): executes <dir>/cmd.txt through the same model/prefs paths the
    /// controls use, then dumps the resulting UI state to <dir>/dump.json.
    fn test_hook(&mut self) {
        let Some(dir) = self.test_dir.clone() else { return };
        if self.dump_in > 0 {
            self.dump_in -= 1;
            if self.dump_in == 0 {
                self.write_dump(&dir);
            }
            return;
        }
        let cmd = dir.join("cmd.txt");
        let Ok(text) = std::fs::read_to_string(&cmd) else { return };
        let _ = std::fs::remove_file(&cmd);
        for line in text.lines() {
            let line = line.trim();
            let (k, v) = line.split_once('=').unwrap_or((line, ""));
            let by_url = |m: &SessionModel, sub: &str| m.rows.iter().find(|r| r.url.contains(sub)).map(|r| r.id);
            match k {
                "method" => self.prefs.method = v.to_string(),
                "status" => self.prefs.status = v.to_string(),
                "url" => self.prefs.url = v.to_string(),
                "search" => self.model.search_query = v.to_string(),
                        "filter_builder_rule" => {
                            let parts: Vec<&str> = v.splitn(3, ':').collect();
                            if parts.len() == 3 {
                                let f = match parts[0] {
                                    "method" => FilterField::Method,
                                    "status" => FilterField::Status,
                                    _ => FilterField::Host,
                                };
                                let op = match parts[1] {
                                    "eq" => ConditionOp::Equals,
                                    "contains" => ConditionOp::Contains,
                                    _ => ConditionOp::Contains,
                                };
                                self.filter_rule = Some(AdvancedFilterRule::single(FilterCondition::new(f, op, parts[2])));
                            }
                        },
                        "clear_filter_rule" => self.filter_rule = None,
                        "add_custom_column" => {
                            let parts: Vec<&str> = v.splitn(2, ':').collect();
                            let title = parts[0];
                            let hdr = if parts.len() > 1 { parts[1] } else { parts[0] };
                            self.custom_columns.push(CustomColumnDef::new(hdr, title, ColumnSource::Header(hdr.to_string())));
                        },
                        "set_group_by" => {
                            match v {
                                "host" => self.groups_manager.set_group_by(GroupBy::Host),
                                "method" => self.groups_manager.set_group_by(GroupBy::Method),
                                "status" => self.groups_manager.set_group_by(GroupBy::StatusClass),
                                _ => {}
                            }
                            self.groups_manager.rebuild(&self.model.rows);
                        },
                        "select_group" => {
                            self.groups_manager.selected_group = if v.is_empty() { None } else { Some(v.to_string()) };
                        },
                        "save_filter" => {
                            let name = if v.is_empty() { "default_saved" } else { v };
                            let r = self.filter_rule.clone().unwrap_or_else(|| AdvancedFilterRule::single(FilterCondition::new(FilterField::Method, ConditionOp::Equals, "GET")));
                            self.saved_filters.add(name, r);
                        },
                        "toggle_layout" => {
                            self.layout_split_horizontal = !self.layout_split_horizontal;
                        },
                "headers" => self.model.search_in_headers = v == "1",
                "bodies" => self.model.search_in_bodies = v == "1",
                "reset" => {
                    self.prefs.method.clear();
                    self.prefs.status.clear();
                    self.prefs.url.clear();
                    self.mark_filter = None;
                    self.model.filter.clear();
                    self.model.search_query.clear();
                    self.model.search_in_headers = true;
                    self.model.search_in_bodies = true;
                }
                "markfilter" => self.mark_filter = SessionMark::parse(v),
                "mark" => {
                    if let Some((c, sub)) = v.split_once('@') {
                        if let Some(id) = by_url(&self.model, sub) {
                            self.model.mark_session(id, SessionMark::parse(c));
                            self.sync_annotation(id);
                        }
                    }
                }
                "comment" => {
                    if let Some((t, sub)) = v.split_once('@') {
                        if let Some(id) = by_url(&self.model, sub) {
                            self.model.set_comment(id, t.to_string());
                            self.sync_annotation(id);
                        }
                    }
                }
                "hidecol" | "showcol" => {
                    for c in self.model.column_state.columns.iter_mut() {
                        if c.col.title().to_string().eq_ignore_ascii_case(v) {
                            c.visible = k == "showcol";
                        }
                    }
                }
                "composer" => self.show_composer = v == "1",
                "cmethod" => self.comp_method = v.to_string(),
                "curl" => self.comp_url = v.to_string(),
                "cheaders" => self.comp_hdr_rows = crate::model::rows_from_header_lines(&v.replace(";;", "\n")),
                "cbody" => self.comp_body = v.to_string(),
                "csend" => { if let Some(c) = self.ctx_for_hook.clone() { self.composer_send(&c); } }
                "cparams" => self.comp_params = crate::model::parse_query(&format!("x?{v}")).1,
                "cclear" => { self.comp_method = "GET".into(); self.comp_url.clear(); self.comp_params.clear(); self.comp_hdr_rows.clear(); self.comp_body.clear(); self.comp_resp = None; }
                "csave" => { if let Some((c, n)) = v.split_once('|') { self.comp_msg = match self.comp_save(c, n) { Ok(()) => format!("saved {n}"), Err(e) => e }; } }
                "cload" => { if let Some((c, n)) = v.split_once('|') { let ok = self.comp_load(c, n); self.comp_msg = format!("load {n}: {ok}"); } }
                "radd" => { let f: Vec<&str> = v.splitn(5, '|').collect(); if f.len() == 5 { let mut r = crate::model::Rule::default(); r.name = format!("rule{}", self.rules.len() + 1); r.match_kind = f[0].into(); r.pattern = f[1].into(); r.method = f[2].into(); r.status = f[3].parse().unwrap_or(200); r.body = f[4].into(); if !r.body.is_empty() { r.headers.push(crate::model::KvRow { enabled: true, key: "X-Rule".into(), value: "canned".into() }); } self.rules_add(r); } }
                "rfile" => { if let Some((pat, path)) = v.split_once('|') { let mut r = crate::model::Rule::default(); r.name = format!("rule{}", self.rules.len() + 1); r.pattern = pat.into(); r.action = "file".into(); r.file = path.into(); self.rules_add(r); } }
                "rtoggle" => { if let Ok(i) = v.parse::<usize>() { if let Some(r) = self.rules.get_mut(i) { r.enabled = !r.enabled; } self.rules_persist(); } }
                "rdel" => { if let Ok(i) = v.parse::<usize>() { if i < self.rules.len() { self.rules.remove(i); } self.rules_sel = None; self.rules_persist(); } }
                "rmove" => { if let Some((a, b)) = v.split_once('|') { if let (Ok(i), Ok(d)) = (a.parse::<usize>(), b.parse::<i32>()) { self.rules_move(i, d); } } }
                "rclear" => { self.rules.clear(); self.rules_sel = None; self.rules_persist(); }
                "cdel" => { if let Some((c, n)) = v.split_once('|') { let ok = self.comp_delete_request(c, n); self.comp_msg = format!("del {n}: {ok}"); } }
                "chist" => { if let Ok(i) = v.parse::<usize>() { let ok = self.comp_load_history(i); self.comp_msg = format!("hist {i}: {ok}"); } }
                _ => {}
            }
        }
        self.dump_in = 3;
    }

    fn write_dump(&self, dir: &std::path::Path) {
        let vis = self.model.visible();
        let rows: Vec<serde_json::Value> = vis
            .iter()
            .map(|&i| {
                let r = &self.model.rows[i];
                serde_json::json!({"id": r.id, "method": r.method, "status": r.status, "url": r.url,
                    "mark": r.mark.map(|m| m.as_str()), "comment": r.comment})
            })
            .collect();
        let cols: Vec<String> = self.model.column_state.columns.iter().filter(|c| c.visible).map(|c| c.col.title().to_string()).collect();
        let v = serde_json::json!({"total": self.model.rows.len(), "visible": rows, "columns": cols,
            "filters": {"method": self.prefs.method, "status": self.prefs.status, "url": self.prefs.url,
                "search": self.model.search_query,
                                    "custom_columns_count": self.custom_columns.len(),
                                    "saved_filters_count": self.saved_filters.filters().len(),
                                    "groups_count": self.groups_manager.groups.len(),
                                    "selected_group": self.groups_manager.selected_group.clone(),
                                    "has_filter_rule": self.filter_rule.is_some(),
                                    "layout_split_horizontal": self.layout_split_horizontal},
            "rules": {"list": self.rules, "msg": self.rules_msg, "open": self.show_rules},
                "composer": {"open": self.show_composer, "sending": self.comp_sending, "method": self.comp_method, "url": self.comp_url, "params": self.comp_params, "headers_rows": self.comp_hdr_rows, "body": self.comp_body,
                "collections": self.comp_collections, "history_len": self.comp_history.len(), "last_history": self.comp_history.first(), "msg": self.comp_msg, "response": self.comp_resp}});
        let tmp = dir.join("dump.tmp");
        if std::fs::write(&tmp, serde_json::to_string_pretty(&v).unwrap_or_default()).is_ok() {
            let _ = std::fs::rename(&tmp, dir.join("dump.json"));
        }
    }
}


use crate::model::{Collection, HistoryEntry, KvRow, SavedRequest};

fn kv_editor(ui: &mut egui::Ui, id: &str, rows: &mut Vec<KvRow>) {
    let mut del = None;
    egui::Grid::new(id).num_columns(4).striped(true).show(ui, |ui| {
        for (i, r) in rows.iter_mut().enumerate() {
            ui.checkbox(&mut r.enabled, "");
            ui.add(egui::TextEdit::singleline(&mut r.key).hint_text("name").desired_width(150.0));
            ui.add(egui::TextEdit::singleline(&mut r.value).hint_text("value").desired_width(230.0));
            if ui.small_button("x").clicked() { del = Some(i); }
            ui.end_row();
        }
    });
    if let Some(i) = del { rows.remove(i); }
    if ui.button("+ Add").clicked() { rows.push(KvRow::new("", "")); }
}

impl GridApp {
    pub(crate) fn current_saved(&self, name: &str) -> SavedRequest {
        SavedRequest {
            name: name.to_string(),
            method: self.comp_method.clone(),
            url: self.comp_url.clone(),
            params: self.comp_params.clone(),
            headers: self.comp_hdr_rows.clone(),
            body: self.ext.api.effective_body(&self.comp_body),
            tests: self.ext.api.comp_tests.clone(),
            body_mode: self.ext.api.body_mode(),
            example: None,
        }
    }

    pub(crate) fn apply_saved(&mut self, r: &SavedRequest) {
        self.comp_method = if r.method.is_empty() { "GET".into() } else { r.method.clone() };
        self.comp_url = r.url.clone();
        self.comp_params = r.params.clone();
        self.comp_hdr_rows = r.headers.clone();
        self.comp_body = r.body.clone();
        self.ext.api.load_from(r);
    }

    /// Load a parsed request (e.g. from a pasted cURL command) into the Composer, splitting any
    /// query string into the params grid.
    pub(crate) fn apply_composer_request(&mut self, req: &crate::model::ComposerRequest) {
        let (base, params) = crate::model::parse_query(&req.url);
        self.comp_method = if req.method.is_empty() { "GET".into() } else { req.method.clone() };
        self.comp_url = base;
        self.comp_params = params;
        self.comp_hdr_rows = req.headers.iter().map(|(k, v)| crate::model::KvRow::new(k, v)).collect();
        self.comp_body = req.body.clone();
        self.comp_tab = 0;
    }

    /// Sessions for the API tools: the selected one, or all visible ones.
    pub(crate) fn api_sessions(&self, only_selected: bool) -> Vec<crate::model::SessionRecord> {
        if only_selected {
            self.model.selected.and_then(|id| self.model.rows.iter().find(|r| r.id == id)).cloned().into_iter().collect()
        } else {
            self.model.visible().iter().filter_map(|&i| self.model.rows.get(i)).cloned().collect()
        }
    }

    pub fn comp_reload(&mut self) {
        self.comp_collections = crate::model::load_collections(&self.comp_dir);
        self.comp_history = crate::model::load_history(&self.comp_dir);
        self.comp_loaded = true;
    }

    /// Save the current Composer request into collection `coll` under `name` (file under collections dir).
    pub fn comp_save(&mut self, coll: &str, name: &str) -> Result<(), String> {
        let (coll, name) = (coll.trim(), name.trim());
        if coll.is_empty() || name.is_empty() { return Err("collection and request name are required".into()); }
        self.comp_reload();
        let mut req = self.current_saved(name);
        let old_example = self.comp_collections.iter().find(|c| c.name == coll).and_then(|c| c.find(name)).and_then(|r| r.example.clone());
        req.example = match &self.comp_resp {
            Some(r) if r.error.is_empty() && r.status > 0 && !self.ext.api.no_example => Some(crate::api_tools::SavedResponse::from(r)),
            _ => old_example,
        };
        let mut c = self.comp_collections.iter().find(|c| c.name == coll).cloned().unwrap_or_else(|| Collection::new(coll));
        c.upsert(req);
        crate::model::save_collection(&self.comp_dir, &c)?;
        self.comp_reload();
        Ok(())
    }

    /// Add a session to the grid as if it was just captured (tests, imports).
    pub fn ingest_session(&mut self, s: crate::model::SessionRecord) {
        self.model.apply(SessionEvent::New(s));
    }

    /// Number of captured sessions (all tabs, ignoring filters).
    pub fn session_count(&self) -> usize {
        self.model.rows.len()
    }

    /// Keep only the most recent `n` sessions (0 = all), as Clear > Keep does.
    pub fn set_keep_sessions(&mut self, n: usize) {
        self.prefs.keep_sessions = n;
    }

    /// Set the grid's quick filter text, as typing in the filter box does.
    pub fn set_quick_filter(&mut self, text: &str) {
        self.model.filter = text.to_string();
    }

    /// Focus one session (as a click in the grid would).
    pub fn select(&mut self, id: usize) {
        self.model.selected = Some(id);
        self.ext.sel.only(id);
    }

    /// Load a captured session into the Composer (a new tab when tabs are in use); with `save`, also
    /// store it in the "Saved from sessions" collection (the captured response becomes its example).
    pub fn session_to_composer(&mut self, id: usize, save: bool) -> bool {
        const COLL: &str = "Saved from sessions";
        let Some(s) = self.model.rows.iter().find(|r| r.id == id).cloned() else { return false };
        let Some(mut r) = crate::api_tools::collection_from_sessions(COLL, &[&s]).requests.into_iter().next() else {
            self.launch_msg = "CONNECT tunnels cannot be opened in the Composer".into();
            return false;
        };
        if !self.ext.api.tabs.is_empty() { self.api_tab_new(); }
        if save {
            self.comp_reload();
            let mut c = self.comp_collections.iter().find(|c| c.name == COLL).cloned().unwrap_or_else(|| Collection::new(COLL));
            let base = r.name.clone();
            let mut i = 2;
            while c.find(&r.name).is_some() { r.name = format!("{base} ({i})"); i += 1; }
            c.upsert(r.clone());
            if let Err(e) = crate::model::save_collection(&self.comp_dir, &c) {
                self.launch_msg = format!("Could not save the request: {e}");
                return false;
            }
            self.comp_reload();
            self.comp_save_coll = COLL.to_string();
            self.comp_save_name = r.name.clone();
            self.launch_msg = format!("Saved session #{id} as \"{}\" in {COLL}", r.name);
        }
        r.example = None;
        self.apply_saved(&r);
        self.comp_resp = None;
        self.show_composer = true;
        true
    }

    pub fn comp_load(&mut self, coll: &str, name: &str) -> bool {
        self.comp_reload();
        let r = self.comp_collections.iter().find(|c| c.name == coll).and_then(|c| c.find(name)).cloned();
        match r {
            Some(r) => { self.apply_saved(&r); self.comp_save_coll = coll.to_string(); self.comp_save_name = name.to_string(); true }
            None => false,
        }
    }

    pub fn comp_delete_request(&mut self, coll: &str, name: &str) -> bool {
        self.comp_reload();
        let Some(mut c) = self.comp_collections.iter().find(|c| c.name == coll).cloned() else { return false };
        let ok = c.remove(name);
        if ok && crate::model::save_collection(&self.comp_dir, &c).is_err() { return false; }
        self.comp_reload();
        ok
    }

    pub fn comp_load_history(&mut self, i: usize) -> bool {
        match self.comp_history.get(i).cloned() {
            Some(h) => { self.apply_saved(&h.request); true }
            None => false,
        }
    }

    /// Dispatch the current Composer request through the capture proxy on a worker thread.
    pub fn composer_send(&mut self, ctx: &egui::Context) {
        let sr = self.current_saved("");
        let req = crate::api_tools::prepare(&sr, &self.ext.api.active_vars());
        let settings = self.ext.api.settings.clone();
        self.comp_last = Some(sr);
        self.comp_sending = true;
        let tx = self.comp_tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let r = crate::api_tools::send_request(&req, &settings, &format!("127.0.0.1:{}", crate::conn::active_port()));
            let _ = tx.send(r);
            ctx.request_repaint();
        });
    }

    fn poll_composer(&mut self) {
        while let Ok(r) = self.comp_rx.try_recv() {
            self.comp_sending = false;
            if !self.comp_loaded { self.comp_reload(); }
            if let Some(req) = self.comp_last.take() {
                let e = HistoryEntry { ts: crate::model::now_secs(), status: r.status, error: r.error.clone(), elapsed_ms: r.elapsed_ms as u64, request: req };
                crate::model::history_push(&mut self.comp_history, e);
                let _ = crate::model::save_history(&self.comp_dir, &self.comp_history);
            }
            self.ext.api.last_results = crate::api_tools::run_tests(&self.ext.api.comp_tests, &r);
            self.comp_resp = Some(r);
        }
    }

    /// Composer window: method, URL, key-value params/headers, body, Send, collections and history.
    pub fn composer_window(&mut self, ctx: &egui::Context) {
        self.poll_composer();
        if !self.show_composer { return; }
        if !self.comp_loaded { self.comp_reload(); }
        let mut open = true;
        let mut do_send = false;
        let mut do_save = false;
        let mut act_load: Option<(String, String)> = None;
        let mut act_del: Option<(String, String)> = None;
        let mut act_hist: Option<usize> = None;
        let mut clear_hist = false;
        egui::Window::new("Composer").open(&mut open).default_width(640.0).show(ctx, |ui| {
            self.api_composer_tabs(ui);
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("comp_method").selected_text(self.comp_method.clone()).show_ui(ui, |ui| {
                    for m in ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"] {
                        ui.selectable_value(&mut self.comp_method, m.to_string(), m);
                    }
                });
                let resp = ui.add(egui::TextEdit::singleline(&mut self.comp_url).hint_text("http://127.0.0.1:8898/path  (or paste a curl command)").desired_width(340.0));
                if resp.changed() {
                    // Pasting a curl command into the URL bar expands it into the whole request.
                    if self.comp_url.trim_start().starts_with("curl ") {
                        if let Some(req) = crate::codegen::parse_curl(&self.comp_url) {
                            self.apply_composer_request(&req);
                        }
                    } else if self.comp_url.contains('?') {
                        let (b, rows) = crate::model::parse_query(&self.comp_url);
                        self.comp_url = b;
                        self.comp_params.extend(rows);
                        self.comp_tab = 0;
                    }
                }
                let label = if self.comp_sending { "Sending..." } else { "Send" };
                if ui.add_enabled(!self.comp_sending, egui::Button::new(label)).clicked() { do_send = true; }
            });
            ui.horizontal(|ui| {
                ui.label("Save to collection:");
                ui.add(egui::TextEdit::singleline(&mut self.comp_save_coll).desired_width(120.0));
                ui.label("as");
                ui.add(egui::TextEdit::singleline(&mut self.comp_save_name).hint_text("request name").desired_width(140.0));
                if ui.button("Save").clicked() { do_save = true; }
                let mut keep = !self.ext.api.no_example;
                if ui.checkbox(&mut keep, "with response").on_hover_text("Store the last response as the request's example (used by the mock server and exports)").changed() { self.ext.api.no_example = !keep; }
            });
            self.api_composer_env_row(ui);
            if !self.comp_msg.is_empty() { ui.label(RichText::new(&self.comp_msg).small().color(Color32::GRAY)); }
            ui.horizontal(|ui| {
                let np = self.comp_params.iter().filter(|r| r.enabled).count();
                let nh = self.comp_hdr_rows.iter().filter(|r| r.enabled).count();
                ui.selectable_value(&mut self.comp_tab, 0, format!("Params ({np})"));
                ui.selectable_value(&mut self.comp_tab, 1, format!("Headers ({nh})"));
                let nc = crate::composer_more::cookies_from_headers(&self.comp_hdr_rows).len();
                ui.selectable_value(&mut self.comp_tab, 5, format!("Cookies ({nc})"));
                ui.selectable_value(&mut self.comp_tab, 2, if self.ext.api.graphql_mode { "Body (GraphQL)" } else { "Body" });
                let nt = self.ext.api.comp_tests.iter().filter(|a| a.enabled).count();
                ui.selectable_value(&mut self.comp_tab, 3, format!("Tests ({nt})"));
                ui.selectable_value(&mut self.comp_tab, 4, "Code");
            });
            match self.comp_tab {
                0 => { if !self.composer_params_raw(ui) { kv_editor(ui, "comp_params_grid", &mut self.comp_params); } }
                5 => self.composer_cookies(ui),
                1 => { if !self.api_composer_headers(ui) { kv_editor(ui, "comp_headers_grid", &mut self.comp_hdr_rows); } }
                3 => self.api_composer_tests(ui),
                4 => self.api_composer_code(ui),
                _ => self.api_composer_body(ui),
            }
            ui.separator();
            ui.collapsing(format!("Collections ({})", self.comp_collections.len()), |ui| {
                match self.composer_collections_tree(ui) {
                    Some(crate::composer_more::TreeAction::Load(c, n)) => act_load = Some((c, n)),
                    Some(crate::composer_more::TreeAction::Delete(c, n)) => act_del = Some((c, n)),
                    None => {}
                }
            });
            ui.collapsing(format!("History ({})", self.comp_history.len()), |ui| {
                if ui.small_button("Clear history").clicked() { clear_hist = true; }
                egui::ScrollArea::vertical().max_height(120.0).id_salt("comp_hist_scroll").show(ui, |ui| {
                    for (i, h) in self.comp_history.iter().enumerate().take(50) {
                        let st = if h.error.is_empty() { h.status.to_string() } else { "ERR".to_string() };
                        if ui.button(format!("{} {} {}", st, h.request.method, h.request.url)).clicked() { act_hist = Some(i); }
                    }
                });
            });
            ui.separator();
            match &self.comp_resp {
                None => { ui.label(RichText::new("(not sent)").color(Color32::GRAY)); }
                Some(r) if !r.error.is_empty() => { ui.label(RichText::new(format!("Error: {}", r.error)).color(Color32::LIGHT_RED)); self.api_composer_results(ui); }
                Some(r) => {
                    ui.label(RichText::new(format!("{} {}  |  {} ms  |  {} bytes  |  {} headers", r.status, r.reason, r.elapsed_ms, r.body.len(), r.headers.len())).strong());
                    self.api_composer_results(ui);
                    egui::ScrollArea::vertical().max_height(180.0).id_salt("comp_resp_scroll").show(ui, |ui| {
                        for (k, v) in &r.headers { ui.label(RichText::new(format!("{k}: {v}")).monospace().small()); }
                        ui.separator();
                        // laying out megabytes of text takes gigabytes: show the start only
                        const SHOW: usize = 200_000;
                        if r.body.len() > SHOW {
                            let mut cut = SHOW;
                            while !r.body.is_char_boundary(cut) { cut -= 1; }
                            ui.label(RichText::new(format!("Showing the first {} of {} bytes.", cut, r.body.len())).italics().color(Color32::GRAY));
                            ui.label(RichText::new(&r.body[..cut]).monospace());
                        } else {
                            ui.label(RichText::new(&r.body).monospace());
                        }
                    });
                }
            }
        });
        self.show_composer = open;
        if do_save {
            let (c, n) = (self.comp_save_coll.clone(), self.comp_save_name.clone());
            self.comp_msg = match self.comp_save(&c, &n) { Ok(()) => format!("Saved '{}' to '{}'", n.trim(), c.trim()), Err(e) => e };
        }
        if let Some((c, n)) = act_load { let ok = self.comp_load(&c, &n); self.comp_msg = if ok { format!("Loaded '{n}'") } else { format!("Could not load '{n}'") }; }
        if let Some((c, n)) = act_del { let ok = self.comp_delete_request(&c, &n); self.comp_msg = format!("Delete '{n}': {ok}"); }
        if let Some(i) = act_hist { self.comp_load_history(i); }
        if clear_hist { self.comp_history.clear(); let _ = crate::model::save_history(&self.comp_dir, &self.comp_history); }
        if do_send { self.composer_send(ctx); }
    }
}


impl GridApp {
    pub fn rules_persist(&mut self) {
        crate::rules_ext::ensure_ids(&mut self.rules);
        crate::model::set_global_rules(self.rules.clone());
        self.rules_msg = match crate::model::save_rules(&crate::model::rules_dir(), &self.rules) {
            Ok(()) => format!("{} rules saved", self.rules.len()),
            Err(e) => format!("save failed: {e}"),
        };
    }

    pub fn rules_add(&mut self, mut r: crate::model::Rule) {
        if r.id == 0 { r.id = crate::rules_ext::new_rule_id(); }
        self.rules.push(r);
        self.rules_sel = Some(self.rules.len() - 1);
        self.rules_hdr_for = None;
        self.rules_persist();
    }

    pub fn rules_move(&mut self, i: usize, d: i32) {
        let j = i as i32 + d;
        if i < self.rules.len() && j >= 0 && (j as usize) < self.rules.len() {
            self.rules.swap(i, j as usize);
            if self.rules_sel == Some(i) { self.rules_sel = Some(j as usize); }
            self.rules_hdr_for = None;
            self.rules_persist();
        }
    }

    pub fn rules_window(&mut self, ctx: &egui::Context) {
        use crate::rules_ext as rx;
        if !self.show_rules { return; }
        // Hit counters change on the proxy thread: refresh the list now and then.
        ctx.request_repaint_after(std::time::Duration::from_millis(1000));
        let mut open = true;
        let mut persist = false;
        let mut add_new = false;
        let mut add_from_session = false;
        let mut reset_all = false;
        let mut del: Option<usize> = None;
        let mut dup: Option<usize> = None;
        let mut reset_one: Option<u64> = None;
        let mut mv: Option<(usize, i32)> = None;
        let mut group_toggle: Option<(String, bool)> = None;
        let mut collapse_toggle: Option<String> = None;
        let mut sel = self.rules_sel;
        let has_session = self.model.selected_row().is_some();
        let groups = rx::group_names(&self.rules);
        let (mut apply_capture, mut export_set, mut import_set, mut pick_io, mut pick_file) = (false, false, false, false, false);
        let mut pick_folder = false;
        let mut dup_group: Option<String> = None;
        egui::Window::new("Rules (Auto Responder)").open(&mut open).default_width(720.0).vscroll(true).show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Add rule").clicked() { add_new = true; }
                if ui.add_enabled(has_session, egui::Button::new("Add rule from selected session")).clicked() { add_from_session = true; }
                if ui.button("Reset all counters").clicked() { reset_all = true; }
                if ui.button("Apply rules + start capture").on_hover_text("Save and activate the enabled rules, then turn capturing on").clicked() { apply_capture = true; }
                ui.label(&self.rules_msg);
            });
            ui.horizontal(|ui| {
                let st = &mut self.ext.rules;
                if st.io_path.is_empty() {
                    st.io_path = crate::data_io::documents_dir().join("ferrotap-rules.json").display().to_string();
                }
                ui.label("Rule set file");
                ui.add(egui::TextEdit::singleline(&mut st.io_path).desired_width(300.0));
                if ui.small_button("Browse...").clicked() { pick_io = true; }
                if ui.button("Export").on_hover_text("Write every rule to this JSON file").clicked() { export_set = true; }
                if ui.button("Import").on_hover_text("Read rules from this JSON file (or every .json in this folder)").clicked() { import_set = true; }
                ui.checkbox(&mut st.io_replace, "replace current rules");
            });
            ui.label("Rules run in order: answering actions (canned, Map Local, redirect, block, close) end the list; pass-through rules (modify, Map Remote) combine unless 'stop' is set. Response rules run on the server's answer.");
            ui.separator();
            for g in &groups {
                // A collapsed parent hides its sub-groups too.
                if self.ext.rules.collapsed.iter().any(|c| c != g && rx::in_group(g, c)) { continue; }
                let collapsed = self.ext.rules.collapsed.contains(g);
                let indent = if g.is_empty() { 0.0 } else { 18.0 * (rx::group_depth(g) as f32 + 1.0) };
                if !g.is_empty() {
                    let mut on = rx::group_enabled(&self.rules, g);
                    ui.horizontal(|ui| {
                        ui.add_space(indent - 18.0);
                        if ui.small_button(if collapsed { "+" } else { "-" }).clicked() { collapse_toggle = Some(g.clone()); }
                        if ui.checkbox(&mut on, "").changed() { group_toggle = Some((g.clone(), on)); }
                        let n = self.rules.iter().filter(|r| rx::in_group(&r.group, g)).count();
                        let leaf = g.rsplit('/').next().unwrap_or(g);
                        ui.strong(format!("Group: {leaf} ({n})")).on_hover_text(g.as_str());
                        if ui.small_button("Duplicate group").clicked() { dup_group = Some(g.clone()); }
                    });
                    if collapsed { continue; }
                }
                for (i, r) in self.rules.iter_mut().enumerate().filter(|(_, r)| &r.group == g) {
                    ui.horizontal(|ui| {
                        if !g.is_empty() { ui.add_space(indent); }
                        if ui.checkbox(&mut r.enabled, "").changed() { persist = true; }
                        let label = format!("{}. {} {}", i + 1, r.name, rx::rule_summary(r));
                        let lab = if r.group_off { egui::RichText::new(label).weak() } else { egui::RichText::new(label) };
                        if ui.selectable_label(sel == Some(i), lab).clicked() { sel = Some(i); }
                        ui.label(format!("hits: {}", rx::rule_hits(r.id)));
                        if ui.small_button("Reset").on_hover_text("Reset this rule's match counter").clicked() { reset_one = Some(r.id); }
                        if ui.small_button("Up").clicked() { mv = Some((i, -1)); }
                        if ui.small_button("Down").clicked() { mv = Some((i, 1)); }
                        if ui.small_button("Duplicate").clicked() { dup = Some(i); }
                        if ui.small_button("Delete").clicked() { del = Some(i); }
                    });
                }
            }
            if let Some(i) = sel {
                if i < self.rules.len() {
                    ui.separator();
                    if self.rules_hdr_for != Some(i) {
                        self.rules_hdr_text = self.rules[i].headers.iter().map(|h| format!("{}: {}", h.key, h.value)).collect::<Vec<_>>().join("\n");
                        self.rules_hdr_for = Some(i);
                    }
                    let st = &mut self.ext.rules;
                    if st.edit_for != Some(self.rules[i].id) {
                        let r = &self.rules[i];
                        st.req_hdr_text = rx::header_edits_to_text(&r.req_headers);
                        st.resp_hdr_text = rx::header_edits_to_text(&r.resp_headers);
                        st.req_body_text = rx::replaces_to_text(&r.req_body);
                        st.resp_body_text = rx::replaces_to_text(&r.resp_body);
                        st.query_text = crate::rules_more::kv_edits_to_text(&r.more.query);
                        st.cookie_text = crate::rules_more::kv_edits_to_text(&r.more.cookies);
                        st.edit_for = Some(r.id);
                    }
                    let r = &mut self.rules[i];
                    ui.horizontal(|ui| {
                        ui.label("Name");
                        if ui.text_edit_singleline(&mut r.name).changed() { persist = true; }
                        ui.label("Group");
                        if ui.add(egui::TextEdit::singleline(&mut r.group).hint_text("parent/child").desired_width(120.0)).changed() { persist = true; }
                        egui::ComboBox::from_id_salt("rule_move_group").selected_text("Move to...").show_ui(ui, |ui| {
                            for g in &groups {
                                let label = if g.is_empty() { "(no group)".to_string() } else { g.clone() };
                                if ui.selectable_label(&r.group == g, label).clicked() { r.group = g.clone(); persist = true; }
                            }
                        });
                    });
                    ui.horizontal(|ui| {
                        ui.label("Stage");
                        let cur = if r.stage == "response" { "response" } else { "request" };
                        egui::ComboBox::from_id_salt("rule_stage").selected_text(cur).show_ui(ui, |ui| {
                            for (k, l) in [("", "request"), ("response", "response")] {
                                if ui.selectable_value(&mut r.stage, k.to_string(), l).changed() { persist = true; }
                            }
                        });
                        egui::ComboBox::from_id_salt("rule_kind").selected_text(r.match_kind.clone()).show_ui(ui, |ui| {
                            for k in ["contains", "glob", "regex"] {
                                if ui.selectable_value(&mut r.match_kind, k.to_string(), k).changed() { persist = true; }
                            }
                        });
                        ui.label("URL").on_hover_text("contains: substring; glob: * and ? wildcards (whole URL); regex: full regular expression");
                        if ui.text_edit_singleline(&mut r.pattern).changed() { persist = true; }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Method (blank = any)");
                        if ui.add(egui::TextEdit::singleline(&mut r.method).desired_width(80.0)).changed() { persist = true; }
                        ui.label("Host (*.domain, =exact, or part)");
                        if ui.text_edit_singleline(&mut r.host).changed() { persist = true; }
                    });
                    ui.label("Conditions (all must hold; status / resp_* need a response rule; status value: 404, 4xx, 500-599):");
                    let mut rm_cond: Option<usize> = None;
                    for (ci, c) in r.conds.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            if ui.checkbox(&mut c.enabled, "").changed() { persist = true; }
                            egui::ComboBox::from_id_salt(("rule_cond_kind", ci)).selected_text(c.kind.clone()).width(90.0).show_ui(ui, |ui| {
                                for k in rx::COND_KINDS {
                                    if ui.selectable_value(&mut c.kind, k.to_string(), *k).changed() { persist = true; }
                                }
                            });
                            if matches!(c.kind.as_str(), "header" | "cookie" | "resp_header")
                                && ui.add(egui::TextEdit::singleline(&mut c.name).hint_text("name").desired_width(110.0)).changed() { persist = true; }
                            if c.kind != "status" {
                                let cur = if c.op.is_empty() { "contains".to_string() } else { c.op.clone() };
                                egui::ComboBox::from_id_salt(("rule_cond_op", ci)).selected_text(cur).width(90.0).show_ui(ui, |ui| {
                                    for k in rx::COND_OPS {
                                        if ui.selectable_value(&mut c.op, k.to_string(), *k).changed() { persist = true; }
                                    }
                                });
                            }
                            if c.op != "exists"
                                && ui.add(egui::TextEdit::singleline(&mut c.value).hint_text("value").desired_width(180.0)).changed() { persist = true; }
                            if ui.small_button("Remove").clicked() { rm_cond = Some(ci); }
                        });
                    }
                    if let Some(ci) = rm_cond { r.conds.remove(ci); persist = true; }
                    ui.horizontal(|ui| {
                        if ui.small_button("Add condition").clicked() {
                            r.conds.push(rx::RuleCond { enabled: true, kind: "header".into(), op: "contains".into(), ..Default::default() });
                            persist = true;
                        }
                    });
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label("Action");
                        egui::ComboBox::from_id_salt("rule_action").selected_text(rx::action_label(&r.action)).show_ui(ui, |ui| {
                            for (k, l) in rx::ACTIONS {
                                if ui.selectable_value(&mut r.action, k.to_string(), *l).changed() {
                                    if r.action == "redirect" && !(300..400).contains(&r.status) { r.status = 302; }
                                    persist = true;
                                }
                            }
                        });
                        ui.label("Delay (ms)");
                        if ui.add(egui::DragValue::new(&mut r.delay_ms).range(0..=600_000)).changed() { persist = true; }
                        if !rx::is_terminal(&r.action)
                            && ui.checkbox(&mut r.stop, "stop processing more rules").changed() { persist = true; }
                    });
                    match r.action.as_str() {
                        "canned" | "file" => {
                            ui.horizontal(|ui| {
                                ui.label("Status");
                                if ui.add(egui::DragValue::new(&mut r.status).range(100..=599)).changed() { persist = true; }
                                if r.action == "canned" {
                                    let mut pick: Option<&str> = None;
                                    egui::ComboBox::from_id_salt("rule_template").selected_text("Template...").show_ui(ui, |ui| {
                                        for t in rx::templates() {
                                            if ui.selectable_label(false, t.0).clicked() { pick = Some(t.0); }
                                        }
                                    });
                                    if let Some(name) = pick {
                                        rx::apply_template(r, name);
                                        self.rules_hdr_text = r.headers.iter().map(|h| format!("{}: {}", h.key, h.value)).collect::<Vec<_>>().join("\n");
                                        persist = true;
                                    }
                                }
                            });
                            if r.action == "file" {
                                ui.horizontal(|ui| {
                                    ui.label("File (Content-Type from extension unless set below)");
                                    if ui.text_edit_singleline(&mut r.file).changed() { persist = true; }
                                    if ui.small_button("Browse...").clicked() { pick_file = true; }
                                });
                            }
                            ui.label("Headers (Key: Value per line)");
                            if ui.text_edit_multiline(&mut self.rules_hdr_text).changed() {
                                r.headers = self.rules_hdr_text.lines().filter_map(|l| l.split_once(':')).map(|(k, v)| crate::model::KvRow { enabled: true, key: k.trim().to_string(), value: v.trim().to_string() }).collect();
                                persist = true;
                            }
                            if r.action == "canned" {
                                ui.label("Body");
                                if ui.text_edit_multiline(&mut r.body).changed() { persist = true; }
                            }
                        }
                        "redirect" => {
                            ui.horizontal(|ui| {
                                ui.label("Status");
                                for s in [301u16, 302, 307, 308] {
                                    if ui.selectable_label(r.status == s, s.to_string()).clicked() { r.status = s; persist = true; }
                                }
                                ui.label("Location");
                                if ui.text_edit_singleline(&mut r.target).changed() { persist = true; }
                            });
                        }
                        "block" => { ui.label("Answers 403 Forbidden without contacting the server."); }
                        "allow_only" => { ui.label("While any allow-list rule is on, requests that match none of them get 403 Forbidden; matching requests continue to the other rules."); }
                        "folder" => {
                            ui.horizontal(|ui| {
                                ui.label("Folder");
                                if ui.text_edit_singleline(&mut r.file).changed() { persist = true; }
                                if ui.small_button("Browse...").clicked() { pick_folder = true; }
                            });
                            ui.horizontal(|ui| {
                                ui.label("URL base (optional)");
                                if ui.add(egui::TextEdit::singleline(&mut r.target).hint_text("https://example.com/static/").desired_width(300.0)).changed() { persist = true; }
                            });
                            ui.label("The URL path after the base picks the file in the folder; a folder or '/' serves index.html; missing files get 404.");
                        }
                        "close" => { ui.label("Closes the client connection without an answer."); }
                        _ => {
                            if r.action == "map_remote" {
                                ui.horizontal(|ui| {
                                    ui.label("Send to");
                                    if ui.text_edit_singleline(&mut r.target).changed() { persist = true; }
                                });
                                ui.label("http://host:port keeps the path; a full URL replaces it; a regex rule may use $1 in the target.");
                            }
                            let st = &mut self.ext.rules;
                            ui.label("Header edits, one per line: 'set Name: value', 'add Name: value', 'remove Name'");
                            if r.stage != "response" {
                                ui.label("Request headers");
                                if ui.add(egui::TextEdit::multiline(&mut st.req_hdr_text).desired_rows(2)).changed() { r.req_headers = rx::header_edits_from_text(&st.req_hdr_text); persist = true; }
                            }
                            ui.label("Response headers");
                            if ui.add(egui::TextEdit::multiline(&mut st.resp_hdr_text).desired_rows(2)).changed() { r.resp_headers = rx::header_edits_from_text(&st.resp_hdr_text); persist = true; }
                            ui.label("Body replacements, one per line: 'find => replace' (prefix 're:' for a regex)");
                            if r.stage != "response" {
                                ui.label("Request body");
                                if ui.add(egui::TextEdit::multiline(&mut st.req_body_text).desired_rows(2)).changed() { r.req_body = rx::replaces_from_text(&st.req_body_text); persist = true; }
                            }
                            ui.label("Response body");
                            if ui.add(egui::TextEdit::multiline(&mut st.resp_body_text).desired_rows(2)).changed() { r.resp_body = rx::replaces_from_text(&st.resp_body_text); persist = true; }
                            ui.collapsing("More: query, cookies, status, user-agent, caching, throttling", |ui| {
                                let m = &mut r.more;
                                if r.stage != "response" {
                                    ui.label("Query parameters, one per line: 'set name=value', 'add name=value', 'remove name'");
                                    if ui.add(egui::TextEdit::multiline(&mut st.query_text).desired_rows(2)).changed() { m.query = crate::rules_more::kv_edits_from_text(&st.query_text); persist = true; }
                                    ui.label("Request cookies: 'set name=value', 'remove name'");
                                    if ui.add(egui::TextEdit::multiline(&mut st.cookie_text).desired_rows(2)).changed() { m.cookies = crate::rules_more::kv_edits_from_text(&st.cookie_text); persist = true; }
                                    ui.horizontal(|ui| {
                                        ui.label("User-Agent");
                                        if ui.add(egui::TextEdit::singleline(&mut m.user_agent).hint_text("keep").desired_width(260.0)).changed() { persist = true; }
                                        egui::ComboBox::from_id_salt("rule_ua").selected_text("Preset...").show_ui(ui, |ui| {
                                            for (n, ua) in crate::rules_more::USER_AGENTS {
                                                if ui.selectable_label(false, *n).clicked() { m.user_agent = ua.to_string(); persist = true; }
                                            }
                                        });
                                    });
                                }
                                ui.horizontal(|ui| {
                                    ui.label("Response status (0 = keep)");
                                    if ui.add(egui::DragValue::new(&mut m.status).range(0..=599)).changed() { persist = true; }
                                    if ui.checkbox(&mut m.no_cache, "Disable caching").changed() { persist = true; }
                                    if ui.checkbox(&mut m.no_cookies, "Disable cookies").changed() { persist = true; }
                                });
                                ui.horizontal(|ui| {
                                    ui.label("Throttle");
                                    egui::ComboBox::from_id_salt("rule_throttle").selected_text("Preset...").show_ui(ui, |ui| {
                                        for (n, lat, k, loss) in crate::rules_more::THROTTLE_PRESETS {
                                            if ui.selectable_label(false, *n).clicked() { r.delay_ms = *lat; m.kbps = *k; m.loss_pct = *loss; persist = true; }
                                        }
                                    });
                                    ui.label("kbit/s (0 = no limit)");
                                    if ui.add(egui::DragValue::new(&mut m.kbps).range(0..=1_000_000)).changed() { persist = true; }
                                    ui.label("loss %");
                                    if ui.add(egui::DragValue::new(&mut m.loss_pct).range(0..=100)).changed() { persist = true; }
                                });
                                ui.label("Latency is the Delay above. Throttle a host by setting this rule's Host; use a broad URL pattern for everything.");
                            });
                        }
                    }
                }
            }
        });
        self.show_rules = open;
        if sel != self.rules_sel { self.rules_hdr_for = None; }
        self.rules_sel = sel;
        if add_new {
            let mut r = crate::model::Rule::default();
            r.name = format!("rule{}", self.rules.len() + 1);
            r.pattern = "example.com".into();
            self.rules_add(r);
        }
        if add_from_session {
            if let Some(s) = self.model.selected_row() {
                let r = rx::rule_from_session(s);
                self.rules_add(r);
            }
        }
        if reset_all { rx::reset_hits_for(&self.rules); }
        if let Some(id) = reset_one { rx::reset_hits(id); }
        if let Some((g, on)) = group_toggle { rx::set_group_enabled(&mut self.rules, &g, on); persist = true; }
        if let Some(g) = dup_group {
            let copies = rx::duplicate_group(&self.rules, &g);
            let n = copies.len();
            let at = self.rules.iter().rposition(|r| rx::in_group(&r.group, &g)).map(|i| i + 1).unwrap_or(self.rules.len());
            for (k, c) in copies.into_iter().enumerate() { self.rules.insert(at + k, c); }
            self.rules_sel = None;
            self.rules_hdr_for = None;
            self.rules_msg = format!("duplicated group '{g}' ({n} rules)");
            persist = true;
        }
        if let Some(g) = collapse_toggle {
            let c = &mut self.ext.rules.collapsed;
            if let Some(p) = c.iter().position(|x| x == &g) { c.remove(p); } else { c.push(g); }
        }
        if let Some(i) = dup {
            if i < self.rules.len() {
                let c = rx::duplicate_rule(&self.rules[i]);
                self.rules.insert(i + 1, c);
                self.rules_sel = Some(i + 1);
                self.rules_hdr_for = None;
                persist = true;
            }
        }
        if let Some(i) = del { if i < self.rules.len() { self.rules.remove(i); } self.rules_sel = None; self.rules_hdr_for = None; persist = true; }
        if let Some((i, d)) = mv { self.rules_move(i, d); }
        // Rule set files and the file picker.
        let io_path = std::path::PathBuf::from(self.ext.rules.io_path.trim().trim_matches('"'));
        if pick_io { self.ext.pick.open_file("rules_io", "Choose a rule set file", &self.ext.rules.io_path, &["json", "farx"]); }
        if pick_file {
            let start = self.rules_sel.and_then(|i| self.rules.get(i)).map(|r| r.file.clone()).unwrap_or_default();
            self.ext.pick.open_file("rule_file", "Choose the response file", &start, &[]);
        }
        if pick_folder {
            let start = self.rules_sel.and_then(|i| self.rules.get(i)).map(|r| r.file.clone()).unwrap_or_default();
            self.ext.pick.open_folder("rule_folder", "Choose the folder to serve", &start);
        }
        if let Some(p) = self.ext.pick.take("rule_folder") {
            if let Some(r) = self.rules_sel.and_then(|i| self.rules.get_mut(i)) { r.file = p.display().to_string(); persist = true; }
        }
        if let Some(p) = self.ext.pick.take("rules_io") { self.ext.rules.io_path = p.display().to_string(); }
        if let Some(p) = self.ext.pick.take("rule_file") {
            if let Some(r) = self.rules_sel.and_then(|i| self.rules.get_mut(i)) { r.file = p.display().to_string(); persist = true; }
        }
        if export_set {
            self.rules_msg = match rx::export_rule_set(&io_path, &self.rules) {
                Ok(n) => format!("exported {n} rules to {}", io_path.display()),
                Err(e) => format!("export failed: {e}"),
            };
        }
        if import_set {
            match rx::import_rule_set(&io_path) {
                Ok(list) => {
                    let n = list.len();
                    if self.ext.rules.io_replace { self.rules.clear(); }
                    self.rules.extend(list);
                    self.rules_sel = None;
                    self.rules_hdr_for = None;
                    self.rules_persist();
                    self.rules_msg = format!("imported {n} rules ({} total)", self.rules.len());
                }
                Err(e) => self.rules_msg = format!("import failed: {e}"),
            }
        }
        if apply_capture {
            self.rules_persist();
            self.capturing = true;
            let on = self.rules.iter().filter(|r| r.enabled && !r.group_off).count();
            self.rules_msg = format!("{on} rules active, capturing");
        }
        if persist { self.rules_persist(); }
    }

    // ---------------- Breakpoints UI ----------------
    pub fn breakpoints_ui(&mut self, ctx: &egui::Context) {
        let mut open = true;
        let mut persist = false;
        let mut add_new = false;
        let mut del = None;
        let hub = crate::model::breakpoint_hub();
        let paused_list = hub.list();

        egui::Window::new("Breakpoints & Active Pauses").open(&mut open).default_width(700.0).show(ctx, |ui| {
            ui.horizontal(|ui|
            {
                ui.heading("Breakpoints");
                if ui.button("Add Breakpoint").clicked() {
                    add_new = true;
                }
                if ui.button("Save Rules").clicked() {
                    persist = true;
                    self.bp_msg = "Breakpoints saved.".to_string();
                }
                ui.label(&self.bp_msg);
            });
            ui.label("Pause matching in-flight requests or responses before upstream delivery or client receipt.");
            ui.separator();

            // Rules table/list
            ui.collapsing("Configured Rules", |ui| {
                for (i, r) in self.bp_rules.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut r.enabled, "").changed() { persist = true; }
                        ui.checkbox(&mut r.on_request, "Req");
                        ui.checkbox(&mut r.on_response, "Resp");
                        let meth = if r.method.is_empty() { "ANY" } else { &r.method };
                        ui.label(format!("[{}|{}]", meth, if r.match_kind.is_empty() { "contains" } else { r.match_kind.as_str() }));
                        let pat = if r.pattern.is_empty() { "* (any URL)" } else { &r.pattern };
                        let is_sel = self.bp_rules_sel == Some(i);
                        if ui.selectable_label(is_sel, pat).clicked() {
                            self.bp_rules_sel = if is_sel { None } else { Some(i) };
                        }
                        if ui.button("X").clicked() { del = Some(i); }
                    });
                }
            });

            // Rule editor if selected
            if let Some(i) = self.bp_rules_sel {
                if i < self.bp_rules.len() {
                    let r = &mut self.bp_rules[i];
                    ui.separator();
                    ui.label(format!("Editing Breakpoint #{}", i + 1));
                    ui.horizontal(|ui| {
                        ui.label("Match type:");
                        let cur = if r.match_kind.is_empty() { "contains".to_string() } else { r.match_kind.clone() };
                        egui::ComboBox::from_id_salt("bp_match_kind").selected_text(cur).show_ui(ui, |ui| {
                            for k in ["contains", "regex", "dsl"] {
                                if ui.selectable_value(&mut r.match_kind, k.to_string(), k).changed() { persist = true; }
                            }
                        });
                        if ui.checkbox(&mut r.enabled, "Enabled").changed() { persist = true; }
                    });
                    if r.match_kind == "dsl" {
                        ui.label(RichText::new("DSL: ~m METHOD  ~u url  ~d host  ~h header  ~t content-type  ~b body  ~c status (404, 5xx)  ~q request  ~s response  ! & | ( )").weak());
                        match crate::bpdsl::parse(&r.pattern) {
                            Ok(_) => { ui.colored_label(Color32::from_rgb(80, 180, 80), "\u{2713} filter parses OK"); }
                            Err(e) => { ui.colored_label(Color32::from_rgb(220, 80, 80), format!("\u{2717} {}", e)); }
                        }
                        let id = egui::Id::new(("bp_test", i));
                        let mut tm = ui.data_mut(|d| d.get_temp::<String>(id.with("m"))).unwrap_or_else(|| "GET".to_string());
                        let mut turl = ui.data_mut(|d| d.get_temp::<String>(id)).unwrap_or_else(|| "https://example.com/api/users".to_string());
                        let mut tst = ui.data_mut(|d| d.get_temp::<String>(id.with("s"))).unwrap_or_else(|| "200".to_string());
                        ui.horizontal(|ui| {
                            ui.label("Try it - method:");
                            ui.add(egui::TextEdit::singleline(&mut tm).desired_width(60.0));
                            ui.label("URL:");
                            ui.text_edit_singleline(&mut turl);
                            ui.label("status:");
                            ui.add(egui::TextEdit::singleline(&mut tst).desired_width(50.0));
                        });
                        ui.data_mut(|d| { d.insert_temp(id.with("m"), tm.clone()); d.insert_temp(id, turl.clone()); d.insert_temp(id.with("s"), tst.clone()); });
                        let st = tst.trim().parse::<u16>().unwrap_or(200);
                        let yn = |b: bool| if b { "PAUSES" } else { "no pause" };
                        // The preview has no sample headers/body, so ~h/~b show as no-match here.
                        ui.label(format!("Request: {}   |   Response: {}", yn(r.matches_request(&tm, &turl, &[], "")), yn(r.matches_response(&tm, &turl, st, &[], ""))));
                    }
                    ui.horizontal(|ui| {
                        ui.label("Name:");
                        ui.text_edit_singleline(&mut r.name);
                    });
                    ui.horizontal(|ui| {
                        ui.label("Method:");
                        ui.text_edit_singleline(&mut r.method);
                        ui.label("Pattern:");
                        if ui.text_edit_singleline(&mut r.pattern).changed() { persist = true; }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Status filter:");
                        ui.text_edit_singleline(&mut r.status);
                        ui.checkbox(&mut r.on_request, "Pause Request");
                        ui.checkbox(&mut r.on_response, "Pause Response");
                    });
                }
            }

            ui.separator();
            ui.horizontal(|ui| {
                ui.heading(format!("Active Paused Sessions ({})", paused_list.len()));
                if ui.add_enabled(!paused_list.is_empty(), egui::Button::new("Resume all")).on_hover_text("Let every paused request/response continue unchanged").clicked() {
                    hub.resume_all();
                    self.bp_active_id = None;
                }
                if ui.add_enabled(!paused_list.is_empty(), egui::Button::new("Abort all")).clicked() {
                    for p in &paused_list { hub.abort(p.id); }
                    self.bp_active_id = None;
                }
            });

            if paused_list.is_empty() {
                ui.label("No sessions currently paused at a breakpoint.");
            } else {
                for p in &paused_list {
                    ui.group(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(format!("#{} [Stage: {:?}] {} {}", p.id, p.stage, p.method, p.url));
                            if p.status > 0 {
                                ui.label(format!("Status: {}", p.status));
                            }
                        });
                        ui.horizontal(|ui| {
                            if ui.button("Resume (Unmodified)").clicked() {
                                hub.resume(p.id, crate::model::BpEdit::default());
                            }
                            if ui.button("Abort / Drop").clicked() {
                                hub.abort(p.id);
                            }
                            if self.bp_active_id == Some(p.id) {
                                if ui.button("Close Inspector").clicked() {
                                    self.bp_active_id = None;
                                }
                            } else if ui.button("Inspect & Modify").clicked() {
                                self.bp_active_id = Some(p.id);
                                self.bp_edit_url = p.url.clone();
                                self.bp_edit_method = p.method.clone();
                                self.bp_edit_status = if p.status > 0 { p.status.to_string() } else { "200".to_string() };
                                self.bp_edit_headers = p.headers.iter().map(|(k, v)| format!("{}: {}\n", k, v)).collect();
                                self.bp_edit_body = String::from_utf8_lossy(&p.body).to_string();
                            }
                        });
                    });
                }
            }

            // Modification subpanel for paused session
            if let Some(active_id) = self.bp_active_id {
                if let Some(target) = paused_list.iter().find(|p| p.id == active_id) {
                    ui.separator();
                    ui.heading(format!("Modify Paused Session #{}", active_id));
                    ui.horizontal(|ui| {
                        ui.label("Method:");
                        ui.text_edit_singleline(&mut self.bp_edit_method);
                        ui.label("URL:");
                        ui.text_edit_singleline(&mut self.bp_edit_url);
                    });
                    if target.stage == crate::model::BpStage::Response {
                        ui.horizontal(|ui| {
                            ui.label("Status Code:");
                            ui.text_edit_singleline(&mut self.bp_edit_status);
                        });
                    }
                    ui.label("Headers (Header: Value per line):");
                    ui.text_edit_multiline(&mut self.bp_edit_headers);
                    ui.label("Body:");
                    ui.text_edit_multiline(&mut self.bp_edit_body);

                    ui.horizontal(|ui| {
                        if ui.button("Execute Changes & Resume").clicked() {
                            let mut hdrs = Vec::new();
                            for line in self.bp_edit_headers.lines() {
                                if let Some((k, v)) = line.split_once(':') {
                                    hdrs.push((k.trim().to_string(), v.trim().to_string()));
                                }
                            }
                            let st_opt = self.bp_edit_status.trim().parse::<u16>().ok();
                            let edits = crate::model::BpEdit {
                                url: if self.bp_edit_url != target.url { Some(self.bp_edit_url.clone()) } else { None },
                                method: if self.bp_edit_method != target.method { Some(self.bp_edit_method.clone()) } else { None },
                                status: st_opt,
                                headers: if !hdrs.is_empty() { Some(hdrs) } else { None },
                                body: Some(self.bp_edit_body.as_bytes().to_vec()),
                            };
                            hub.resume(active_id, edits);
                            self.bp_active_id = None;
                        }
                        if ui.button("Abort Session").clicked() {
                            hub.abort(active_id);
                            self.bp_active_id = None;
                        }
                    });
                } else {
                    self.bp_active_id = None;
                }
            }
        });

        if !open { self.show_breakpoints = false; }
        if add_new {
            let mut r = crate::model::BreakpointRule::default();
            r.name = format!("Rule {}", self.bp_rules.len() + 1);
            self.bp_rules.push(r);
            self.bp_rules_sel = Some(self.bp_rules.len() - 1);
            persist = true;
        }
        if let Some(i) = del {
            if i < self.bp_rules.len() {
                self.bp_rules.remove(i);
            }
            self.bp_rules_sel = None;
            persist = true;
        }
        if persist {
            crate::model::set_global_breakpoints(self.bp_rules.clone());
        }
    }

}

/// Kinds offered by "Add column…": (label, hint for the name/pattern box).
pub const CUSTOM_SOURCE_KINDS: [(&str, &str); 7] = [
    ("Request header", "header name"),
    ("Response header", "header name"),
    ("Cookie", "cookie name"),
    ("Query parameter", "parameter name"),
    ("Response JSON path", "e.g. user.id or items.0.name"),
    ("Body text (regex)", "regex; first group is shown"),
    ("Script / plugin", "script name (its column(s) function)"),
];

/// The column source for an "Add column…" kind (index into CUSTOM_SOURCE_KINDS).
pub fn custom_source(kind: usize, arg: &str) -> ColumnSource {
    let a = arg.to_string();
    match kind {
        0 => ColumnSource::ReqHeader(a),
        1 => ColumnSource::ResHeader(a),
        2 => ColumnSource::Cookie(a),
        3 => ColumnSource::QueryParam(a),
        4 => ColumnSource::JsonPath(a),
        6 => ColumnSource::Script(a),
        _ => ColumnSource::BodyRegex(a),
    }
}

fn custom_source_label(s: &ColumnSource) -> String {
    match s {
        ColumnSource::Header(n) => format!("header {n}"),
        ColumnSource::ReqHeader(n) => format!("request header {n}"),
        ColumnSource::ResHeader(n) => format!("response header {n}"),
        ColumnSource::Cookie(n) => format!("cookie {n}"),
        ColumnSource::QueryParam(n) => format!("query {n}"),
        ColumnSource::JsonPath(n) => format!("JSON {n}"),
        ColumnSource::BodyRegex(n) => format!("body /{n}/"),
        ColumnSource::Script(n) => format!("script {n}"),
        other => format!("{other:?}"),
    }
}
