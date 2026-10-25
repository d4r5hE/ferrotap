//! G10 settings and quality of life: settings screen, theme, keyboard shortcuts dialog, notifications, status bar, home screen.
//! Owned by one builder; the app calls `qol_menu` from the toolbar, `qol_windows` every frame and `qol_frame` +
//! `qol_status_bar` from the bottom status panel.
//!
//! The pure parts (settings file, `sanitize`, the shortcut table, theme mapping, status text) are usable without the UI,
//! e.g. exports can call `sanitize(&rec, &qol::privacy_settings())`.
use crate::model::SessionRecord;
use crate::ui::GridApp;
use eframe::egui::{self, Color32, Key, RichText};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------- settings file

/// Light / dark / follow the OS.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ThemeChoice {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeChoice {
    pub const ALL: [ThemeChoice; 3] = [ThemeChoice::System, ThemeChoice::Light, ThemeChoice::Dark];

    pub fn label(self) -> &'static str {
        match self {
            ThemeChoice::System => "System (follow the OS)",
            ThemeChoice::Light => "Light",
            ThemeChoice::Dark => "Dark",
        }
    }

    /// The egui preference; `System` makes egui follow the theme the OS reports for the window.
    pub fn preference(self) -> egui::ThemePreference {
        match self {
            ThemeChoice::System => egui::ThemePreference::System,
            ThemeChoice::Light => egui::ThemePreference::Light,
            ThemeChoice::Dark => egui::ThemePreference::Dark,
        }
    }

    /// The theme actually used, given what the OS reports (None = unknown, which egui treats as dark).
    pub fn resolve(self, system: Option<egui::Theme>) -> egui::Theme {
        match self {
            ThemeChoice::Light => egui::Theme::Light,
            ThemeChoice::Dark => egui::Theme::Dark,
            ThemeChoice::System => system.unwrap_or(egui::Theme::Dark),
        }
    }
}

/// What gets masked by `sanitize` (names are matched case-insensitively).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct PrivacySettings {
    pub headers: Vec<String>,
    pub cookies: Vec<String>,
    pub query_params: Vec<String>,
    /// Regular expressions applied to request and response bodies.
    pub body_regexes: Vec<String>,
    pub replacement: String,
}

impl Default for PrivacySettings {
    fn default() -> Self {
        PrivacySettings {
            headers: vec!["Authorization".into(), "Proxy-Authorization".into(), "X-Api-Key".into()],
            cookies: Vec::new(),
            query_params: vec!["access_token".into(), "api_key".into(), "token".into()],
            body_regexes: Vec::new(),
            replacement: "[REDACTED]".into(),
        }
    }
}

/// Which events show an in-app toast.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct NotifySettings {
    pub capture: bool,
    pub rule_hit: bool,
    pub breakpoint_hit: bool,
    pub errors: bool,
}

impl Default for NotifySettings {
    fn default() -> Self {
        NotifySettings { capture: true, rule_hit: false, breakpoint_hit: true, errors: false }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemeChoice,
    /// Show the home screen while there are no sessions.
    pub show_home: bool,
    pub privacy: PrivacySettings,
    pub notify: NotifySettings,
    /// Toolbar buttons the user hid (ids from TOOLBAR_ITEMS).
    pub hidden_toolbar: Vec<String>,
}

/// Toolbar buttons that can be shown / hidden: (id, label).
pub const TOOLBAR_ITEMS: &[(&str, &str)] = &[
    ("capture", "Capture on/off"),
    ("clear", "Clear menu"),
    ("autoscroll", "Auto-scroll"),
    ("sysproxy", "System proxy"),
    ("browser", "Open Clean Browser"),
    ("https", "HTTPS Settings"),
    ("composer", "Composer"),
    ("rules", "Rules"),
    ("breakpoints", "Breakpoints"),
    ("terminal", "Open Terminal"),
    ("connections", "Connections"),
    ("intercept", "Intercept"),
];

impl Settings {
    /// Is this toolbar button shown?
    pub fn toolbar_shows(&self, id: &str) -> bool {
        !self.hidden_toolbar.iter().any(|h| h == id)
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings { theme: ThemeChoice::System, show_home: true, privacy: PrivacySettings::default(), notify: NotifySettings::default(), hidden_toolbar: Vec::new() }
    }
}

impl Settings {
    /// `FERROTAP_SETTINGS` overrides the location (tests use a temp dir).
    pub fn default_path() -> PathBuf {
        std::env::var("FERROTAP_SETTINGS").map(PathBuf::from).unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data").join("settings.json")
        })
    }

    /// Missing or unreadable file -> defaults.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        let j = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, j).map_err(|e| e.to_string())
    }
}

/// The saved privacy settings, for other features (exports) to pass to `sanitize`.
pub fn privacy_settings() -> PrivacySettings {
    Settings::load(&Settings::default_path()).privacy
}

