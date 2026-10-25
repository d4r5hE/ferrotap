//! G1 "Compare two sessions": request line / status / timing side by side, header differences with an
//! ignore-headers list, and a line diff of the bodies.
use std::collections::{BTreeMap, BTreeSet};

use eframe::egui::{self, Color32, RichText};

use crate::model::SessionRecord;
use crate::ui::GridApp;

/// Headers that differ between almost any two responses; ignored by default (editable in the window).
pub const DEFAULT_IGNORE: &str = "Date, Age, Expires, Last-Modified, ETag, X-Request-Id, X-Correlation-Id, Server-Timing, CF-Ray, Set-Cookie";

pub struct CompareState {
    pub open: bool,
    /// The session picked with "Set as compare left" (waiting for a second one).
    pub left: Option<usize>,
    /// The two sessions being compared.
    pub pair: Option<(usize, usize)>,
    pub ignore: String,
    /// Body diffs (request, response) of the pair, keyed by ids and body lengths: the diff is
    /// quadratic, so it is not redone every frame.
    pub body_diffs: Option<(BodyKey, std::sync::Arc<(Vec<DiffLine>, Vec<DiffLine>)>)>,
}

pub type BodyKey = (usize, usize, usize, usize, usize, usize);

impl Default for CompareState {
    fn default() -> Self {
        Self { open: false, left: None, pair: None, ignore: DEFAULT_IGNORE.to_string(), body_diffs: None }
    }
}

/// One header that differs: present only on the left, only on the right, or with different values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderDiff {
    pub name: String,
    pub left: Option<String>,
    pub right: Option<String>,
}

/// Names from a comma/space separated list, lower-cased.
pub fn ignore_set(list: &str) -> BTreeSet<String> {
    list.split([',', ';', '\n']).map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty()).collect()
}

