//! Persistent UI state (data/ui_state.json): column visibility/width, filter + search settings,
//! and per-session marks/comments (keyed by id + url so stale ids from older runs never mis-apply).
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::{ColumnState, FilterCriteria, SessionMark, SessionModel, SessionRecord};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Annotation {
    pub id: usize,
    pub url: String,
    #[serde(default)]
    pub mark: Option<SessionMark>,
    #[serde(default)]
    pub comment: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct UiPrefs {
    pub columns: ColumnState,
    pub filter: String,
    pub method: String,
    pub status: String,
    pub url: String,
    pub search_query: String,
    pub search_in_headers: bool,
    pub search_in_bodies: bool,
    pub search_case_sensitive: bool,
    pub annotations: Vec<Annotation>,
    pub custom_columns: Vec<crate::g2::CustomColumnDef>,
    pub pinned_groups: Vec<String>,
    /// Keep only this many sessions (oldest unmarked ones go first); 0 = keep all.
    pub keep_sessions: usize,
}

impl Default for UiPrefs {
    fn default() -> Self {
        Self {
            columns: ColumnState::default(),
            filter: String::new(),
            method: String::new(),
            status: String::new(),
            url: String::new(),
            search_query: String::new(),
            search_in_headers: true,
            search_in_bodies: true,
            search_case_sensitive: false,
            annotations: Vec::new(),
            custom_columns: Vec::new(),
            pinned_groups: Vec::new(),
            keep_sessions: 0,
        }
    }
}

impl UiPrefs {
    pub fn default_path() -> PathBuf {
        std::env::var("FERROTAP_UI_STATE").map(PathBuf::from).unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data").join("ui_state.json")
        })
    }

    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        let j = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, j).map_err(|e| e.to_string())
    }

    /// Structured criteria from the toolbar text boxes (+ the mark combo).
    pub fn criteria(&self, mark: Option<SessionMark>) -> FilterCriteria {
        let nz = |s: &str| {
            let t = s.trim();
            if t.is_empty() { None } else { Some(t.to_string()) }
        };
        FilterCriteria {
            method: nz(&self.method),
            status: self.status.trim().parse::<u16>().ok(),
            url_substring: nz(&self.url),
            mark,
            ..Default::default()
        }
    }

    /// Record (or drop) the stored annotation for this row after a mark/comment change.
    pub fn sync_row(&mut self, r: &SessionRecord) {
        self.annotations.retain(|a| !(a.id == r.id && a.url == r.url));
        if r.mark.is_some() || !r.comment.trim().is_empty() {
            self.annotations.push(Annotation { id: r.id, url: r.url.clone(), mark: r.mark, comment: r.comment.clone() });
        }
    }

    /// Re-apply a stored mark/comment to a freshly captured session.
    pub fn apply_to(&self, r: &mut SessionRecord) {
        if let Some(a) = self.annotations.iter().find(|a| a.id == r.id && a.url == r.url) {
            r.mark = a.mark;
            r.comment = a.comment.clone();
        }
    }

    pub fn from_model(&mut self, m: &SessionModel) {
        self.columns = m.column_state.clone();
        self.filter = m.filter.clone();
        self.search_query = m.search_query.clone();
        self.search_in_headers = m.search_in_headers;
        self.search_in_bodies = m.search_in_bodies;
        self.search_case_sensitive = m.search_case_sensitive;
    }

    pub fn to_model(&self, m: &mut SessionModel) {
        m.column_state = self.columns.clone();
        m.column_state.normalize();
        m.filter = self.filter.clone();
        m.search_query = self.search_query.clone();
        m.search_in_headers = self.search_in_headers;
        m.search_in_bodies = self.search_in_bodies;
        m.search_case_sensitive = self.search_case_sensitive;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{GridColumn, SessionEvent};

    fn rec(id: usize, method: &str, url: &str, status: u16) -> SessionRecord {
        SessionRecord { id, method: method.into(), url: url.into(), status, ..Default::default() }
    }

    fn sample() -> Vec<SessionRecord> {
        let mut a = rec(1, "GET", "http://a.com/index", 200);
        a.response_body = "hello NEEDLE world".into();
        let mut b = rec(2, "POST", "http://b.com/api/login", 404);
        b.request_headers.insert("X-Trace".into(), "zebra-123".into());
        let c = rec(3, "GET", "http://b.com/api/items", 500);
        let d = rec(4, "GET", "http://c.com/img.png", 200);
        vec![a, b, c, d]
    }

    fn load_model(p: &UiPrefs) -> SessionModel {
        let mut m = SessionModel::default();
        p.to_model(&mut m);
        for mut r in sample() {
            p.apply_to(&mut r);
            m.apply(SessionEvent::New(r));
        }
        m
    }

    #[test]
    fn filters_search_marks_and_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ui_state.json");
        let mut prefs = UiPrefs::default();
        let mut m = load_model(&prefs);
        assert_eq!(m.visible().len(), 4);

        // method / status / url substring
        prefs.method = "get".into();
        m.structured_filter = prefs.criteria(None);
        assert_eq!(m.visible(), vec![0, 2, 3]);
        prefs.method.clear();
        prefs.status = "404".into();
        m.structured_filter = prefs.criteria(None);
        assert_eq!(m.visible(), vec![1]);
        prefs.status.clear();
        prefs.url = "/API/".into();
        m.structured_filter = prefs.criteria(None);
        assert_eq!(m.visible(), vec![1, 2]);
        prefs.method = "GET".into();
        m.structured_filter = prefs.criteria(None);
        assert_eq!(m.visible(), vec![2]);
        prefs = UiPrefs::default();
        m.structured_filter = prefs.criteria(None);

        // full-text search: body, header, flags
        m.search_query = "needle".into();
        assert_eq!(m.visible(), vec![0]);
        m.search_in_bodies = false;
        assert!(m.visible().is_empty());
        m.search_in_bodies = true;
        m.search_query = "ZEBRA".into();
        assert_eq!(m.visible(), vec![1]);
        m.search_in_headers = false;
        assert!(m.visible().is_empty());
        m.search_in_headers = true;
        m.search_case_sensitive = true;
        assert!(m.visible().is_empty());
        m.search_case_sensitive = false;
        m.search_query.clear();

        // marks + comments, live in the table and filterable
        m.mark_session(2, Some(SessionMark::Red));
        m.set_comment(2, "login fails");
        m.mark_session(4, Some(SessionMark::Strikeout));
        for id in [2usize, 4] {
            let r = m.rows.iter().find(|r| r.id == id).unwrap().clone();
            prefs.sync_row(&r);
        }
        m.structured_filter = prefs.criteria(Some(SessionMark::Red));
        assert_eq!(m.visible(), vec![1]);
        m.structured_filter = prefs.criteria(None);
        m.search_query = "login fails".into(); // comments are searchable
        assert_eq!(m.visible(), vec![1]);
        m.search_query.clear();

        // columns + persistence round trip
        m.column_state.set_visible(GridColumn::Protocol, false);
        m.column_state.set_visible(GridColumn::Comments, true);
        m.search_query = "zzz".into();
        m.filter = "method:get".into();
        prefs.from_model(&m);
        prefs.save(&path).unwrap();

        let loaded = UiPrefs::load(&path);
        let m2 = load_model(&loaded);
        assert!(!m2.column_state.is_visible(GridColumn::Protocol));
        assert_eq!(m2.search_query, "zzz");
        assert_eq!(m2.filter, "method:get");
        assert_eq!(m2.rows[1].mark, Some(SessionMark::Red));
        assert_eq!(m2.rows[1].comment, "login fails");
        assert_eq!(m2.rows[3].mark, Some(SessionMark::Strikeout));
        assert_eq!(m2.rows[0].mark, None);

        // clearing a mark/comment removes the stored annotation
        let mut r = m2.rows[3].clone();
        r.mark = None;
        let mut p3 = loaded.clone();
        p3.sync_row(&r);
        assert_eq!(p3.annotations.len(), 1);
    }
}