// ---------------------------------------------------------------- sanitize

fn name_in(list: &[String], name: &str) -> bool {
    let n = name.trim();
    !n.is_empty() && list.iter().any(|x| x.trim().eq_ignore_ascii_case(n))
}

/// Mask the values of the named query parameters in a URL (the fragment is kept as is).
pub fn mask_query(url: &str, params: &[String], rep: &str) -> String {
    if params.is_empty() {
        return url.to_string();
    }
    let (main, frag) = match url.find('#') {
        Some(i) => (&url[..i], &url[i..]),
        None => (url, ""),
    };
    let Some(q) = main.find('?') else { return url.to_string() };
    let (base, query) = (&main[..q], &main[q + 1..]);
    let parts: Vec<String> = query
        .split('&')
        .map(|p| match p.split_once('=') {
            Some((k, _)) if name_in(params, k) => format!("{k}={rep}"),
            _ => p.to_string(),
        })
        .collect();
    format!("{base}?{}{frag}", parts.join("&"))
}

/// `Cookie: a=1; b=2` with the named cookies' values masked.
fn mask_cookie_header(v: &str, names: &[String], rep: &str) -> String {
    v.split(';')
        .map(|p| match p.split_once('=') {
            Some((k, _)) if name_in(names, k) => format!("{k}={rep}"),
            _ => p.to_string(),
        })
        .collect::<Vec<_>>()
        .join(";")
}

