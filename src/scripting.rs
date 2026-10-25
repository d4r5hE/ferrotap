//! G9 scripting (Rhai) and the G16 plugin folder. Scripts can define any of:
//!   fn on_request(req)        -> change req.method / req.url / req.headers / req.body (return req)
//!   fn on_response(req, resp) -> change resp.status / resp.headers / resp.body (return resp)
//!   fn column(s)              -> text for a "Plugin" grid column
//!   fn inspect(s)             -> text for the inspector's Plugins tab
//! `req.headers` / `resp.headers` are maps of lower-case name -> value; bodies are strings.
//! `log(text)` adds a line to the Scripts window diagnostics.
//! Scripts run in a sandbox: no file, network or process access, and limits on operations,
//! call depth and data sizes, so a runaway script stops instead of hanging the proxy.
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use rhai::{Dynamic, Engine, Map, Scope, AST};
use serde::{Deserialize, Serialize};

use crate::model::SessionRecord;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct Script {
    pub name: String,
    pub enabled: bool,
    pub code: String,
}

/// Which hooks a script defines.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hooks {
    pub on_request: bool,
    pub on_response: bool,
    pub column: bool,
    pub inspect: bool,
}

/// A compiled, enabled script.
#[derive(Clone)]
pub struct Compiled {
    pub name: String,
    pub ast: Arc<AST>,
    pub hooks: Hooks,
}

pub const HOOK_NAMES: &[&str] = &["on_request", "on_response", "column", "inspect"];

/// Help text for the editor (hover help / reference).
pub const HELP: &str = "fn on_request(req) { req.headers[\"x-debug\"] = \"1\"; req }\n\
fn on_response(req, resp) { if resp.status == 500 { log(\"server error: \" + req.url); } resp }\n\
fn column(s) { s.headers.len().to_string() }   // s: method, url, status, headers, response_headers, duration_ms, process\n\
fn inspect(s) { \"Body length: \" + s.response_body.len() }\n\
req: method, url, headers (map), body | resp: status, headers (map), body | log(text)";

/// Identifiers offered by the editor's completion list.
pub const COMPLETIONS: &[&str] = &[
    "fn on_request(req) {\n    \n    req\n}",
    "fn on_response(req, resp) {\n    \n    resp\n}",
    "fn column(s) {\n    \"\"\n}",
    "fn inspect(s) {\n    \"\"\n}",
    "req.method", "req.url", "req.headers[\"name\"]", "req.body", "resp.status", "resp.headers[\"name\"]", "resp.body",
    "log(\"text\")", "s.url", "s.status", "s.process", ".contains(\"x\")", ".replace(\"a\", \"b\")",
];

// ------------------------------------------------------------------------------------------ engine

fn diag() -> &'static Mutex<Vec<String>> {
    static D: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    D.get_or_init(Default::default)
}

/// Add a diagnostics line (kept to the last 200).
pub fn note(line: String) {
    if let Ok(mut d) = diag().lock() {
        d.push(format!("{} {line}", chrono::Local::now().format("%H:%M:%S")));
        let n = d.len();
        if n > 200 {
            d.drain(..n - 200);
        }
    }
}

pub fn diagnostics() -> Vec<String> {
    diag().lock().map(|d| d.clone()).unwrap_or_default()
}

pub fn clear_diagnostics() {
    if let Ok(mut d) = diag().lock() {
        d.clear();
    }
}

/// The sandboxed engine.
pub fn engine() -> Engine {
    let mut e = Engine::new();
    e.set_max_operations(200_000);
    e.set_max_call_levels(32);
    e.set_max_expr_depths(64, 32);
    e.set_max_string_size(16 * 1024 * 1024);
    e.set_max_array_size(100_000);
    e.set_max_map_size(10_000);
    e.on_print(|s| note(format!("print: {s}")));
    e.on_debug(|s, _, _| note(format!("debug: {s}")));
    e.register_fn("log", |s: &str| note(format!("log: {s}")));
    e.register_fn("log", |d: Dynamic| note(format!("log: {d}")));
    e
}

fn shared_engine() -> &'static Engine {
    static E: OnceLock<Engine> = OnceLock::new();
    E.get_or_init(engine)
}

/// Compile and report the hooks; Err with a line:column message.
pub fn validate(code: &str) -> Result<Hooks, String> {
    let ast = shared_engine().compile(code).map_err(|e| e.to_string())?;
    Ok(hooks_of(&ast))
}

