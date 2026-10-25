//! G6/G17 sessions storage: HAR / own-format export and import, snapshots, auto-save, HAR viewer.
//! Owned by one builder; the app calls `data_io_menu` from the toolbar and `data_io_windows` every frame.
//!
//! The logic (file formats, snapshot folders, auto-save pruning, compare) is plain functions so it
//! is unit-testable; the `GridApp` methods at the bottom are the egui windows over it.
use std::path::{Path, PathBuf};
use std::time::Instant;

use eframe::egui::{self, Color32, RichText};
use serde::{Deserialize, Serialize};

use crate::model::{SessionEvent, SessionRecord};
use crate::ui::GridApp;

// ---------------------------------------------------------------------------------------------
// Paths

/// The user's Documents\Ferrotap folder (default location for exports and snapshots).
pub fn documents_dir() -> PathBuf {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join("Documents").join("Ferrotap")
}

/// Root folder of all snapshots. Override with env FERROTAP_SNAPSHOTS (used by tests).
pub fn snapshots_root() -> PathBuf {
    match std::env::var_os("FERROTAP_SNAPSHOTS") {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        _ => documents_dir().join("Snapshots"),
    }
}

// ---------------------------------------------------------------------------------------------
// Own format (.ftap): JSON with a small header and the session records.

pub const FTAP_FORMAT: &str = "ferrotap-sessions";

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FtapFile {
    pub format: String,
    pub version: u32,
    #[serde(default)]
    pub created: String,
    #[serde(default)]
    pub name: String,
    pub sessions: Vec<SessionRecord>,
}

pub fn ftap_to_string(name: &str, sessions: &[SessionRecord]) -> Result<String, String> {
    let f = FtapFile {
        format: FTAP_FORMAT.to_string(),
        version: 1,
        created: chrono::Utc::now().to_rfc3339(),
        name: name.to_string(),
        sessions: sessions.to_vec(),
    };
    serde_json::to_string(&f).map_err(|e| e.to_string())
}

/// Parse a .ftap document. A plain JSON array of session records (the old sessions.json dump)
/// is accepted too.
pub fn parse_ftap(text: &str) -> Result<Vec<SessionRecord>, String> {
    let text = text.trim_start_matches('\u{feff}');
    if text.trim_start().starts_with('[') {
        return serde_json::from_str::<Vec<SessionRecord>>(text).map_err(|e| format!("not a session list: {e}"));
    }
    let f: FtapFile = serde_json::from_str(text).map_err(|e| format!("not a Ferrotap session file: {e}"))?;
    if f.format != FTAP_FORMAT {
        return Err(format!("unknown format '{}'", f.format));
    }
    Ok(f.sessions)
}

fn ensure_parent(path: &Path) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
        }
    }
    Ok(())
}

pub fn export_ftap(path: &Path, sessions: &[SessionRecord]) -> Result<usize, String> {
    ensure_parent(path)?;
    let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    std::fs::write(path, ftap_to_string(&name, sessions)?).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(sessions.len())
}

pub fn import_ftap(path: &Path) -> Result<Vec<SessionRecord>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    parse_ftap(&text)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExportFormat {
    #[default]
    Har,
    Ftap,
    Saz,
    Postman,
}

impl ExportFormat {
    pub fn ext(self) -> &'static str {
        match self {
            ExportFormat::Har => "har",
            ExportFormat::Ftap => "ftap",
            ExportFormat::Saz => "saz",
            ExportFormat::Postman => "json",
        }
    }
}

/// Copies of the sessions with the privacy settings applied (for "share a redacted copy").
pub fn redacted(sessions: &[SessionRecord], p: &crate::qol::PrivacySettings) -> Vec<SessionRecord> {
    sessions.iter().map(|s| crate::qol::sanitize(s, p)).collect()
}

/// Export sessions in the chosen format.
pub fn export_sessions(path: &Path, fmt: ExportFormat, sessions: &[SessionRecord]) -> Result<usize, String> {
    match fmt {
        ExportFormat::Har => crate::har::export_har(path, sessions),
        ExportFormat::Ftap => export_ftap(path, sessions),
        ExportFormat::Saz => crate::saz::export_saz(path, sessions),
        ExportFormat::Postman => crate::postman::export_postman(path, sessions),
    }
}