/// `Set-Cookie` (one cookie per line) with the named cookies' values masked; attributes are kept.
fn mask_set_cookie(v: &str, names: &[String], rep: &str) -> String {
    v.split('\n')
        .map(|line| {
            let (first, rest) = match line.find(';') {
                Some(i) => (&line[..i], &line[i..]),
                None => (line, ""),
            };
            match first.split_once('=') {
                Some((k, _)) if name_in(names, k) => format!("{k}={rep}{rest}"),
                _ => line.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Regexes that fail to compile, with the error (shown in the settings window).
pub fn invalid_regexes(p: &PrivacySettings) -> Vec<(String, String)> {
    p.body_regexes
        .iter()
        .filter(|r| !r.trim().is_empty())
        .filter_map(|r| regex::Regex::new(r).err().map(|e| (r.clone(), e.to_string())))
        .collect()
}

/// A copy of the session with the chosen headers, cookies, query parameters and body matches replaced by
/// `p.replacement`. Pure: the input is not changed. Invalid regexes are skipped.
pub fn sanitize(rec: &SessionRecord, p: &PrivacySettings) -> SessionRecord {
    let mut r = rec.clone();
    let rep = p.replacement.as_str();
    for headers in [&mut r.request_headers, &mut r.response_headers] {
        for (k, v) in headers.iter_mut() {
            if name_in(&p.headers, k) {
                *v = rep.to_string();
            } else if k.eq_ignore_ascii_case("cookie") {
                *v = mask_cookie_header(v, &p.cookies, rep);
            } else if k.eq_ignore_ascii_case("set-cookie") {
                *v = mask_set_cookie(v, &p.cookies, rep);
            } else if k.eq_ignore_ascii_case("referer") || k.eq_ignore_ascii_case("location") {
                *v = mask_query(v, &p.query_params, rep);
            }
        }
    }
    for (k, v) in r.response_trailers.iter_mut() {
        if name_in(&p.headers, k) {
            *v = rep.to_string();
        }
    }
    r.url = mask_query(&r.url, &p.query_params, rep);
    for re in p.body_regexes.iter().filter(|x| !x.trim().is_empty()) {
        // Bytes regex on the exact bodies, so binary bodies (kept as base64) are masked too.
        if let Ok(re) = regex::bytes::Regex::new(re) {
            let req = re.replace_all(&r.request_bytes(), regex::bytes::NoExpand(rep.as_bytes())).into_owned();
            let resp = re.replace_all(&r.response_bytes(), regex::bytes::NoExpand(rep.as_bytes())).into_owned();
            r.set_request_body(&req);
            r.set_response_body(&resp);
        }
    }
    r
}

/// Plain-text dump of a session (request + response), used by "Copy sanitized".
pub fn session_text(r: &SessionRecord) -> String {
    let mut s = format!("{} {}\n", r.method, r.url);
    for (k, v) in &r.request_headers {
        s.push_str(&format!("{k}: {v}\n"));
    }
    if !r.request_body.is_empty() {
        s.push('\n');
        s.push_str(&r.request_body);
        s.push('\n');
    }
    s.push_str(&format!("\n--- response {} ---\n", r.status));
    for (k, v) in &r.response_headers {
        s.push_str(&format!("{k}: {v}\n"));
    }
    if !r.response_body.is_empty() {
        s.push('\n');
        s.push_str(&r.response_body);
        s.push('\n');
    }
    s
}

// ---------------------------------------------------------------- shortcuts

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    ToggleCapture,
    ClearAll,
    Find,
    Composer,
    Rules,
    Breakpoints,
    Decode,
    Settings,
    Home,
    NextSession,
    PrevSession,
    Shortcuts,
    // Handled by the grid itself (listed in the dialog only)
    SelectAll,
    RemoveSelected,
    ClearSelection,
}

pub struct Shortcut {
    pub action: Action,
    pub key: Key,
    /// Ctrl (Cmd on macOS) held.
    pub ctrl: bool,
    pub label: &'static str,
    /// Handled elsewhere in the app; this module only lists it.
    pub builtin: bool,
}

const fn sc(action: Action, key: Key, ctrl: bool, label: &'static str, builtin: bool) -> Shortcut {
    Shortcut { action, key, ctrl, label, builtin }
}

/// Every keyboard shortcut. Each (key, ctrl) pair appears once.
pub const SHORTCUTS: &[Shortcut] = &[
    sc(Action::ToggleCapture, Key::F12, false, "Start / pause capture", false),
    sc(Action::ClearAll, Key::X, true, "Clear all sessions", false),
    sc(Action::Find, Key::F, true, "Find: full-text search box", false),
    sc(Action::Composer, Key::E, true, "Composer", false),
    sc(Action::Rules, Key::R, true, "Rules", false),
    sc(Action::Breakpoints, Key::B, true, "Breakpoints", false),
    sc(Action::Decode, Key::D, true, "Decode / encode tool", false),
    sc(Action::Settings, Key::Comma, true, "Settings", false),
    sc(Action::Home, Key::H, true, "Home screen", false),
    sc(Action::NextSession, Key::ArrowDown, false, "Next session", false),
    sc(Action::PrevSession, Key::ArrowUp, false, "Previous session", false),
    sc(Action::Shortcuts, Key::F1, false, "Keyboard shortcuts (this list)", false),
    sc(Action::Shortcuts, Key::Slash, true, "Keyboard shortcuts (this list)", false),
    sc(Action::SelectAll, Key::A, true, "Select all visible sessions", true),
    sc(Action::RemoveSelected, Key::Delete, false, "Remove selected sessions", true),
    sc(Action::ClearSelection, Key::Escape, false, "Clear the selection", true),
];

pub fn shortcut_text(s: &Shortcut) -> String {
    let k = match s.key {
        Key::Comma => ",".to_string(),
        Key::Slash => "/".to_string(),
        Key::ArrowDown => "Down".to_string(),
        Key::ArrowUp => "Up".to_string(),
        k => k.name().to_string(),
    };
    if s.ctrl { format!("Ctrl+{k}") } else { k }
}

/// The first shortcut text for an action (for tooltips), e.g. "Ctrl+E".
pub fn shortcut_for(a: Action) -> String {
    SHORTCUTS.iter().find(|s| s.action == a).map(shortcut_text).unwrap_or_default()
}

/// The actions whose shortcuts were pressed this frame (consumes those key presses). Ctrl+X arrives as a Cut event.
pub fn pressed_actions(ctx: &egui::Context) -> Vec<Action> {
    ctx.input_mut(|i| {
        let mut v = Vec::new();
        for s in SHORTCUTS.iter().filter(|s| !s.builtin) {
            let m = if s.ctrl { egui::Modifiers::COMMAND } else { egui::Modifiers::NONE };
            if i.consume_key(m, s.key) && !v.contains(&s.action) {
                v.push(s.action);
            }
        }
        if i.events.iter().any(|e| matches!(e, egui::Event::Cut)) && !v.contains(&Action::ClearAll) {
            v.push(Action::ClearAll);
        }
        v
    })
}

/// The next / previous id in grid order (the first / last when nothing is selected).
pub fn step_selection(order: &[usize], current: Option<usize>, next: bool) -> Option<usize> {
    if order.is_empty() {
        return None;
    }
    match current.and_then(|c| order.iter().position(|&x| x == c)) {
        None => Some(if next { order[0] } else { order[order.len() - 1] }),
        Some(p) if next => Some(order[(p + 1).min(order.len() - 1)]),
        Some(p) => Some(order[p.saturating_sub(1)]),
    }
}

// ---------------------------------------------------------------- status bar

/// What the status bar shows; built by the status panel in ui.rs each frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AppInfo {
    pub total: usize,
    pub visible: usize,
    pub selected: usize,
    pub capturing: bool,
    pub port: u16,
    pub proxy_on: bool,
    pub active_rules: usize,
    pub active_breakpoints: usize,
    /// Sessions currently paused at a breakpoint.
    pub paused: usize,
}

pub fn status_text(i: &AppInfo) -> String {
    let mut s = format!(
        "{} sessions  |  {} visible  |  {} selected  |  {}  |  port {}  |  system proxy {}  |  rules: {} active  |  breakpoints: {} active",
        i.total,
        i.visible,
        i.selected,
        if i.capturing { "capturing" } else { "paused" },
        i.port,
        if i.proxy_on { "ON" } else { "off" },
        i.active_rules,
        i.active_breakpoints
    );
    if i.paused > 0 {
        s.push_str(&format!(" ({} paused)", i.paused));
    }
    s
}

// ---------------------------------------------------------------- state

/// Actions from this module's windows or keys that need GridApp's private fields; `qol_frame` returns them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameActions {
    /// Some(new capture state)
    pub capture: Option<bool>,
    /// Some(new Rules-window visibility)
    pub rules: Option<bool>,
    pub clear_all: bool,
}

/// The cards on the home screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HomeCard {
    StartCapture,
    OpenBrowser,
    Composer,
    Rules,
    ImportHar,
    Settings,
}