fn hooks_of(ast: &AST) -> Hooks {
    let has = |n: &str| ast.iter_functions().any(|f| f.name == n);
    Hooks { on_request: has("on_request"), on_response: has("on_response"), column: has("column"), inspect: has("inspect") }
}

fn active() -> &'static RwLock<Vec<Compiled>> {
    static A: OnceLock<RwLock<Vec<Compiled>>> = OnceLock::new();
    A.get_or_init(Default::default)
}

/// Compile the enabled scripts (in order) and make them live; returns (script, error) for failures.
pub fn set_active(scripts: &[Script]) -> Vec<(String, String)> {
    let mut list = Vec::new();
    let mut errs = Vec::new();
    for s in scripts.iter().filter(|s| s.enabled && !s.code.trim().is_empty()) {
        match shared_engine().compile(&s.code) {
            Ok(ast) => list.push(Compiled { name: s.name.clone(), hooks: hooks_of(&ast), ast: Arc::new(ast) }),
            Err(e) => errs.push((s.name.clone(), e.to_string())),
        }
    }
    if let Ok(mut a) = active().write() {
        *a = list;
    }
    errs
}

pub fn active_scripts() -> Vec<Compiled> {
    active().read().map(|a| a.clone()).unwrap_or_default()
}

pub fn has_hook(f: impl Fn(&Hooks) -> bool) -> bool {
    active().read().map(|a| a.iter().any(|c| f(&c.hooks))).unwrap_or(false)
}

fn headers_map(h: &[(String, String)]) -> Map {
    let mut m = Map::new();
    for (k, v) in h {
        let k = k.to_ascii_lowercase();
        match m.get_mut(k.as_str()) {
            Some(prev) => *prev = Dynamic::from(format!("{}, {v}", prev)),
            None => {
                m.insert(k.into(), Dynamic::from(v.clone()));
            }
        }
    }
    m
}