/// The headers that differ (case-insensitive names), skipping ignored ones, sorted by name.
pub fn header_diff(a: &BTreeMap<String, String>, b: &BTreeMap<String, String>, ignore: &BTreeSet<String>) -> Vec<HeaderDiff> {
    let low = |m: &BTreeMap<String, String>| -> BTreeMap<String, (String, String)> {
        m.iter().map(|(k, v)| (k.to_ascii_lowercase(), (k.clone(), v.clone()))).collect()
    };
    let (la, lb) = (low(a), low(b));
    let names: BTreeSet<&String> = la.keys().chain(lb.keys()).collect();
    let mut out = Vec::new();
    for n in names {
        if ignore.contains(n) {
            continue;
        }
        let (x, y) = (la.get(n), lb.get(n));
        if x.map(|v| &v.1) == y.map(|v| &v.1) {
            continue;
        }
        let name = x.or(y).map(|v| v.0.clone()).unwrap_or_else(|| n.clone());
        out.push(HeaderDiff { name, left: x.map(|v| v.1.clone()), right: y.map(|v| v.1.clone()) });
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffLine {
    Same(String),
    /// Only in the left body.
    Left(String),
    /// Only in the right body.
    Right(String),
}

/// Bodies longer than this many lines are compared only up to it (the diff is quadratic).
pub const MAX_DIFF_LINES: usize = 3000;

/// A line diff (longest common subsequence) of two texts.
pub fn line_diff(a: &str, b: &str) -> Vec<DiffLine> {
    let x: Vec<&str> = a.lines().take(MAX_DIFF_LINES).collect();
    let y: Vec<&str> = b.lines().take(MAX_DIFF_LINES).collect();
    let (n, m) = (x.len(), y.len());
    // lcs[i][j] = LCS length of x[i..] and y[j..], in one flat table
    let w = m + 1;
    let mut lcs = vec![0u32; (n + 1) * w];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i * w + j] = if x[i] == y[j] {
                lcs[(i + 1) * w + j + 1] + 1
            } else {
                lcs[(i + 1) * w + j].max(lcs[i * w + j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    while i < n && j < m {
        if x[i] == y[j] {
            out.push(DiffLine::Same(x[i].to_string()));
            i += 1;
            j += 1;
        } else if lcs[(i + 1) * w + j] >= lcs[i * w + j + 1] {
            out.push(DiffLine::Left(x[i].to_string()));
            i += 1;
        } else {
            out.push(DiffLine::Right(y[j].to_string()));
            j += 1;
        }
    }
    out.extend(x[i..].iter().map(|l| DiffLine::Left(l.to_string())));
    out.extend(y[j..].iter().map(|l| DiffLine::Right(l.to_string())));
    out
}

/// JSON and XML/HTML bodies are compared pretty-printed, so a minified one-line reply still diffs
/// line by line instead of showing the whole body as a single changed line.
pub fn diffable_body(body: &str) -> String {
    let t = body.trim_start();
    if t.starts_with(['{', '[']) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
            return serde_json::to_string_pretty(&v).unwrap_or_else(|_| body.to_string());
        }
    }
    if t.starts_with('<') {
        let low = t.to_ascii_lowercase();
        let html = low.starts_with("<!doctype html") || low.starts_with("<html");
        return crate::inspect::pretty_markup(body, html);
    }
    body.to_string()
}

fn diff_lines_ui(ui: &mut egui::Ui, id: &str, lines: &[DiffLine]) {
    let changed = lines.iter().filter(|l| !matches!(l, DiffLine::Same(_))).count();
    if changed == 0 {
        ui.label(RichText::new("Identical").color(Color32::GRAY));
        return;
    }
    egui::ScrollArea::vertical().id_salt(id).max_height(220.0).show(ui, |ui| {
        for l in lines {
            let (prefix, text, color) = match l {
                DiffLine::Same(t) => ("  ", t, Color32::GRAY),
                DiffLine::Left(t) => ("- ", t, Color32::from_rgb(230, 110, 110)),
                DiffLine::Right(t) => ("+ ", t, Color32::from_rgb(110, 200, 120)),
            };
            ui.label(RichText::new(format!("{prefix}{text}")).monospace().color(color));
        }
    });
}

fn headers_diff_ui(ui: &mut egui::Ui, id: &str, title: &str, diffs: &[HeaderDiff]) {
    ui.label(RichText::new(format!("{title}: {} difference(s)", diffs.len())).strong());
    if diffs.is_empty() {
        return;
    }
    egui::Grid::new(id).striped(true).num_columns(3).show(ui, |ui| {
        ui.label(RichText::new("Header").strong());
        ui.label(RichText::new("Left").strong());
        ui.label(RichText::new("Right").strong());
        ui.end_row();
        for d in diffs {
            ui.label(&d.name);
            ui.label(d.left.as_deref().unwrap_or("(missing)"));
            ui.label(d.right.as_deref().unwrap_or("(missing)"));
            ui.end_row();
        }
    });
}

impl GridApp {
    /// Open the compare window for two sessions.
    pub fn compare_sessions(&mut self, a: usize, b: usize) {
        self.ext.compare.pair = Some((a.min(b), a.max(b)));
        self.ext.compare.left = None;
        self.ext.compare.open = true;
    }

    pub fn compare_window(&mut self, ctx: &egui::Context) {
        let st = &mut self.ext.compare;
        if !st.open {
            return;
        }
        let Some((ia, ib)) = st.pair else {
            st.open = false;
            return;
        };
        let rows = &self.model.rows;
        let find = |id: usize| rows.iter().find(|r| r.id == id);
        let (Some(a), Some(b)) = (find(ia), find(ib)) else {
            self.ext.compare.open = false;
            return;
        };
        let mut open = true;
        let st = &mut self.ext.compare;
        let key = (a.id, b.id, a.request_body.len(), b.request_body.len(), a.response_body.len(), b.response_body.len());
        let diffs = match &st.body_diffs {
            Some((k, d)) if *k == key => d.clone(),
            _ => {
                let d = std::sync::Arc::new((
                    line_diff(&diffable_body(&a.request_body), &diffable_body(&b.request_body)),
                    line_diff(&diffable_body(&a.response_body), &diffable_body(&b.response_body)),
                ));
                st.body_diffs = Some((key, d.clone()));
                d
            }
        };
        egui::Window::new(format!("Compare #{} and #{}", a.id, b.id)).open(&mut open).default_width(720.0).show(ctx, |ui| {
            let side = |s: &SessionRecord| {
                format!("#{}  {} {}\n{}  {}  {} ms  {} B", s.id, s.method, s.url, s.status, s.content_type(), s.duration_ms, s.body_size())
            };
            ui.columns(2, |c| {
                c[0].label(RichText::new(side(a)).monospace());
                c[1].label(RichText::new(side(b)).monospace());
            });
            ui.horizontal(|ui| {
                ui.label("Ignore headers");
                ui.add(egui::TextEdit::singleline(&mut st.ignore).desired_width(520.0));
            });
            let ignore = ignore_set(&st.ignore);
            ui.separator();
            headers_diff_ui(ui, "cmp_req_h", "Request headers", &header_diff(&a.request_headers, &b.request_headers, &ignore));
            headers_diff_ui(ui, "cmp_resp_h", "Response headers", &header_diff(&a.response_headers, &b.response_headers, &ignore));
            ui.separator();
            ui.label(RichText::new("Request body").strong());
            diff_lines_ui(ui, "cmp_req_b", &diffs.0);
            ui.label(RichText::new("Response body").strong());
            diff_lines_ui(ui, "cmp_resp_b", &diffs.1);
        });
        if !open {
            self.ext.compare.open = false;
        }
    }
}