pub const HOME_CARDS: [(HomeCard, &str, &str); 6] = [
    (HomeCard::StartCapture, "Start capture", "Record the traffic that goes through the proxy"),
    (HomeCard::OpenBrowser, "Open browser", "A clean browser profile that already uses the proxy"),
    (HomeCard::Composer, "Composer", "Build and send a request by hand"),
    (HomeCard::Rules, "Rules", "Answer matching requests with a canned response"),
    (HomeCard::ImportHar, "Import HAR", "Open a .har or .ftap file without capturing"),
    (HomeCard::Settings, "Settings", "Theme, privacy masking, notifications"),
];

pub struct Toast {
    pub text: String,
    pub error: bool,
    pub at: Instant,
}

/// This module's UI state (kept in `GridApp::ext.qol`).
pub struct State {
    pub settings: Settings,
    pub path: PathBuf,
    pub loaded: bool,
    last_saved: String,
    pub save_msg: String,
    applied_theme: Option<egui::ThemePreference>,
    pub show_settings: bool,
    /// A window the Settings screen asked to open ("connections", "intercept", "messages").
    pub open_window: Option<&'static str>,
    pub show_shortcuts: bool,
    pub show_find: bool,
    focus_find: bool,
    pub show_home: bool,
    /// The user closed the automatic home screen.
    pub home_dismissed: bool,
    /// Edit buffers for the four privacy lists (one entry per line).
    pub privacy_text: [String; 4],
    privacy_text_for: Option<PrivacySettings>,
    pub toasts: Vec<Toast>,
    pub pending: FrameActions,
    pub open_https: bool,
    pub open_composer: bool,
    pub open_browser: bool,
    pub open_import: bool,
    pub last_info: AppInfo,
    // event tracking for notifications
    seen_capturing: Option<bool>,
    seen_paused: usize,
    seen_max_id: Option<usize>,
}

impl Default for State {
    fn default() -> Self {
        State {
            settings: Settings::default(),
            path: PathBuf::new(),
            loaded: false,
            last_saved: String::new(),
            save_msg: String::new(),
            applied_theme: None,
            show_settings: false,
            open_window: None,
            show_shortcuts: false,
            show_find: false,
            focus_find: false,
            show_home: false,
            home_dismissed: false,
            privacy_text: Default::default(),
            privacy_text_for: None,
            toasts: Vec::new(),
            pending: FrameActions::default(),
            open_https: false,
            open_composer: false,
            open_browser: false,
            open_import: false,
            last_info: AppInfo::default(),
            seen_capturing: None,
            seen_paused: 0,
            seen_max_id: None,
        }
    }
}

impl State {
    /// A state using the settings file at `path` (loaded now).
    pub fn with_path(path: PathBuf) -> Self {
        let settings = Settings::load(&path);
        let last_saved = serde_json::to_string(&settings).unwrap_or_default();
        State { settings, path, loaded: true, last_saved, ..Default::default() }
    }

    fn ensure_loaded(&mut self) {
        if !self.loaded {
            *self = State::with_path(Settings::default_path());
        }
    }

    /// Writes the settings file if anything changed since the last save.
    pub fn save_if_changed(&mut self) {
        let j = serde_json::to_string(&self.settings).unwrap_or_default();
        if j != self.last_saved {
            self.save_msg = match self.settings.save(&self.path) {
                Ok(()) => format!("Saved to {}", self.path.display()),
                Err(e) => format!("Could not save: {e}"),
            };
            self.last_saved = j;
        }
    }

    pub fn toast(&mut self, text: impl Into<String>, error: bool) {
        self.toasts.push(Toast { text: text.into(), error, at: Instant::now() });
        if self.toasts.len() > 5 {
            self.toasts.remove(0);
        }
    }