fn map_headers(m: &Map) -> Vec<(String, String)> {
    m.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

/// A field as text; a field set to nothing (`()`, e.g. from an in-place string method) counts as unchanged.
fn get_str(m: &Map, k: &str) -> Option<String> {
    m.get(k).filter(|v| !v.is_unit()).map(|v| v.clone().into_string().unwrap_or_else(|_| v.to_string()))
}

/// A request as scripts see it (and change it).
#[derive(Clone, Debug, PartialEq)]
pub struct ReqParts {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RespParts {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

fn req_map(r: &ReqParts) -> Map {
    let mut m = Map::new();
    m.insert("method".into(), r.method.clone().into());
    m.insert("url".into(), r.url.clone().into());
    m.insert("headers".into(), Dynamic::from_map(headers_map(&r.headers)));
    m.insert("body".into(), String::from_utf8_lossy(&r.body).into_owned().into());
    m
}

fn resp_map(r: &RespParts) -> Map {
    let mut m = Map::new();
    m.insert("status".into(), Dynamic::from(r.status as i64));
    m.insert("headers".into(), Dynamic::from_map(headers_map(&r.headers)));
    m.insert("body".into(), String::from_utf8_lossy(&r.body).into_owned().into());
    m
}

/// Run every enabled `on_request` in order. Returns the names of the scripts that changed it.
pub fn run_request(r: &mut ReqParts) -> Vec<String> {
    let mut changed = Vec::new();
    for c in active_scripts().iter().filter(|c| c.hooks.on_request) {
        let before = req_map(r);
        let res: Result<Dynamic, _> = shared_engine().call_fn(&mut Scope::new(), &c.ast, "on_request", (Dynamic::from_map(before.clone()),));
        match res {
            Ok(d) => {
                let Some(m) = d.try_cast::<Map>() else { continue };
                let body_text = String::from_utf8_lossy(&r.body).into_owned();
                let new = ReqParts {
                    method: get_str(&m, "method").unwrap_or_else(|| r.method.clone()),
                    url: get_str(&m, "url").unwrap_or_else(|| r.url.clone()),
                    headers: m.get("headers").and_then(|h| h.clone().try_cast::<Map>()).map(|h| map_headers(&h)).unwrap_or_else(|| r.headers.clone()),
                    body: match get_str(&m, "body") {
                        Some(b) if b != body_text => b.into_bytes(),
                        _ => r.body.clone(),
                    },
                };
                if new != *r {
                    *r = new;
                    changed.push(c.name.clone());
                }
            }
            Err(e) => note(format!("{}: on_request failed: {e}", c.name)),
        }
    }
    changed
}

/// Run every enabled `on_response` in order.
pub fn run_response(req: &ReqParts, resp: &mut RespParts) -> Vec<String> {
    let mut changed = Vec::new();
    for c in active_scripts().iter().filter(|c| c.hooks.on_response) {
        let rq = Dynamic::from_map(req_map(req));
        let res: Result<Dynamic, _> = shared_engine().call_fn(&mut Scope::new(), &c.ast, "on_response", (rq, Dynamic::from_map(resp_map(resp))));
        match res {
            Ok(d) => {
                let Some(m) = d.try_cast::<Map>() else { continue };
                let body_text = String::from_utf8_lossy(&resp.body).into_owned();
                let new = RespParts {
                    status: m.get("status").and_then(|s| s.as_int().ok()).map(|s| s.clamp(100, 999) as u16).unwrap_or(resp.status),
                    headers: m.get("headers").and_then(|h| h.clone().try_cast::<Map>()).map(|h| map_headers(&h)).unwrap_or_else(|| resp.headers.clone()),
                    body: match get_str(&m, "body") {
                        Some(b) if b != body_text => b.into_bytes(),
                        _ => resp.body.clone(),
                    },
                };
                if new != *resp {
                    *resp = new;
                    changed.push(c.name.clone());
                }
            }
            Err(e) => note(format!("{}: on_response failed: {e}", c.name)),
        }
    }
    changed
}

fn session_map(s: &SessionRecord) -> Map {
    let mut m = Map::new();
    m.insert("id".into(), Dynamic::from(s.id as i64));
    m.insert("method".into(), s.method.clone().into());
    m.insert("url".into(), s.url.clone().into());
    m.insert("status".into(), Dynamic::from(s.status as i64));
    m.insert("headers".into(), Dynamic::from_map(headers_map(&s.request_headers.iter().map(|(k, v)| (k.clone(), v.clone())).collect::<Vec<_>>())));
    m.insert("response_headers".into(), Dynamic::from_map(headers_map(&s.response_headers.iter().map(|(k, v)| (k.clone(), v.clone())).collect::<Vec<_>>())));
    m.insert("request_body".into(), s.request_body.clone().into());
    m.insert("response_body".into(), s.response_body.clone().into());
    m.insert("duration_ms".into(), Dynamic::from(s.duration_ms as i64));
    m.insert("process".into(), s.process_name.clone().unwrap_or_default().into());
    m.insert("comment".into(), s.comment.clone().into());
    m
}

/// Call `func` (column / inspect) of script `name` for a session.
pub fn call_session_fn(name: &str, func: &str, s: &SessionRecord) -> Result<String, String> {
    let list = active_scripts();
    let c = list.iter().find(|c| c.name == name).ok_or_else(|| format!("no active script {name}"))?;
    let d: Dynamic = shared_engine().call_fn(&mut Scope::new(), &c.ast, func, (Dynamic::from_map(session_map(s)),)).map_err(|e| e.to_string())?;
    Ok(d.into_string().unwrap_or_else(|d| d.to_string()))
}

/// Text of every active `inspect` hook for a session: (script, text or error).
pub fn inspect_all(s: &SessionRecord) -> Vec<(String, String)> {
    active_scripts()
        .iter()
        .filter(|c| c.hooks.inspect)
        .map(|c| (c.name.clone(), call_session_fn(&c.name, "inspect", s).unwrap_or_else(|e| format!("error: {e}"))))
        .collect()
}

/// Names of active scripts with a `column` hook (each can be a grid column).
pub fn column_scripts() -> Vec<String> {
    active_scripts().iter().filter(|c| c.hooks.column).map(|c| c.name.clone()).collect()
}

// ------------------------------------------------------------------------------------------ files

pub fn scripts_path() -> PathBuf {
    std::env::var("FERROTAP_SCRIPTS").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data").join("scripts.json"))
}

/// The plugin folder: every `*.rhai` in it is loaded as an enabled script named after the file.
pub fn plugins_dir() -> PathBuf {
    std::env::var("FERROTAP_PLUGINS").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("plugins"))
}

pub fn load_scripts(p: &Path) -> Vec<Script> {
    std::fs::read_to_string(p).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn save_scripts(p: &Path, s: &[Script]) -> Result<(), String> {
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    std::fs::write(p, serde_json::to_string_pretty(s).unwrap_or_default()).map_err(|e| e.to_string())
}

/// Scripts from the plugin folder (`plugin: <file stem>`), sorted by file name.
pub fn load_plugins(dir: &Path) -> Vec<Script> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("rhai"))).collect())
        .unwrap_or_default();
    files.sort();
    files
        .into_iter()
        .filter_map(|f| {
            let code = std::fs::read_to_string(&f).ok()?;
            Some(Script { name: format!("plugin: {}", f.file_stem()?.to_string_lossy()), enabled: true, code })
        })
        .collect()
}

