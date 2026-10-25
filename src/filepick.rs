//! A small in-app file / folder picker (no OS dialog dependency): browse drives and folders,
//! pick a file (optionally filtered by extension) or the current folder. The caller opens it with
//! a `purpose` tag and collects the result with `take(purpose)`.
use std::path::{Path, PathBuf};

use eframe::egui::{self, Color32, RichText};

#[derive(Default)]
pub struct FilePicker {
    pub open: bool,
    pub dir: PathBuf,
    /// Lower-case extensions to show (empty = all files).
    pub exts: Vec<String>,
    /// Pick a folder instead of a file.
    pub folder: bool,
    pub purpose: String,
    pub title: String,
    result: Option<(String, PathBuf)>,
    typed: String,
}

/// Folders, then files (matching `exts`), each sorted case-insensitively.
pub fn list_dir(dir: &Path, exts: &[String]) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                dirs.push(p);
            } else {
                let ext = p.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
                if exts.is_empty() || exts.contains(&ext) {
                    files.push(p);
                }
            }
        }
    }
    let key = |p: &PathBuf| p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    dirs.sort_by_key(key);
    files.sort_by_key(key);
    (dirs, files)
}

/// Existing drive roots (Windows) or "/" elsewhere.
pub fn roots() -> Vec<PathBuf> {
    if cfg!(windows) {
        (b'A'..=b'Z').map(|c| PathBuf::from(format!("{}:\\", c as char))).filter(|p| p.exists()).collect()
    } else {
        vec![PathBuf::from("/")]
    }
}

impl FilePicker {
    /// Open for a file with one of `exts` (e.g. ["json"]), starting at `start` (a file or a folder).
    pub fn open_file(&mut self, purpose: &str, title: &str, start: &str, exts: &[&str]) {
        self.begin(purpose, title, start);
        self.folder = false;
        self.exts = exts.iter().map(|e| e.to_ascii_lowercase()).collect();
    }

    /// Open for a folder.
    pub fn open_folder(&mut self, purpose: &str, title: &str, start: &str) {
        self.begin(purpose, title, start);
        self.folder = true;
        self.exts.clear();
    }

    fn begin(&mut self, purpose: &str, title: &str, start: &str) {
        let s = PathBuf::from(start.trim().trim_matches('"'));
        let dir = if s.is_dir() { s } else { s.parent().map(Path::to_path_buf).filter(|p| p.is_dir()).unwrap_or_else(crate::data_io::documents_dir) };
        self.dir = dir;
        self.typed = self.dir.display().to_string();
        self.purpose = purpose.to_string();
        self.title = title.to_string();
        self.result = None;
        self.open = true;
    }

    /// The picked path, once, if the picker was opened for `purpose`.
    pub fn take(&mut self, purpose: &str) -> Option<PathBuf> {
        if self.result.as_ref().is_some_and(|(p, _)| p == purpose) {
            return self.result.take().map(|(_, p)| p);
        }
        None
    }

    pub fn show(&mut self, ctx: &egui::Context) {
        if !self.open {
            return;
        }
        let mut open = true;
        let mut go: Option<PathBuf> = None;
        let mut picked: Option<PathBuf> = None;
        let (dirs, files) = list_dir(&self.dir, &self.exts);
        egui::Window::new(if self.title.is_empty() { "Choose a file" } else { &self.title })
            .id(egui::Id::new("ferrotap_file_picker"))
            .open(&mut open)
            .default_size([520.0, 420.0])
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for r in roots() {
                        if ui.small_button(r.display().to_string()).clicked() {
                            go = Some(r);
                        }
                    }
                    if ui.small_button("Documents").clicked() {
                        go = Some(crate::data_io::documents_dir());
                    }
                    if let Some(p) = self.dir.parent() {
                        if ui.small_button("Up").clicked() {
                            go = Some(p.to_path_buf());
                        }
                    }
                });
                ui.horizontal(|ui| {
                    let r = ui.add(egui::TextEdit::singleline(&mut self.typed).desired_width(400.0));
                    if (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) || ui.button("Go").clicked() {
                        let p = PathBuf::from(self.typed.trim().trim_matches('"'));
                        if p.is_dir() {
                            go = Some(p);
                        } else if p.is_file() && !self.folder {
                            picked = Some(p);
                        }
                    }
                });
                if !self.exts.is_empty() {
                    ui.label(RichText::new(format!("Showing: {}", self.exts.iter().map(|e| format!("*.{e}")).collect::<Vec<_>>().join(", "))).small().color(Color32::GRAY));
                }
                ui.separator();
                egui::ScrollArea::vertical().max_height(300.0).auto_shrink([false, false]).show(ui, |ui| {
                    for d in &dirs {
                        let name = d.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        if ui.selectable_label(false, format!("\u{1F4C1} {name}")).clicked() {
                            go = Some(d.clone());
                        }
                    }
                    if !self.folder {
                        for f in &files {
                            let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                            if ui.selectable_label(false, name).double_clicked() {
                                picked = Some(f.clone());
                            }
                        }
                    }
                    if dirs.is_empty() && (self.folder || files.is_empty()) {
                        ui.label(RichText::new("(empty)").italics());
                    }
                });
                ui.separator();
                if self.folder {
                    if ui.button(format!("Use this folder: {}", self.dir.display())).clicked() {
                        picked = Some(self.dir.clone());
                    }
                } else {
                    ui.label(RichText::new("Double-click a file to choose it.").small().color(Color32::GRAY));
                }
            });
        if let Some(d) = go {
            self.typed = d.display().to_string();
            self.dir = d;
        }
        if let Some(p) = picked {
            self.result = Some((self.purpose.clone(), p));
            open = false;
        }
        self.open = open;
    }
}