    /// Turns state changes into toasts according to the notification settings.
    /// `new_rows` are the sessions that arrived since the last call.
    pub fn notice_events(&mut self, info: &AppInfo, new_rows: &[&SessionRecord]) {
        let n = self.settings.notify.clone();
        if let Some(was) = self.seen_capturing {
            if was != info.capturing && n.capture {
                self.toast(if info.capturing { "Capture started" } else { "Capture paused" }, false);
            }
        }
        self.seen_capturing = Some(info.capturing);
        if info.paused > self.seen_paused && n.breakpoint_hit {
            self.toast(format!("Breakpoint hit: {} session(s) paused", info.paused), false);
        }
        self.seen_paused = info.paused;
        let hits = new_rows.iter().filter(|r| r.comment.starts_with("AutoResponder:")).count();
        if hits > 0 && n.rule_hit {
            self.toast(format!("Rule hit: {hits} session(s) answered by a rule"), false);
        }
        let errs = new_rows.iter().filter(|r| r.status == 0 || r.status >= 500).count();
        if errs > 0 && n.errors {
            self.toast(format!("{errs} failed session(s) (no response or 5xx)"), true);
        }
    }
}

// ---------------------------------------------------------------- windows (headless-testable)

fn list_to_text(v: &[String]) -> String {
    v.join("\n")
}

fn text_to_list(t: &str) -> Vec<String> {
    t.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()
}