/// Load a script file for the command line: a .rhai file, or a scripts.json list.
pub fn load_cli(p: &Path) -> Result<Vec<Script>, String> {
    let text = std::fs::read_to_string(p).map_err(|e| format!("read {}: {e}", p.display()))?;
    if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("json")) {
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", p.display()))
    } else {
        validate(&text).map_err(|e| format!("{}: {e}", p.display()))?;
        Ok(vec![Script { name: p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(), enabled: true, code: text }])
    }
}

// ------------------------------------------------------------------------------------------ window

/// Scripts window state (GridApp.ext.scripts).
#[derive(Default)]
pub struct ScriptsUi {
    pub show: bool,
    pub scripts: Option<Vec<Script>>,
    pub plugins: Vec<Script>,
    pub sel: Option<usize>,
    pub check: Option<Result<Hooks, String>>,
    pub msg: String,
    pub dirty: bool,
}

impl ScriptsUi {
    /// Load scripts + plugins once and make them live (the proxy uses them even when the window is closed).
    pub fn ensure_loaded(&mut self) {
        if self.scripts.is_some() {
            return;
        }
        self.scripts = Some(load_scripts(&scripts_path()));
        self.plugins = load_plugins(&plugins_dir());
        self.apply();
    }

    /// Compile and activate the enabled scripts, then the enabled plugins.
    pub fn apply(&mut self) {
        let mut all = self.scripts.clone().unwrap_or_default();
        all.extend(self.plugins.iter().cloned());
        let errs = set_active(&all);
        self.msg = if errs.is_empty() {
            format!("{} script(s) active", active_scripts().len())
        } else {
            errs.iter().map(|(n, e)| format!("{n}: {e}")).collect::<Vec<_>>().join("; ")
        };
    }
}

fn hooks_text(h: &Hooks) -> String {
    let mut v = Vec::new();
    if h.on_request {
        v.push("on_request");
    }
    if h.on_response {
        v.push("on_response");
    }
    if h.column {
        v.push("column");
    }
    if h.inspect {
        v.push("inspect");
    }
    if v.is_empty() { "no hooks yet (define on_request, on_response, column or inspect)".into() } else { format!("OK - hooks: {}", v.join(", ")) }
}

impl crate::ui::GridApp {
    pub fn scripts_menu(&mut self, ui: &mut eframe::egui::Ui) {
        if ui.selectable_label(self.ext.scripts.show, "Scripts").on_hover_text("Scripts that run on each request / response, and plugins").clicked() {
            self.ext.scripts.show = !self.ext.scripts.show;
        }
    }