/// Import a .har, .saz, .ftap / .json file, or every such file in a folder (sorted by name).
pub fn import_path(path: &Path) -> Result<Vec<SessionRecord>, String> {
    if path.is_dir() {
        let mut files: Vec<PathBuf> = std::fs::read_dir(path)
            .map_err(|e| format!("read {}: {e}", path.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_file() && matches!(ext_of(p).as_str(), "har" | "saz" | "ftap" | "json"))
            .collect();
        files.sort();
        let mut out = Vec::new();
        for f in files {
            out.extend(import_file(&f)?);
        }
        return Ok(out);
    }
    import_file(path)
}

fn ext_of(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

fn import_file(path: &Path) -> Result<Vec<SessionRecord>, String> {
    if ext_of(path) == "saz" {
        return crate::saz::import_saz(path).map_err(|e| format!("{}: {e}", path.display()));
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    // Postman collections carry their schema URL; detect them before the HAR/ftap sniff.
    if text.contains("schema.getpostman.com") {
        return crate::postman::parse_collection(&text).map_err(|e| format!("{}: {e}", path.display()));
    }
    let looks_har = ext_of(path) == "har" || (text.contains("\"log\"") && !text.contains(FTAP_FORMAT));
    if looks_har {
        crate::har::parse_har(&text).map_err(|e| format!("{}: {e}", path.display()))
    } else {
        parse_ftap(&text).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// First id handed to imported sessions; far above the proxy's live counter so they never clash.
pub const IMPORT_ID_BASE: usize = 1_000_000_000;

/// Give imported sessions fresh ids that do not clash with `existing` (or with each other).
pub fn assign_fresh_ids(imported: &mut [SessionRecord], existing: &[SessionRecord]) {
    let mut next = existing.iter().map(|r| r.id + 1).max().unwrap_or(0).max(IMPORT_ID_BASE);
    for r in imported.iter_mut() {
        r.id = next;
        next += 1;
    }
}

// ---------------------------------------------------------------------------------------------
// Snapshots: `<root>/<folder>/<name>.ftap`; folder "" is the root itself.

pub const SNAPSHOT_EXT: &str = "ftap";

#[derive(Debug, Clone, PartialEq)]
pub struct SnapshotInfo {
    pub folder: String,
    pub name: String,
    pub path: PathBuf,
    pub bytes: u64,
    pub modified: String,
    /// Password protected (encrypted).
    pub locked: bool,
}

impl SnapshotInfo {
    pub fn label(&self) -> String {
        if self.folder.is_empty() { self.name.clone() } else { format!("{}/{}", self.folder, self.name) }
    }
}

/// Make a user-typed name safe as a file / folder name.
pub fn clean_name(name: &str) -> Result<String, String> {
    if name.trim().is_empty() {
        return Err("name is empty".to_string());
    }
    Ok(crate::model::safe_file_name(name))
}

pub fn snapshot_path(root: &Path, folder: &str, name: &str) -> Result<PathBuf, String> {
    let dir = if folder.is_empty() { root.to_path_buf() } else { root.join(clean_name(folder)?) };
    Ok(dir.join(format!("{}.{SNAPSHOT_EXT}", clean_name(name)?)))
}

/// Sub-folders of the snapshot root (sorted). The root itself is folder "".
pub fn list_folders(root: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(root)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.sort_by_key(|s| s.to_lowercase());
    v
}

pub fn create_folder(root: &Path, name: &str) -> Result<PathBuf, String> {
    let p = root.join(clean_name(name)?);
    std::fs::create_dir_all(&p).map_err(|e| format!("create {}: {e}", p.display()))?;
    Ok(p)
}

fn snapshots_in(dir: &Path, folder: &str, out: &mut Vec<SnapshotInfo>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.filter_map(|e| e.ok()) {
        let p = e.path();
        if !p.is_file() || ext_of(&p) != SNAPSHOT_EXT { continue; }
        let meta = e.metadata().ok();
        let modified = meta
            .as_ref()
            .and_then(|m| m.modified().ok())
            .map(|t| chrono::DateTime::<chrono::Local>::from(t).format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_default();
        out.push(SnapshotInfo {
            folder: folder.to_string(),
            name: p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
            bytes: meta.map(|m| m.len()).unwrap_or(0),
            modified,
            locked: crate::snapcrypt::is_encrypted(&p),
            path: p,
        });
    }
}

/// All snapshots in the root and its sub-folders, sorted by folder then name.
pub fn list_snapshots(root: &Path) -> Vec<SnapshotInfo> {
    let mut out = Vec::new();
    snapshots_in(root, "", &mut out);
    for f in list_folders(root) {
        snapshots_in(&root.join(&f), &f, &mut out);
    }
    out.sort_by_key(|a| (a.folder.to_lowercase(), a.name.to_lowercase()));
    out
}

/// Save sessions as a named snapshot. Fails if one with that name already exists in the folder.
pub fn save_snapshot(root: &Path, folder: &str, name: &str, sessions: &[SessionRecord]) -> Result<PathBuf, String> {
    let p = snapshot_path(root, folder, name)?;
    if p.exists() {
        return Err(format!("a snapshot named '{}' already exists", clean_name(name)?));
    }
    ensure_parent(&p)?;
    std::fs::write(&p, ftap_to_string(name, sessions)?).map_err(|e| format!("write {}: {e}", p.display()))?;
    Ok(p)
}

pub fn load_snapshot(path: &Path) -> Result<Vec<SessionRecord>, String> {
    import_ftap(path)
}

/// Rename a snapshot inside its folder; returns the new path.
pub fn rename_snapshot(path: &Path, new_name: &str) -> Result<PathBuf, String> {
    let dir = path.parent().ok_or("bad path")?;
    let np = dir.join(format!("{}.{SNAPSHOT_EXT}", clean_name(new_name)?));
    if np == path { return Ok(np); }
    if np.exists() {
        return Err(format!("a snapshot named '{}' already exists", clean_name(new_name)?));
    }
    std::fs::rename(path, &np).map_err(|e| format!("rename: {e}"))?;
    Ok(np)
}

/// Copy a snapshot next to itself as "<name> copy", "<name> copy 2", ...
pub fn clone_snapshot(path: &Path) -> Result<PathBuf, String> {
    let dir = path.parent().ok_or("bad path")?;
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut n = 1;
    loop {
        let candidate = if n == 1 { format!("{stem} copy") } else { format!("{stem} copy {n}") };
        let np = dir.join(format!("{candidate}.{SNAPSHOT_EXT}"));
        if !np.exists() {
            std::fs::copy(path, &np).map_err(|e| format!("copy: {e}"))?;
            return Ok(np);
        }
        n += 1;
    }
}

pub fn delete_snapshot(path: &Path) -> Result<(), String> {
    std::fs::remove_file(path).map_err(|e| format!("delete {}: {e}", path.display()))
}

// ---------------------------------------------------------------------------------------------
// Auto-save

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct AutoSaveCfg {
    pub enabled: bool,
    pub interval_min: u32,
    /// Folder for auto-save files; empty = `<snapshots root>/Autosave`.
    pub folder: String,
    pub keep: usize,
}

impl Default for AutoSaveCfg {
    fn default() -> Self {
        AutoSaveCfg { enabled: false, interval_min: 10, folder: String::new(), keep: 10 }
    }
}

impl AutoSaveCfg {
    pub fn dir(&self, root: &Path) -> PathBuf {
        if self.folder.trim().is_empty() { root.join("Autosave") } else { PathBuf::from(self.folder.trim()) }
    }
    pub fn path(root: &Path) -> PathBuf {
        root.join("autosave.json")
    }
    pub fn load(root: &Path) -> Self {
        std::fs::read_to_string(Self::path(root)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }
    pub fn save(&self, root: &Path) -> Result<(), String> {
        let p = Self::path(root);
        ensure_parent(&p)?;
        std::fs::write(&p, serde_json::to_string_pretty(self).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
    }
}

pub const AUTOSAVE_PREFIX: &str = "autosave-";

/// Write one auto-save file named after `stamp` (e.g. "20261016-101500") and prune to `keep` files.
pub fn autosave_write(dir: &Path, stamp: &str, sessions: &[SessionRecord], keep: usize) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let p = dir.join(format!("{AUTOSAVE_PREFIX}{stamp}.{SNAPSHOT_EXT}"));
    std::fs::write(&p, ftap_to_string(&format!("{AUTOSAVE_PREFIX}{stamp}"), sessions)?).map_err(|e| format!("write {}: {e}", p.display()))?;
    prune_autosaves(dir, keep)?;
    Ok(p)
}

/// Delete the oldest auto-save files so at most `keep` remain (keep 0 = keep all).
/// Returns the deleted paths. Only files named `autosave-*.ftap` are touched.
pub fn prune_autosaves(dir: &Path, keep: usize) -> Result<Vec<PathBuf>, String> {
    if keep == 0 { return Ok(Vec::new()); }
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("read {}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.is_file()
                && ext_of(p) == SNAPSHOT_EXT
                && p.file_name().map(|n| n.to_string_lossy().starts_with(AUTOSAVE_PREFIX)).unwrap_or(false)
        })
        .collect();
    files.sort(); // the timestamp in the name sorts chronologically
    let mut deleted = Vec::new();
    while files.len() > keep {
        let p = files.remove(0);
        std::fs::remove_file(&p).map_err(|e| format!("delete {}: {e}", p.display()))?;
        deleted.push(p);
    }
    Ok(deleted)
}

// ---------------------------------------------------------------------------------------------
// Compare

#[derive(Debug, Clone, PartialEq)]
pub struct ChangedPair {
    pub method: String,
    pub url: String,
    /// (status, response body size, duration ms) in A and in B.
    pub old: (u16, usize, u128),
    pub new: (u16, usize, u128),
    /// Which of "status", "size", "duration" differ.
    pub what: Vec<&'static str>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CompareResult {
    /// (method, url, status) only in B.
    pub added: Vec<(String, String, u16)>,
    /// (method, url, status) only in A.
    pub removed: Vec<(String, String, u16)>,
    pub changed: Vec<ChangedPair>,
    pub same: usize,
}

/// Compare snapshot A (old) with B (new). Sessions are matched by method + URL (the n-th
/// occurrence in A with the n-th in B). A matched pair is "changed" when the status, the
/// response body size, or the duration differs (duration only by more than `dur_tolerance_ms`).
pub fn compare_sessions(a: &[SessionRecord], b: &[SessionRecord], dur_tolerance_ms: u128) -> CompareResult {
    use std::collections::{HashMap, VecDeque};
    let key = |r: &SessionRecord| (r.method.to_uppercase(), r.url.clone());
    let mut pool: HashMap<(String, String), VecDeque<usize>> = HashMap::new();
    for (i, r) in a.iter().enumerate() {
        pool.entry(key(r)).or_default().push_back(i);
    }
    let mut matched = vec![false; a.len()];
    let mut res = CompareResult::default();
    for nb in b {
        match pool.get_mut(&key(nb)).and_then(|q| q.pop_front()) {
            None => res.added.push((nb.method.clone(), nb.url.clone(), nb.status)),
            Some(i) => {
                matched[i] = true;
                let oa = &a[i];
                let mut what = Vec::new();
                if oa.status != nb.status { what.push("status"); }
                if oa.body_size() != nb.body_size() { what.push("size"); }
                if oa.duration_ms.abs_diff(nb.duration_ms) > dur_tolerance_ms { what.push("duration"); }
                if what.is_empty() {
                    res.same += 1;
                } else {
                    res.changed.push(ChangedPair {
                        method: nb.method.clone(),
                        url: nb.url.clone(),
                        old: (oa.status, oa.body_size(), oa.duration_ms),
                        new: (nb.status, nb.body_size(), nb.duration_ms),
                        what,
                    });
                }
            }
        }
    }
    for (i, r) in a.iter().enumerate() {
        if !matched[i] {
            res.removed.push((r.method.clone(), r.url.clone(), r.status));
        }
    }
    res
}

// ---------------------------------------------------------------------------------------------
// UI state

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Scope {
    #[default]
    All,
    Visible,
    Selected,
}

impl Scope {
    pub fn label(self) -> &'static str {
        match self { Scope::All => "All", Scope::Visible => "Visible (filtered)", Scope::Selected => "Selected" }
    }
}

/// This module's UI state (kept in `GridApp::ext.data`).
#[derive(Default)]
pub struct State {
    pub show_export: bool,
    pub show_import: bool,
    pub show_snapshots: bool,
    pub show_autosave: bool,
    pub show_compare: bool,

    pub export_path: String,
    pub export_fmt: ExportFormat,
    pub export_scope: Scope,
    /// Apply the privacy settings (Settings > Privacy) to the exported copy.
    pub export_redact: bool,
    pub export_msg: String,

    pub show_report: bool,
    pub report_path: String,
    pub report_fmt: crate::report::ReportFormat,
    pub report_scope: Scope,
    pub report_title: String,
    pub report_redact: bool,
    pub report_msg: String,

    pub import_path: String,
    pub import_append: bool,
    pub import_msg: String,

    pub snap_root: String,
    pub snap_folder: String,
    pub snap_new_folder: String,
    pub snap_name: String,
    pub snap_scope: Scope,
    pub snap_list: Vec<SnapshotInfo>,
    pub snap_folders: Vec<String>,
    pub snap_loaded: bool,
    pub snap_msg: String,
    pub snap_rename: Option<(PathBuf, String)>,
    pub snap_confirm_delete: Option<PathBuf>,
    /// Password for opening protected snapshots (and for protecting new ones).
    pub snap_password: String,
    pub snap_protect: bool,
    /// Snapshot whose password panel is open, and the new password typed there.
    pub snap_pw_for: Option<PathBuf>,
    pub snap_new_password: String,

    pub autosave: AutoSaveCfg,
    pub autosave_loaded: bool,
    pub autosave_last: Option<Instant>,
    pub autosave_msg: String,
    /// An auto-save being written (serializing many sessions takes seconds).
    pub autosave_job: Option<std::sync::mpsc::Receiver<String>>,

    pub cmp_a: Option<PathBuf>,
    /// None = the sessions currently in the grid.
    pub cmp_b: Option<PathBuf>,
    pub cmp_tol: String,
    pub cmp_result: Option<CompareResult>,
    pub cmp_msg: String,
}

impl State {
    fn root(&mut self) -> PathBuf {
        if self.snap_root.trim().is_empty() {
            self.snap_root = snapshots_root().display().to_string();
        }
        PathBuf::from(self.snap_root.trim())
    }

    fn refresh(&mut self) {
        let root = self.root();
        self.snap_list = list_snapshots(&root);
        self.snap_folders = list_folders(&root);
        self.snap_loaded = true;
    }
}

impl GridApp {
    /// Sessions in the given scope: all, the ones the grid currently shows, or the selected one.
    pub(crate) fn data_io_sessions(&self, scope: Scope) -> Vec<SessionRecord> {
        match scope {
            Scope::All => self.model.rows.clone(),
            Scope::Selected => {
                // every multi-selected session, or the focused one
                let ids: std::collections::HashSet<usize> = self.selected_ids().into_iter().collect();
                self.model.rows.iter().filter(|r| ids.contains(&r.id)).cloned().collect()
            }
            Scope::Visible => {
                let mut vis = self.model.visible();
                if let Some(rule) = &self.filter_rule {
                    vis.retain(|&i| rule.matches(&self.model.rows[i]));
                }
                if let Some(sel_grp) = &self.groups_manager.selected_group {
                    let gb = self.groups_manager.group_by.as_ref().unwrap_or(&crate::g2::GroupBy::Host);
                    vis.retain(|&i| gb.key_for(&self.model.rows[i]) == *sel_grp);
                }
                if let Some(ids) = self.tabs_manager.active_ids() {
                    let ids: std::collections::HashSet<usize> = ids.iter().copied().collect();
                    vis.retain(|&i| ids.contains(&self.model.rows[i].id));
                }
                vis.into_iter().map(|i| self.model.rows[i].clone()).collect()
            }
        }
    }

    /// Put sessions into the grid (replacing or appending), with fresh ids. Returns the count.
    pub(crate) fn data_io_load(&mut self, mut sessions: Vec<SessionRecord>, append: bool, label: &str) -> usize {
        if !append {
            self.model.apply(SessionEvent::Clear);
        }
        assign_fresh_ids(&mut sessions, &self.model.rows);
        let n = sessions.len();
        for s in sessions {
            self.model.apply(SessionEvent::New(s));
        }
        if !append {
            self.source_label = label.to_string();
        }
        n
    }

    /// Toolbar entry: a "File" menu.
    pub fn data_io_menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("File \u{25be}", |ui| {
            let st = &mut self.ext.data;
            if ui.button("Export sessions (HAR / SAZ / .ftap)...").clicked() {
                st.show_export = true;
                ui.close();
            }
            if ui.button("Import / open HAR, SAZ or .ftap...").on_hover_text("Built-in HAR viewer: open a .har without capturing").clicked() {
                st.show_import = true;
                ui.close();
            }
            if ui.button("Session report (HTML / Markdown / PDF)...").clicked() {
                st.show_report = true;
                ui.close();
            }
            ui.separator();
            if ui.button("Snapshots...").clicked() {
                st.show_snapshots = true;
                st.snap_loaded = false;
                ui.close();
            }
            if ui.button("Compare snapshots...").clicked() {
                st.show_compare = true;
                st.snap_loaded = false;
                ui.close();
            }
            if ui.button("Auto-save...").clicked() {
                st.show_autosave = true;
                ui.close();
            }
        });
    }

    /// Windows this module draws each frame, plus the auto-save timer.
    pub fn data_io_windows(&mut self, ctx: &egui::Context) {
        self.data_io_autosave_tick(ctx);
        if self.ext.data.show_export { self.data_io_export_window(ctx); }
        if self.ext.data.show_import { self.data_io_import_window(ctx); }
        if self.ext.data.show_report { self.data_io_report_window(ctx); }
        if self.ext.data.show_snapshots { self.data_io_snapshots_window(ctx); }
        if self.ext.data.show_compare { self.data_io_compare_window(ctx); }
        if self.ext.data.show_autosave { self.data_io_autosave_window(ctx); }
    }

    fn data_io_report_window(&mut self, ctx: &egui::Context) {
        use crate::report::ReportFormat;
        let mut open = true;
        let mut go = false;
        {
            let st = &mut self.ext.data;
            if st.report_path.is_empty() {
                st.report_path = documents_dir().join("ferrotap-report.html").display().to_string();
            }
            if st.report_title.is_empty() {
                st.report_title = "Ferrotap session report".into();
            }
            egui::Window::new("Session report").open(&mut open).default_width(460.0).show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Sessions:");
                    for s in [Scope::All, Scope::Visible, Scope::Selected] {
                        ui.radio_value(&mut st.report_scope, s, s.label());
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Format:");
                    let before = st.report_fmt;
                    for f in ReportFormat::ALL {
                        ui.radio_value(&mut st.report_fmt, f, f.label());
                    }
                    if before != st.report_fmt {
                        st.report_path = PathBuf::from(&st.report_path).with_extension(st.report_fmt.ext()).display().to_string();
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Title:");
                    ui.add(egui::TextEdit::singleline(&mut st.report_title).desired_width(360.0));
                });
                ui.horizontal(|ui| {
                    ui.label("File:");
                    ui.add(egui::TextEdit::singleline(&mut st.report_path).desired_width(360.0));
                });
                ui.checkbox(&mut st.report_redact, "Redact URLs (query parameters) with the Settings > Privacy rules");
                ui.label(RichText::new("Summary, status counts, errors, per-session timings and notes (comments).").small().color(Color32::GRAY));
                if ui.button("Write report").clicked() { go = true; }
                if !st.report_msg.is_empty() { ui.label(RichText::new(&st.report_msg).color(Color32::GRAY)); }
            });
        }
        if go {
            let st = &self.ext.data;
            let mut sessions = self.data_io_sessions(st.report_scope);
            if st.report_redact {
                sessions = redacted(&sessions, &crate::qol::privacy_settings());
            }
            let path = PathBuf::from(st.report_path.trim().trim_matches('"'));
            let msg = if sessions.is_empty() {
                "Nothing to report (no sessions in that scope)".to_string()
            } else {
                match crate::report::write_report(&path, st.report_fmt, st.report_title.trim(), &sessions) {
                    Ok(n) => format!("Wrote a report of {n} session(s) to {}", path.display()),
                    Err(e) => format!("Report failed: {e}"),
                }
            };
            self.ext.data.report_msg = msg;
        }
        if !open { self.ext.data.show_report = false; }
    }

    fn data_io_export_window(&mut self, ctx: &egui::Context) {
        let mut open = true;
        let mut do_export = false;
        {
            let st = &mut self.ext.data;
            if st.export_path.is_empty() {
                st.export_path = documents_dir().join("sessions.har").display().to_string();
            }
            egui::Window::new("Export sessions").open(&mut open).default_width(460.0).show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Sessions:");
                    for s in [Scope::All, Scope::Visible, Scope::Selected] {
                        ui.radio_value(&mut st.export_scope, s, s.label());
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Format:");
                    let before = st.export_fmt;
                    ui.radio_value(&mut st.export_fmt, ExportFormat::Har, "HAR 1.2 (.har)");
                    ui.radio_value(&mut st.export_fmt, ExportFormat::Ftap, "Ferrotap (.ftap)");
                    ui.radio_value(&mut st.export_fmt, ExportFormat::Saz, "SAZ archive (.saz)");
                    ui.radio_value(&mut st.export_fmt, ExportFormat::Postman, "Postman collection (.json)");
                    if before != st.export_fmt {
                        let p = PathBuf::from(&st.export_path).with_extension(st.export_fmt.ext());
                        st.export_path = p.display().to_string();
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("File:");
                    ui.add(egui::TextEdit::singleline(&mut st.export_path).desired_width(360.0));
                });
                ui.checkbox(&mut st.export_redact, "Redacted copy (mask with the Settings > Privacy rules)")
                    .on_hover_text("Headers, cookies, query parameters and body patterns from the privacy settings are replaced in the exported file; the captured sessions are not changed.");
                if ui.button("Export").clicked() { do_export = true; }
                if !st.export_msg.is_empty() { ui.label(RichText::new(&st.export_msg).color(Color32::GRAY)); }
            });
        }
        if do_export {
            let st = &self.ext.data;
            let mut sessions = self.data_io_sessions(st.export_scope);
            if st.export_redact {
                sessions = redacted(&sessions, &crate::qol::privacy_settings());
            }
            let path = PathBuf::from(st.export_path.trim().trim_matches('"'));
            let fmt = st.export_fmt;
            let msg = if sessions.is_empty() {
                "Nothing to export (no sessions in that scope)".to_string()
            } else {
                match export_sessions(&path, fmt, &sessions) {
                    Ok(n) => format!("Exported {n} session(s) to {}", path.display()),
                    Err(e) => format!("Export failed: {e}"),
                }
            };
            self.ext.data.export_msg = msg;
        }
        if !open { self.ext.data.show_export = false; }
    }

    fn data_io_import_window(&mut self, ctx: &egui::Context) {
        let mut open = true;
        let mut do_import = false;
        {
            let st = &mut self.ext.data;
            egui::Window::new("Import / open sessions").open(&mut open).default_width(460.0).show(ctx, |ui| {
                ui.label(RichText::new("Open a .har, .saz, Postman collection (.json) or .ftap file, or a folder of them. Imported sessions get new ids.").small().color(Color32::GRAY));
                ui.horizontal(|ui| {
                    ui.label("Path:");
                    let hint = documents_dir().join("file.har").display().to_string();
                    let r = ui.add(egui::TextEdit::singleline(&mut st.import_path).desired_width(360.0).hint_text(hint));
                    if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) { do_import = true; }
                });
                ui.horizontal(|ui| {
                    ui.radio_value(&mut st.import_append, false, "Replace grid");
                    ui.radio_value(&mut st.import_append, true, "Append to grid");
                });
                if ui.button("Open").clicked() { do_import = true; }
                if !st.import_msg.is_empty() { ui.label(RichText::new(&st.import_msg).color(Color32::GRAY)); }
            });
        }
        if do_import {
            let path = PathBuf::from(self.ext.data.import_path.trim().trim_matches('"'));
            let append = self.ext.data.import_append;
            let msg = match import_path(&path) {
                Ok(v) => {
                    let label = format!("file: {}", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
                    let n = self.data_io_load(v, append, &label);
                    format!("Loaded {n} session(s) from {}", path.display())
                }
                Err(e) => format!("Import failed: {e}"),
            };
            self.ext.data.import_msg = msg;
        }
        if !open { self.ext.data.show_import = false; }
    }

    fn data_io_snapshots_window(&mut self, ctx: &egui::Context) {
        if !self.ext.data.snap_loaded { self.ext.data.refresh(); }
        let mut open = true;
        // Actions collected inside the window closure, run after it (they need &mut self).
        let mut save: Option<Scope> = None;
        let mut load: Option<(PathBuf, bool)> = None;
        {
            let st = &mut self.ext.data;
            let root = st.root();
            egui::Window::new("Snapshots").open(&mut open).default_width(640.0).default_height(420.0).show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Root:");
                    let r = ui.add(egui::TextEdit::singleline(&mut st.snap_root).desired_width(420.0));
                    if r.lost_focus() || ui.button("Refresh").clicked() { st.snap_loaded = false; }
                });
                ui.horizontal(|ui| {
                    ui.label("Folder:");
                    let shown = if st.snap_folder.is_empty() { "(root)".to_string() } else { st.snap_folder.clone() };
                    egui::ComboBox::from_id_salt("dio_folder").selected_text(shown).show_ui(ui, |ui| {
                        ui.selectable_value(&mut st.snap_folder, String::new(), "(root)");
                        for f in st.snap_folders.clone() {
                            ui.selectable_value(&mut st.snap_folder, f.clone(), f);
                        }
                    });
                    ui.add(egui::TextEdit::singleline(&mut st.snap_new_folder).desired_width(140.0).hint_text("new folder"));
                    if ui.button("Create folder").clicked() {
                        st.snap_msg = match create_folder(&root, &st.snap_new_folder) {
                            Ok(_) => {
                                st.snap_folder = clean_name(&st.snap_new_folder).unwrap_or_default();
                                st.snap_new_folder.clear();
                                st.snap_loaded = false;
                                "Folder created".to_string()
                            }
                            Err(e) => format!("Create folder failed: {e}"),
                        };
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Save");
                    egui::ComboBox::from_id_salt("dio_scope").selected_text(st.snap_scope.label()).show_ui(ui, |ui| {
                        for s in [Scope::All, Scope::Visible, Scope::Selected] {
                            ui.selectable_value(&mut st.snap_scope, s, s.label());
                        }
                    });
                    ui.label("sessions as");
                    ui.add(egui::TextEdit::singleline(&mut st.snap_name).desired_width(200.0).hint_text("snapshot name"));
                    if ui.button("Save snapshot").clicked() { save = Some(st.snap_scope); }
                });
                ui.horizontal(|ui| {
                    ui.label("Password");
                    ui.add(egui::TextEdit::singleline(&mut st.snap_password).password(true).desired_width(160.0).hint_text("for protected snapshots"));
                    ui.checkbox(&mut st.snap_protect, "protect new snapshots with it");
                });
                ui.separator();
                if st.snap_list.is_empty() {
                    ui.label(RichText::new("No snapshots yet").italics().color(Color32::GRAY));
                }
                let list = st.snap_list.clone();
                egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                    egui::Grid::new("dio_snaps").striped(true).num_columns(3).show(ui, |ui| {
                        for s in &list {
                            let renaming = st.snap_rename.as_ref().map(|(p, _)| p == &s.path).unwrap_or(false);
                            if renaming {
                                let mut done = false;
                                if let Some((_, text)) = st.snap_rename.as_mut() {
                                    let r = ui.add(egui::TextEdit::singleline(text).desired_width(200.0));
                                    if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) { done = true; }
                                }
                                ui.horizontal(|ui| {
                                    if ui.button("OK").clicked() { done = true; }
                                    if ui.button("Cancel").clicked() { st.snap_rename = None; }
                                });
                                if done {
                                    if let Some((p, text)) = st.snap_rename.take() {
                                        st.snap_msg = match rename_snapshot(&p, &text) {
                                            Ok(_) => "Renamed".to_string(),
                                            Err(e) => format!("Rename failed: {e}"),
                                        };
                                        st.snap_loaded = false;
                                    }
                                }
                            } else {
                                ui.label(RichText::new(if s.locked { format!("{} (locked)", s.label()) } else { s.label() }).strong())
                                    .on_hover_text(if s.locked { "Password protected: type the password above to open it" } else { "" });
                                ui.label(RichText::new(format!("{}  {} KB", s.modified, s.bytes.div_ceil(1024))).small().color(Color32::GRAY));
                            }
                            ui.horizontal(|ui| {
                                if ui.button("Open").on_hover_text("Replace the grid with this snapshot").clicked() { load = Some((s.path.clone(), false)); }
                                if ui.button("Append").on_hover_text("Add this snapshot's sessions to the grid").clicked() { load = Some((s.path.clone(), true)); }
                                if ui.button("Rename").clicked() { st.snap_rename = Some((s.path.clone(), s.name.clone())); }
                                if ui.button("Password...").on_hover_text("Set, change or remove this snapshot's password").clicked() {
                                    st.snap_pw_for = if st.snap_pw_for.as_ref() == Some(&s.path) { None } else { Some(s.path.clone()) };
                                    st.snap_new_password.clear();
                                }
                                if ui.button("Clone").clicked() {
                                    st.snap_msg = match clone_snapshot(&s.path) {
                                        Ok(p) => format!("Cloned to {}", p.file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default()),
                                        Err(e) => format!("Clone failed: {e}"),
                                    };
                                    st.snap_loaded = false;
                                }
                                if st.snap_confirm_delete.as_ref() == Some(&s.path) {
                                    if ui.button(RichText::new("Confirm delete").color(Color32::LIGHT_RED)).clicked() {
                                        st.snap_msg = match delete_snapshot(&s.path) {
                                            Ok(()) => "Deleted".to_string(),
                                            Err(e) => format!("Delete failed: {e}"),
                                        };
                                        st.snap_confirm_delete = None;
                                        st.snap_loaded = false;
                                    }
                                } else if ui.button("Delete").clicked() {
                                    st.snap_confirm_delete = Some(s.path.clone());
                                }
                            });
                            ui.end_row();
                            if st.snap_pw_for.as_ref() == Some(&s.path) {
                                ui.label(RichText::new(if s.locked { "Current password: the Password field above" } else { "Not protected yet" }).small().color(Color32::GRAY));
                                ui.horizontal(|ui| {
                                    ui.add(egui::TextEdit::singleline(&mut st.snap_new_password).password(true).desired_width(160.0).hint_text("new password"));
                                    let old = (!st.snap_password.is_empty()).then(|| st.snap_password.clone());
                                    if ui.add_enabled(!st.snap_new_password.is_empty(), egui::Button::new(if s.locked { "Change" } else { "Set" })).clicked() {
                                        st.snap_msg = match crate::snapcrypt::set_password(&s.path, old.as_deref(), Some(&st.snap_new_password)) {
                                            Ok(()) => format!("'{}' is password protected", s.name),
                                            Err(e) => format!("Password not changed: {e}"),
                                        };
                                        st.snap_pw_for = None;
                                        st.snap_loaded = false;
                                    }
                                    if s.locked && ui.button("Remove password").clicked() {
                                        st.snap_msg = match crate::snapcrypt::set_password(&s.path, old.as_deref(), None) {
                                            Ok(()) => format!("'{}' is no longer protected", s.name),
                                            Err(e) => format!("Password not removed: {e}"),
                                        };
                                        st.snap_pw_for = None;
                                        st.snap_loaded = false;
                                    }
                                });
                                ui.end_row();
                            }
                        }
                    });
                });
                if !st.snap_msg.is_empty() { ui.label(RichText::new(&st.snap_msg).color(Color32::GRAY)); }
            });
        }
        if let Some(scope) = save {
            let sessions = self.data_io_sessions(scope);
            let st = &mut self.ext.data;
            let root = st.root();
            st.snap_msg = if sessions.is_empty() {
                "Nothing to save (no sessions in that scope)".to_string()
            } else {
                let name = if st.snap_name.trim().is_empty() {
                    chrono::Local::now().format("Snapshot %Y-%m-%d %H-%M-%S").to_string()
                } else {
                    st.snap_name.trim().to_string()
                };
                match save_snapshot(&root, &st.snap_folder, &name, &sessions) {
                    Ok(p) if st.snap_protect && !st.snap_password.is_empty() => match crate::snapcrypt::set_password(&p, None, Some(&st.snap_password)) {
                        Ok(()) => { st.snap_name.clear(); format!("Saved {} session(s) as '{name}' (password protected)", sessions.len()) }
                        Err(e) => { let _ = std::fs::remove_file(&p); format!("Save failed: {e}") }
                    },
                    Ok(_) => { st.snap_name.clear(); format!("Saved {} session(s) as '{name}'", sessions.len()) }
                    Err(e) => format!("Save failed: {e}"),
                }
            };
            st.snap_loaded = false;
        }
        if let Some((path, append)) = load {
            let pw = (!self.ext.data.snap_password.is_empty()).then(|| self.ext.data.snap_password.clone());
            let msg = match crate::snapcrypt::load(&path, pw.as_deref()) {
                Ok(v) => {
                    let label = format!("snapshot: {}", path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
                    let n = self.data_io_load(v, append, &label);
                    format!("{} {n} session(s)", if append { "Appended" } else { "Opened" })
                }
                Err(e) => format!("Open failed: {e}"),
            };
            self.ext.data.snap_msg = msg;
        }
        if !open { self.ext.data.show_snapshots = false; }
    }

    fn data_io_compare_window(&mut self, ctx: &egui::Context) {
        if !self.ext.data.snap_loaded { self.ext.data.refresh(); }
        let mut open = true;
        let mut run = false;
        {
            let st = &mut self.ext.data;
            if st.cmp_tol.is_empty() { st.cmp_tol = "0".to_string(); }
            let list = st.snap_list.clone();
            let name_of = |p: &Option<PathBuf>, none: &str| -> String {
                p.as_ref()
                    .and_then(|p| list.iter().find(|s| &s.path == p))
                    .map(|s| s.label())
                    .unwrap_or_else(|| none.to_string())
            };
            let a_text = name_of(&st.cmp_a, "(choose)");
            let b_text = name_of(&st.cmp_b, "(current sessions)");
            egui::Window::new("Compare snapshots").open(&mut open).default_width(640.0).default_height(440.0).show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("A (old):");
                    egui::ComboBox::from_id_salt("dio_cmp_a").width(200.0).selected_text(a_text).show_ui(ui, |ui| {
                        for s in &list { ui.selectable_value(&mut st.cmp_a, Some(s.path.clone()), s.label()); }
                    });
                    ui.label("B (new):");
                    egui::ComboBox::from_id_salt("dio_cmp_b").width(200.0).selected_text(b_text).show_ui(ui, |ui| {
                        ui.selectable_value(&mut st.cmp_b, None, "(current sessions)");
                        for s in &list { ui.selectable_value(&mut st.cmp_b, Some(s.path.clone()), s.label()); }
                    });
                });
                ui.horizontal(|ui| {
                    ui.label("Ignore duration changes up to (ms):");
                    ui.add(egui::TextEdit::singleline(&mut st.cmp_tol).desired_width(60.0));
                    if ui.button("Compare").clicked() { run = true; }
                    if ui.button("Refresh list").clicked() { st.snap_loaded = false; }
                });
                if !st.cmp_msg.is_empty() { ui.label(RichText::new(&st.cmp_msg).color(Color32::GRAY)); }
                if let Some(r) = &st.cmp_result {
                    ui.label(format!("{} added, {} removed, {} changed, {} unchanged", r.added.len(), r.removed.len(), r.changed.len(), r.same));
                    ui.separator();
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.label(RichText::new(format!("Added ({})", r.added.len())).strong().color(Color32::LIGHT_GREEN));
                        for (m, u, s) in &r.added { ui.label(RichText::new(format!("+ {m} {u}  [{s}]")).monospace()); }
                        ui.label(RichText::new(format!("Removed ({})", r.removed.len())).strong().color(Color32::LIGHT_RED));
                        for (m, u, s) in &r.removed { ui.label(RichText::new(format!("- {m} {u}  [{s}]")).monospace()); }
                        ui.label(RichText::new(format!("Changed ({})", r.changed.len())).strong().color(Color32::from_rgb(240, 200, 80)));
                        for c in &r.changed {
                            ui.label(RichText::new(format!(
                                "~ {} {}  status {} -> {}, size {} -> {}, time {} -> {} ms  ({})",
                                c.method, c.url, c.old.0, c.new.0, c.old.1, c.new.1, c.old.2, c.new.2, c.what.join(", ")
                            )).monospace());
                        }
                    });
                }
            });
        }
        if run {
            let tol: u128 = self.ext.data.cmp_tol.trim().parse().unwrap_or(0);
            let a = self.ext.data.cmp_a.clone();
            let b = self.ext.data.cmp_b.clone();
            let current = &self.model.rows;
            let res = (|| -> Result<CompareResult, String> {
                let a = load_snapshot(&a.ok_or("choose snapshot A")?)?;
                let b = match b { Some(p) => load_snapshot(&p)?, None => current.clone() };
                Ok(compare_sessions(&a, &b, tol))
            })();
            match res {
                Ok(r) => { self.ext.data.cmp_result = Some(r); self.ext.data.cmp_msg.clear(); }
                Err(e) => { self.ext.data.cmp_result = None; self.ext.data.cmp_msg = format!("Compare failed: {e}"); }
            }
        }
        if !open { self.ext.data.show_compare = false; }
    }

    fn data_io_autosave_window(&mut self, ctx: &egui::Context) {
        let mut open = true;
        let st = &mut self.ext.data;
        let root = st.root();
        if !st.autosave_loaded {
            st.autosave = AutoSaveCfg::load(&root);
            st.autosave_loaded = true;
        }
        let before = st.autosave.clone();
        egui::Window::new("Auto-save").open(&mut open).default_width(440.0).show(ctx, |ui| {
            ui.checkbox(&mut st.autosave.enabled, "Save all sessions automatically");
            ui.horizontal(|ui| {
                ui.label("Every");
                ui.add(egui::DragValue::new(&mut st.autosave.interval_min).range(1..=1440));
                ui.label("minutes, keep the last");
                ui.add(egui::DragValue::new(&mut st.autosave.keep).range(1..=1000));
                ui.label("files");
            });
            ui.horizontal(|ui| {
                ui.label("Folder:");
                let hint = st.autosave.dir(&root).display().to_string();
                ui.add(egui::TextEdit::singleline(&mut st.autosave.folder).desired_width(320.0).hint_text(hint));
            });
            if let (Some(last), true) = (st.autosave_last, st.autosave.enabled) {
                let due = (st.autosave.interval_min as u64 * 60).saturating_sub(last.elapsed().as_secs());
                ui.label(RichText::new(format!("Next save in {}:{:02}", due / 60, due % 60)).small().color(Color32::GRAY));
            }
            if !st.autosave_msg.is_empty() { ui.label(RichText::new(&st.autosave_msg).color(Color32::GRAY)); }
        });
        if st.autosave != before {
            if st.autosave.enabled && !before.enabled { st.autosave_last = Some(Instant::now()); }
            if let Err(e) = st.autosave.save(&root) { st.autosave_msg = format!("Could not save settings: {e}"); }
        }
        if !open { st.show_autosave = false; }
    }

    fn data_io_autosave_tick(&mut self, ctx: &egui::Context) {
        if !self.ext.data.autosave_loaded {
            let root = self.ext.data.root();
            self.ext.data.autosave = AutoSaveCfg::load(&root);
            self.ext.data.autosave_loaded = true;
        }
        if let Some(rx) = &self.ext.data.autosave_job {
            match rx.try_recv() {
                Ok(m) => {
                    self.ext.data.autosave_msg = m;
                    self.ext.data.autosave_job = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(250));
                    return;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.ext.data.autosave_job = None,
            }
        }
        let cfg = self.ext.data.autosave.clone();
        if !cfg.enabled { return; }
        let period = std::time::Duration::from_secs(cfg.interval_min.max(1) as u64 * 60);
        let last = *self.ext.data.autosave_last.get_or_insert_with(Instant::now);
        let elapsed = last.elapsed();
        if elapsed < period {
            ctx.request_repaint_after(period - elapsed);
            return;
        }
        self.ext.data.autosave_last = Some(Instant::now());
        if self.model.rows.is_empty() { return; }
        let root = self.ext.data.root();
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
        // Copy the sessions here (fast), write them on a thread (slow) so the window keeps going.
        let rows = self.model.rows.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        self.ext.data.autosave_job = Some(rx);
        self.ext.data.autosave_msg = format!("Auto-saving {} session(s)...", rows.len());
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let msg = match autosave_write(&cfg.dir(&root), &stamp, &rows, cfg.keep) {
                Ok(p) => format!("Auto-saved {} session(s) to {}", rows.len(), p.display()),
                Err(e) => format!("Auto-save failed: {e}"),
            };
            let _ = tx.send(msg);
            repaint.request_repaint();
        });
    }
}
