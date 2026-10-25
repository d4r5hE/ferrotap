//! R3 Group G1: session grid actions - copy helpers, session removal and the Clear menu filters.
use crate::model::{SessionModel, SessionRecord};

/// Which sessions the "Clear" menu removes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClearKind {
    All,
    Connect,
    Status2xx,
    Status3xx,
    Status4xx,
    Status5xx,
    Errors,
    StaticAssets,
    Unselected,
    Status1xx,
    Marked,
    NonMarked,
    /// Finished exchanges (a response arrived).
    Completed,
    Cached,
    NonCached,
}

impl ClearKind {
    pub const ALL_KINDS: [ClearKind; 15] = [
        ClearKind::All,
        ClearKind::Connect,
        ClearKind::Status2xx,
        ClearKind::Status3xx,
        ClearKind::Status4xx,
        ClearKind::Status5xx,
        ClearKind::Errors,
        ClearKind::StaticAssets,
        ClearKind::Unselected,
        ClearKind::Status1xx,
        ClearKind::Marked,
        ClearKind::NonMarked,
        ClearKind::Completed,
        ClearKind::Cached,
        ClearKind::NonCached,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            ClearKind::All => "All sessions",
            ClearKind::Connect => "CONNECT tunnels",
            ClearKind::Status2xx => "2xx responses",
            ClearKind::Status3xx => "3xx responses",
            ClearKind::Status4xx => "4xx responses",
            ClearKind::Status5xx => "5xx responses",
            ClearKind::Errors => "Errors (4xx, 5xx, no response)",
            ClearKind::StaticAssets => "Static assets (js, css, images, fonts)",
            ClearKind::Unselected => "All except selected",
            ClearKind::Status1xx => "1xx responses",
            ClearKind::Marked => "Marked sessions",
            ClearKind::NonMarked => "Sessions without a mark",
            ClearKind::Completed => "Completed (got a response)",
            ClearKind::Cached => "Cached (304, cache hits, local answers)",
            ClearKind::NonCached => "Non-cached",
        }
    }
}

pub fn is_static_asset(s: &SessionRecord) -> bool {
    let path = s.url.split(['?', '#']).next().unwrap_or("").to_ascii_lowercase();
    const EXT: [&str; 17] = [
        ".js", ".css", ".png", ".jpg", ".jpeg", ".gif", ".svg", ".ico", ".webp", ".woff", ".woff2", ".ttf", ".map", ".bmp",
        ".avif", ".otf", ".eot",
    ];
    if EXT.iter().any(|e| path.ends_with(e)) {
        return true;
    }
    let ct = s.content_type().to_ascii_lowercase();
    ct.starts_with("image/") || ct.contains("javascript") || ct.contains("text/css") || ct.contains("font")
}

/// Served from a cache: 304 Not Modified, a CDN / proxy cache hit (X-Cache, CF-Cache-Status, Age),
/// or answered locally by Ferrotap (Auto Responder, LLM cache).
pub fn is_cached(s: &SessionRecord) -> bool {
    let hit = |name: &str| s.response_header(name).is_some_and(|v| v.to_ascii_lowercase().contains("hit"));
    s.status == 304
        || hit("x-cache")
        || hit("cf-cache-status")
        || hit("x-cache-status")
        || s.response_header("age").and_then(|v| v.trim().parse::<u64>().ok()).is_some_and(|a| a > 0)
        || s.response_header("x-ferrotap-autoresponder").is_some()
        || s.response_headers.keys().any(|k| k.to_ascii_lowercase().starts_with("x-ferrotap-llm-cache"))
        || s.comment.starts_with("AutoResponder")
}