    pub fn scripts_windows(&mut self, ctx: &eframe::egui::Context) {
        use eframe::egui::{self, Color32, RichText};
        let st = &mut self.ext.scripts;
        st.ensure_loaded();
        if !st.show {
            return;
        }
        let mut open = true;
        let mut save = false;
        egui::Window::new("Scripts").open(&mut open).default_width(760.0).default_height(560.0).show(ctx, |ui| {
            ui.label(RichText::new("Sandbox: scripts cannot touch files, the network or other programs; long-running scripts are stopped (operation and size limits).").small().color(Color32::GRAY));
            ui.horizontal(|ui| {
                if ui.button("Save and apply").clicked() {
                    save = true;
                }
                if ui.button("New script").clicked() {
                    let list = st.scripts.get_or_insert_with(Vec::new);
                    list.push(Script { name: format!("script {}", list.len() + 1), enabled: true, code: "fn on_request(req) {\n    req\n}\n".into() });
                    st.sel = Some(list.len() - 1);
                    st.check = None;
                    st.dirty = true;
                }
                ui.label(RichText::new(&st.msg).color(Color32::GRAY));
            });
            ui.separator();
            ui.columns(2, |cols| {
                let ui = &mut cols[0];
                ui.strong("Scripts (run in this order)");
                let list = st.scripts.get_or_insert_with(Vec::new);
                let mut action: Option<(usize, &str)> = None;
                for (i, s) in list.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut s.enabled, "").changed() {
                            st.dirty = true;
                        }
                        if ui.selectable_label(st.sel == Some(i), &s.name).clicked() {
                            st.sel = Some(i);
                            st.check = None;
                        }
                        if ui.small_button("Up").clicked() {
                            action = Some((i, "up"));
                        }
                        if ui.small_button("Down").clicked() {
                            action = Some((i, "down"));
                        }
                        if ui.small_button("Duplicate").clicked() {
                            action = Some((i, "dup"));
                        }
                        if ui.small_button("Delete").clicked() {
                            action = Some((i, "del"));
                        }
                    });
                }
                if let Some((i, a)) = action {
                    match a {
                        "up" if i > 0 => {
                            list.swap(i, i - 1);
                            st.sel = Some(i - 1);
                        }
                        "down" if i + 1 < list.len() => {
                            list.swap(i, i + 1);
                            st.sel = Some(i + 1);
                        }
                        "dup" => {
                            let mut c = list[i].clone();
                            c.name = format!("{} (copy)", c.name);
                            list.insert(i + 1, c);
                            st.sel = Some(i + 1);
                        }
                        "del" => {
                            list.remove(i);
                            st.sel = None;
                        }
                        _ => {}
                    }
                    st.dirty = true;
                }
                ui.separator();
                ui.strong(format!("Plugins ({})", plugins_dir().display()));
                if st.plugins.is_empty() {
                    ui.label(RichText::new("Put .rhai files in this folder; they load at start (Reload to pick up changes).").small().color(Color32::GRAY));
                }
                for p in st.plugins.iter_mut() {
                    if ui.checkbox(&mut p.enabled, &p.name).changed() {
                        st.dirty = true;
                    }
                }
                if ui.small_button("Reload plugins").clicked() {
                    st.plugins = load_plugins(&plugins_dir());
                    st.dirty = true;
                }
                ui.separator();
                ui.horizontal(|ui| {
                    ui.strong("Diagnostics");
                    if ui.small_button("Clear").clicked() {
                        clear_diagnostics();
                    }
                });
                egui::ScrollArea::vertical().id_salt("script_diag").max_height(160.0).show(ui, |ui| {
                    let d = diagnostics();
                    if d.is_empty() {
                        ui.label(RichText::new("(no log lines or errors yet)").small().color(Color32::GRAY));
                    }
                    for l in d.iter().rev().take(80) {
                        let c = if l.contains("failed") { Color32::from_rgb(230, 90, 90) } else { Color32::GRAY };
                        ui.label(RichText::new(l).small().monospace().color(c));
                    }
                });

                let ui = &mut cols[1];
                let Some(i) = st.sel.filter(|&i| i < list.len()) else {
                    ui.label("Select or create a script.");
                    return;
                };
                let s = &mut list[i];
                ui.horizontal(|ui| {
                    ui.label("Name");
                    if ui.text_edit_singleline(&mut s.name).changed() {
                        st.dirty = true;
                    }
                });
                let r = ui.add(egui::TextEdit::multiline(&mut s.code).code_editor().desired_rows(16).desired_width(f32::INFINITY));
                if r.changed() || st.check.is_none() {
                    st.check = Some(validate(&s.code));
                    st.dirty |= r.changed();
                }
                match &st.check {
                    Some(Ok(h)) => {
                        ui.colored_label(Color32::LIGHT_GREEN, hooks_text(h));
                    }
                    Some(Err(e)) => {
                        ui.colored_label(Color32::from_rgb(230, 90, 90), e);
                    }
                    None => {}
                }
                ui.label("Insert:");
                ui.horizontal_wrapped(|ui| {
                    for c in COMPLETIONS {
                        let label = c.lines().next().unwrap_or(c);
                        if ui.small_button(label).on_hover_text(*c).clicked() {
                            if !s.code.ends_with('\n') && !s.code.is_empty() {
                                s.code.push('\n');
                            }
                            s.code.push_str(c);
                            st.check = None;
                            st.dirty = true;
                        }
                    }
                });
                ui.collapsing("Help", |ui| {
                    ui.label(RichText::new(HELP).monospace().small());
                });
            });
        });
        if save {
            let list = st.scripts.clone().unwrap_or_default();
            match save_scripts(&scripts_path(), &list) {
                Ok(()) => {
                    st.apply();
                    st.dirty = false;
                }
                Err(e) => st.msg = format!("Save failed: {e}"),
            }
        }
        if st.dirty && !save {
            st.msg = "Unsaved changes: Save and apply to use them".into();
        }
        st.show = open;
    }
}