/// The settings window. `selected` is the session used for the sanitization preview.
pub fn draw_settings(ctx: &egui::Context, st: &mut State, selected: Option<&SessionRecord>) {
    if !st.show_settings {
        return;
    }
    if st.privacy_text_for.as_ref() != Some(&st.settings.privacy) {
        let p = &st.settings.privacy;
        st.privacy_text = [list_to_text(&p.headers), list_to_text(&p.cookies), list_to_text(&p.query_params), list_to_text(&p.body_regexes)];
        st.privacy_text_for = Some(p.clone());
    }
    let mut open = true;
    egui::Window::new("Settings").open(&mut open).default_width(560.0).default_height(560.0).vscroll(true).show(ctx, |ui| {
        egui::CollapsingHeader::new(RichText::new("Appearance").strong()).default_open(true).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("Theme:");
                for t in ThemeChoice::ALL {
                    ui.radio_value(&mut st.settings.theme, t, t.label());
                }
            });
            let sys = match ctx.system_theme() {
                Some(egui::Theme::Light) => "light",
                Some(egui::Theme::Dark) => "dark",
                None => "unknown",
            };
            ui.label(RichText::new(format!("The OS reports: {sys}. In use now: {:?}.", ctx.theme())).weak());
            ui.checkbox(&mut st.settings.show_home, "Show the home screen when there are no sessions");
        });
        egui::CollapsingHeader::new(RichText::new("Capture / connections").strong()).default_open(true).show(ui, |ui| {
            let i = st.last_info.clone();
            ui.label(format!("Proxy port: {}", i.port));
            ui.label(format!("System proxy: {}", if i.proxy_on { "ON" } else { "off" }));
            ui.horizontal(|ui| {
                ui.label(format!("Capture: {}", if i.capturing { "capturing" } else { "paused" }));
                let b = if i.capturing { "Pause capture" } else { "Start capture" };
                if ui.button(b).on_hover_text(shortcut_for(Action::ToggleCapture)).clicked() {
                    st.pending.capture = Some(!i.capturing);
                }
            });
            ui.label("Listening port, remote devices, upstream gateway, host remapping, what is recorded and certificate errors:");
            if ui.button("Open Connections...").clicked() {
                st.open_window = Some("connections");
            }
        });
        egui::CollapsingHeader::new(RichText::new("Toolbar").strong()).default_open(false).show(ui, |ui| {
            ui.label("Buttons shown in the toolbar:");
            ui.horizontal_wrapped(|ui| {
                for (id, label) in TOOLBAR_ITEMS {
                    let mut on = st.settings.toolbar_shows(id);
                    if ui.checkbox(&mut on, *label).changed() {
                        st.settings.hidden_toolbar.retain(|h| h != id);
                        if !on {
                            st.settings.hidden_toolbar.push(id.to_string());
                        }
                    }
                }
            });
            if ui.small_button("Show all").clicked() {
                st.settings.hidden_toolbar.clear();
            }
        });
        egui::CollapsingHeader::new(RichText::new("Browsers and terminal").strong()).default_open(true).show(ui, |ui| {
            ui.label("Default browser (right-click one there), browser profile folder (Connections), default terminal and any program with capture on:");
            if ui.button("Open Intercept...").clicked() {
                st.open_window = Some("intercept");
            }
        });
        egui::CollapsingHeader::new(RichText::new("Scripting").strong()).default_open(false).show(ui, |ui| {
            ui.label("Scripts run on each request / response; .rhai files in the plugin folder load at start.");
            if ui.button("Open Scripts...").clicked() {
                st.open_window = Some("scripts");
            }
        });
        egui::CollapsingHeader::new(RichText::new("Protobuf").strong()).default_open(false).show(ui, |ui| {
            ui.label(".proto files used to show gRPC messages with field names are listed in the Messages window (gRPC session > Protobuf).");
            if ui.button("Open Messages...").clicked() {
                st.open_window = Some("messages");
            }
        });
        egui::CollapsingHeader::new(RichText::new("HTTPS").strong()).default_open(true).show(ui, |ui| {
            ui.label("Root certificate export, trust and the do-not-decrypt list are in the HTTPS window.");
            if ui.button("Open HTTPS settings...").clicked() {
                st.open_https = true;
            }
        });
        egui::CollapsingHeader::new(RichText::new("Privacy (masking for sharing / exports)").strong()).default_open(true).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("Replacement text:");
                ui.text_edit_singleline(&mut st.settings.privacy.replacement);
            });
            let labels = ["Header names", "Cookie names", "URL query parameter names", "Body regular expressions"];
            let mut changed = false;
            ui.columns(2, |cols| {
                for (k, label) in labels.iter().enumerate() {
                    let c = &mut cols[k % 2];
                    c.label(format!("{label} (one per line):"));
                    changed |= c.add(egui::TextEdit::multiline(&mut st.privacy_text[k]).desired_rows(3).desired_width(f32::INFINITY)).changed();
                }
            });
            if changed {
                let p = &mut st.settings.privacy;
                p.headers = text_to_list(&st.privacy_text[0]);
                p.cookies = text_to_list(&st.privacy_text[1]);
                p.query_params = text_to_list(&st.privacy_text[2]);
                p.body_regexes = text_to_list(&st.privacy_text[3]);
                st.privacy_text_for = Some(p.clone());
            }
            for (re, e) in invalid_regexes(&st.settings.privacy) {
                ui.colored_label(Color32::from_rgb(220, 80, 60), format!("Invalid regex {re}: {}", e.lines().last().unwrap_or("")));
            }
            ui.separator();
            match selected {
                None => {
                    ui.label(RichText::new("Select a session in the grid to preview it sanitized.").italics().weak());
                }
                Some(r) => {
                    let clean = sanitize(r, &st.settings.privacy);
                    let text = session_text(&clean);
                    ui.horizontal(|ui| {
                        ui.label(format!("Preview of session #{} with the masks applied:", r.id));
                        if ui.button("Copy sanitized").clicked() {
                            ctx.copy_text(text.clone());
                            st.toast(format!("Copied session #{} (sanitized)", r.id), false);
                        }
                    });
                    let mut shown: String = if text.len() > 20_000 { text.chars().take(20_000).chain("...".chars()).collect() } else { text };
                    egui::ScrollArea::vertical().max_height(200.0).id_salt("qol_preview").show(ui, |ui| {
                        ui.add(egui::TextEdit::multiline(&mut shown).code_editor().interactive(false).desired_width(f32::INFINITY));
                    });
                }
            }
        });
        egui::CollapsingHeader::new(RichText::new("Notifications").strong()).default_open(true).show(ui, |ui| {
            ui.label("Show a toast (bottom right) when:");
            let n = &mut st.settings.notify;
            ui.checkbox(&mut n.capture, "capture starts or pauses");
            ui.checkbox(&mut n.rule_hit, "a rule answers a request");
            ui.checkbox(&mut n.breakpoint_hit, "a breakpoint pauses a session");
            ui.checkbox(&mut n.errors, "sessions fail (no response or 5xx)");
            if ui.button("Show a test toast").clicked() {
                st.toast("This is how notifications look", false);
            }
        });
        egui::CollapsingHeader::new(RichText::new("Keyboard").strong()).default_open(false).show(ui, |ui| {
            if ui.button("Show keyboard shortcuts...").clicked() {
                st.show_shortcuts = true;
            }
        });
        ui.separator();
        ui.label(RichText::new(if st.save_msg.is_empty() { format!("Settings file: {}", st.path.display()) } else { st.save_msg.clone() }).weak());
    });
    if !open {
        st.show_settings = false;
    }
}

pub fn draw_shortcuts(ctx: &egui::Context, st: &mut State) {
    if !st.show_shortcuts {
        return;
    }
    let mut open = true;
    egui::Window::new("Keyboard shortcuts").open(&mut open).resizable(false).show(ctx, |ui| {
        egui::Grid::new("qol_shortcuts").striped(true).num_columns(2).show(ui, |ui| {
            for s in SHORTCUTS {
                ui.label(RichText::new(shortcut_text(s)).monospace().strong());
                ui.label(s.label);
                ui.end_row();
            }
        });
        ui.label(RichText::new("Shortcuts work when no text box has the keyboard focus.").weak());
    });
    if !open {
        st.show_shortcuts = false;
    }
}