fn matches_kind(s: &SessionRecord, kind: ClearKind, selected: Option<usize>) -> bool {
    match kind {
        ClearKind::All => true,
        ClearKind::Connect => s.method.eq_ignore_ascii_case("CONNECT"),
        ClearKind::Status2xx => (200..300).contains(&s.status),
        ClearKind::Status3xx => (300..400).contains(&s.status),
        ClearKind::Status4xx => (400..500).contains(&s.status),
        ClearKind::Status5xx => (500..600).contains(&s.status),
        ClearKind::Errors => s.status == 0 || s.status >= 400,
        ClearKind::StaticAssets => is_static_asset(s),
        ClearKind::Unselected => Some(s.id) != selected,
        ClearKind::Status1xx => (100..200).contains(&s.status),
        ClearKind::Marked => s.mark.is_some(),
        ClearKind::NonMarked => s.mark.is_none(),
        ClearKind::Completed => s.status > 0,
        ClearKind::Cached => is_cached(s),
        ClearKind::NonCached => !is_cached(s),
    }
}

impl SessionModel {
    /// Remove sessions by id; clears the selection if it was removed. Returns how many were removed.
    pub fn remove_ids(&mut self, ids: &[usize]) -> usize {
        let before = self.rows.len();
        let gone: std::collections::HashSet<usize> = ids.iter().copied().collect();
        self.rows.retain(|r| !gone.contains(&r.id));
        if let Some(sel) = self.selected {
            if ids.contains(&sel) {
                self.selected = None;
            }
        }
        before - self.rows.len()
    }

    /// Clear-menu action. Returns how many sessions were removed.
    pub fn clear_kind(&mut self, kind: ClearKind) -> usize {
        let selected = self.selected;
        let ids: Vec<usize> = self.rows.iter().filter(|r| matches_kind(r, kind, selected)).map(|r| r.id).collect();
        self.remove_ids(&ids)
    }
}

/// Tab separated row: id, method, status, url, content type, body size, duration.
pub fn copy_row(s: &SessionRecord) -> String {
    format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}ms",
        s.id,
        s.method,
        s.status,
        s.url,
        s.content_type(),
        s.body_size(),
        s.duration_ms
    )
}

/// One-line summary: "#3 GET 200 https://x/y".
pub fn copy_summary(s: &SessionRecord) -> String {
    format!("#{} {} {} {}", s.id, s.method, s.status, s.url)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: usize, method: &str, url: &str, status: u16) -> SessionRecord {
        SessionRecord { id, method: method.into(), url: url.into(), status, ..Default::default() }
    }

    fn model() -> SessionModel {
        let mut m = SessionModel::default();
        m.rows = vec![
            rec(1, "CONNECT", "example.com:443", 200),
            rec(2, "GET", "https://example.com/app.js?v=1", 200),
            rec(3, "GET", "https://example.com/api", 404),
            rec(4, "POST", "https://example.com/api", 500),
            rec(5, "GET", "https://example.com/", 302),
            rec(6, "GET", "https://example.com/down", 0),
        ];
        m
    }

    #[test]
    fn clear_filters() {
        let mut m = model();
        assert_eq!(m.clear_kind(ClearKind::Connect), 1);
        assert_eq!(m.clear_kind(ClearKind::StaticAssets), 1);
        assert_eq!(m.clear_kind(ClearKind::Errors), 3);
        assert_eq!(m.rows.len(), 1);
        assert_eq!(m.rows[0].id, 5);
        let mut m = model();
        assert_eq!(m.clear_kind(ClearKind::Status4xx), 1);
        assert_eq!(m.clear_kind(ClearKind::Status5xx), 1);
        assert_eq!(m.clear_kind(ClearKind::Status3xx), 1);
        let mut m = model();
        assert_eq!(m.clear_kind(ClearKind::All), 6);
    }

    #[test]
    fn remove_and_unselected() {
        let mut m = model();
        m.selected = Some(3);
        assert_eq!(m.clear_kind(ClearKind::Unselected), 5);
        assert_eq!(m.rows.len(), 1);
        assert_eq!(m.remove_ids(&[3, 99]), 1);
        assert_eq!(m.selected, None);
        assert!(m.rows.is_empty());
    }

    #[test]
    fn copy_text() {
        let s = rec(7, "GET", "https://a.test/x", 200);
        assert_eq!(copy_summary(&s), "#7 GET 200 https://a.test/x");
        assert!(copy_row(&s).starts_with("7\tGET\t200\thttps://a.test/x\t"));
    }
}