/// The home screen; returns the card that was clicked.
pub fn draw_home(ctx: &egui::Context, st: &mut State, has_sessions: bool) -> Option<HomeCard> {
    let auto = !has_sessions && st.settings.show_home && !st.home_dismissed;
    if !(st.show_home || auto) {
        return None;
    }
    let mut open = true;
    let mut picked = None;
    egui::Window::new("Home")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label(RichText::new("Welcome to Ferrotap").heading());
            ui.label("Pick a starting point. Sessions appear in the grid as soon as traffic goes through the proxy.");
            ui.add_space(6.0);
            egui::Grid::new("qol_home_cards").spacing([10.0, 10.0]).show(ui, |ui| {
                for (k, (card, title, hint)) in HOME_CARDS.iter().enumerate() {
                    egui::Frame::group(ui.style()).inner_margin(10.0).show(ui, |ui| {
                        ui.set_width(170.0);
                        ui.set_height(70.0);
                        ui.vertical(|ui| {
                            let title = if *card == HomeCard::StartCapture && st.last_info.capturing { "Capturing (pause)" } else { title };
                            if ui.button(RichText::new(title).strong()).clicked() {
                                picked = Some(*card);
                            }
                            ui.label(RichText::new(*hint).small().weak());
                        });
                    });
                    if k % 3 == 2 {
                        ui.end_row();
                    }
                }
            });
            ui.add_space(4.0);
            ui.label(RichText::new(format!("Reopen with {} or the Home button in the status bar. All shortcuts: {}.", shortcut_for(Action::Home), shortcut_for(Action::Shortcuts))).weak());
        });
    if !open || picked.is_some() {
        st.show_home = false;
        st.home_dismissed = true;
    }
    picked
}

pub fn draw_toasts(ctx: &egui::Context, st: &mut State) {
    st.toasts.retain(|t| t.at.elapsed() < Duration::from_secs(5));
    if st.toasts.is_empty() {
        return;
    }
    egui::Area::new(egui::Id::new("qol_toasts")).anchor(egui::Align2::RIGHT_BOTTOM, [-12.0, -40.0]).order(egui::Order::Foreground).show(ctx, |ui| {
        for t in &st.toasts {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                let c = if t.error { Color32::from_rgb(220, 80, 60) } else { ui.visuals().strong_text_color() };
                ui.label(RichText::new(&t.text).color(c));
            });
        }
    });
}

// ---------------------------------------------------------------- GridApp hooks

impl GridApp {
    /// Toolbar entry: the "Settings" menu.
    pub fn qol_menu(&mut self, ui: &mut egui::Ui) {
        let st = &mut self.ext.qol;
        ui.menu_button("Settings \u{25be}", |ui| {
            if ui.button(format!("Settings...   {}", shortcut_for(Action::Settings))).clicked() {
                st.show_settings = true;
                ui.close();
            }
            if ui.button(format!("Keyboard shortcuts...   {}", shortcut_for(Action::Shortcuts))).clicked() {
                st.show_shortcuts = true;
                ui.close();
            }
            if ui.button(format!("Home screen   {}", shortcut_for(Action::Home))).clicked() {
                st.show_home = true;
                ui.close();
            }
            ui.separator();
            ui.label("Theme:");
            for t in ThemeChoice::ALL {
                if ui.radio_value(&mut st.settings.theme, t, t.label()).clicked() {
                    ui.close();
                }
            }
        });
    }

    /// Windows this module draws each frame (also applies the theme and saves changed settings).
    pub fn qol_windows(&mut self, ctx: &egui::Context) {
        self.ext.qol.ensure_loaded();
        let pref = self.ext.qol.settings.theme.preference();
        if self.ext.qol.applied_theme != Some(pref) {
            ctx.set_theme(pref);
            self.ext.qol.applied_theme = Some(pref);
        }
        let selected = self.model.selected_row().cloned();
        draw_settings(ctx, &mut self.ext.qol, selected.as_ref());
        match self.ext.qol.open_window.take() {
            Some("connections") => self.ext.conn.show = true,
            Some("intercept") => self.ext.launch.show = true,
            Some("messages") => self.ext.streams.open = true,
            Some("scripts") => self.ext.scripts.show = true,
            _ => {}
        }
        draw_shortcuts(ctx, &mut self.ext.qol);
        if self.ext.qol.show_find {
            let mut open = true;
            egui::Window::new("Find").open(&mut open).resizable(false).show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Search:");
                    let r = ui.add(egui::TextEdit::singleline(&mut self.model.search_query).hint_text("url, headers, bodies, comments").desired_width(280.0));
                    if self.ext.qol.focus_find {
                        r.request_focus();
                        self.ext.qol.focus_find = false;
                    }
                    if ui.button("\u{2715}").on_hover_text("Clear the search").clicked() {
                        self.model.search_query.clear();
                    }
                });
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.model.search_in_headers, "Headers");
                    ui.checkbox(&mut self.model.search_in_bodies, "Bodies");
                    ui.checkbox(&mut self.model.search_case_sensitive, "Match case");
                });
                ui.label(RichText::new("The grid filters as you type (same search as the filter bar).").weak());
            });
            if !open {
                self.ext.qol.show_find = false;
            }
        }
        let has_sessions = !self.model.rows.is_empty();
        let st = &mut self.ext.qol;
        match draw_home(ctx, st, has_sessions) {
            Some(HomeCard::StartCapture) => st.pending.capture = Some(!st.last_info.capturing),
            Some(HomeCard::OpenBrowser) => st.open_browser = true,
            Some(HomeCard::Composer) => st.open_composer = true,
            Some(HomeCard::Rules) => st.pending.rules = Some(true),
            Some(HomeCard::ImportHar) => st.open_import = true,
            Some(HomeCard::Settings) => st.show_settings = true,
            None => {}
        }
        if std::mem::take(&mut st.open_https) {
            self.show_https = true;
        }
        if std::mem::take(&mut st.open_composer) {
            self.show_composer = true;
        }
        if std::mem::take(&mut st.open_import) {
            self.ext.data.show_import = true;
        }
        if std::mem::take(&mut self.ext.qol.open_browser) {
            self.open_clean_browser();
        }
        self.ext.qol.save_if_changed();
    }

    /// Called by the status panel each frame: keyboard shortcuts, notifications and toasts. Returns the actions
    /// that need GridApp's own fields (capture, Rules window, clear all); the caller applies them.
    pub fn qol_frame(&mut self, ctx: &egui::Context, info: &AppInfo) -> FrameActions {
        self.ext.qol.ensure_loaded();
        self.ext.qol.last_info = info.clone();
        let mut out = std::mem::take(&mut self.ext.qol.pending);
        if !ctx.egui_wants_keyboard_input() {
            for a in pressed_actions(ctx) {
                let st = &mut self.ext.qol;
                match a {
                    Action::ToggleCapture => out.capture = Some(!info.capturing),
                    Action::ClearAll => out.clear_all = true,
                    Action::Find => {
                        st.show_find = true;
                        st.focus_find = true;
                    }
                    Action::Composer => self.show_composer = !self.show_composer,
                    Action::Rules => out.rules = Some(true),
                    Action::Breakpoints => self.show_breakpoints = !self.show_breakpoints,
                    Action::Decode => self.ext.inspect.decode_open = !self.ext.inspect.decode_open,
                    Action::Settings => st.show_settings = !st.show_settings,
                    Action::Home => st.show_home = !st.show_home,
                    Action::Shortcuts => st.show_shortcuts = !st.show_shortcuts,
                    Action::NextSession | Action::PrevSession => {
                        if let Some(id) = step_selection(&self.ext.last_visible, self.model.selected, a == Action::NextSession) {
                            self.model.selected = Some(id);
                            self.ext.sel.only(id);
                        }
                    }
                    Action::SelectAll | Action::RemoveSelected | Action::ClearSelection => {}
                }
            }
        }
        // Notifications for the sessions that arrived since the last frame
        let seen = self.ext.qol.seen_max_id;
        let new_rows: Vec<&SessionRecord> = match seen {
            Some(m) => self.model.rows.iter().filter(|r| r.id > m).collect(),
            None => Vec::new(),
        };
        let max_id = self.model.rows.iter().map(|r| r.id).max();
        self.ext.qol.notice_events(info, &new_rows);
        self.ext.qol.seen_max_id = max_id.or(seen);
        draw_toasts(ctx, &mut self.ext.qol);
        out
    }

    /// The status bar contents (inside `Panel::bottom("status")`).
    pub fn qol_status_bar(&mut self, ui: &mut egui::Ui, info: &AppInfo) {
        ui.horizontal(|ui| {
            let dot = if info.capturing { RichText::new("\u{25cf}").color(Color32::from_rgb(220, 60, 60)) } else { RichText::new("\u{25cb}").weak() };
            ui.label(dot);
            ui.label(status_text(info));
            ui.label(RichText::new(format!("|  source: {}", self.source_label)).weak());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let st = &mut self.ext.qol;
                if ui.small_button("?").on_hover_text(format!("Keyboard shortcuts ({})", shortcut_for(Action::Shortcuts))).clicked() {
                    st.show_shortcuts = !st.show_shortcuts;
                }
                if ui.small_button("Settings").on_hover_text(shortcut_for(Action::Settings)).clicked() {
                    st.show_settings = !st.show_settings;
                }
                if ui.small_button("Home").on_hover_text(shortcut_for(Action::Home)).clicked() {
                    st.show_home = !st.show_home;
                }
            });
        });
    }
}
