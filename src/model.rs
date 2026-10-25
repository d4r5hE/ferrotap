//! Session data model for the grid UI: record type (shared with the proxy core), events,
//! filtering, full-text search, column customization, sorting, and session marking/comments.
//! No UI code here so it is unit-testable.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Visual mark / highlight colour for a session row.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SessionMark {
    Red,
    Blue,
    Green,
    Orange,
    Purple,
    Copper,
    Strikeout,
}

impl SessionMark {
    pub fn as_str(&self) -> &'static str {
        match self {
            SessionMark::Red => "red",
            SessionMark::Blue => "blue",
            SessionMark::Green => "green",
            SessionMark::Orange => "orange",
            SessionMark::Purple => "purple",
            SessionMark::Copper => "copper",
            SessionMark::Strikeout => "strikeout",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "red" => Some(SessionMark::Red),
            "blue" => Some(SessionMark::Blue),
            "green" => Some(SessionMark::Green),
            "orange" => Some(SessionMark::Orange),
            "purple" => Some(SessionMark::Purple),
            "copper" => Some(SessionMark::Copper),
            "strikeout" | "strike" => Some(SessionMark::Strikeout),
            _ => None,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct SessionRecord {
    pub id: usize,
    pub method: String,
    pub url: String,
    pub status: u16,
    /// HTTP version the client used towards the proxy (e.g. "HTTP/2.0").
    #[serde(default)]
    pub http_version: String,
    /// HTTP version of the upstream response.
    #[serde(default)]
    pub response_http_version: String,
    pub request_headers: BTreeMap<String, String>,
    pub response_headers: BTreeMap<String, String>,
    pub request_body: String,
    pub response_body: String,
    pub start_time: String,
    pub duration_ms: u128,
    /// Optional color or strikeout mark.
    #[serde(default)]
    pub mark: Option<SessionMark>,
    /// User or system comment for this session.
    #[serde(default)]
    pub comment: String,
    /// Raw (wire) response body size before chunk reassembly / decompression.
    #[serde(default)]
    pub response_raw_size: Option<usize>,
    /// Decoded response body size after chunk reassembly and Content-Encoding decoding.
    #[serde(default)]
    pub response_decoded_size: Option<usize>,
    /// Trailers captured after a chunked body (e.g. Signature).
    #[serde(default)]
    pub response_trailers: Vec<(String, String)>,
    #[serde(default)]
    pub process_name: Option<String>,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub client_ip: Option<String>,
    #[serde(default)]
    pub remote_ip: Option<String>,
    #[serde(default)]
    pub tls_version: Option<String>,
    #[serde(default)]
    pub dns_ms: Option<u64>,
    #[serde(default)]
    pub connect_ms: Option<u64>,
    #[serde(default)]
    pub tls_ms: Option<u64>,
    #[serde(default)]
    pub ttfb_ms: Option<u64>,
    /// Request sent: from the request reaching the proxy until it was handed to the server
    /// connection (rules, scripts, breakpoints included).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_ms: Option<u64>,
    /// Exact request body bytes (base64) when they are not valid UTF-8; `request_body` then holds a lossy copy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_body_b64: Option<String>,
    /// Exact response body bytes (base64) when they are not valid UTF-8 (compressed, images, gRPC...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_body_b64: Option<String>,
}

/// Lossy text form of a captured body (what the text views show).
pub fn body_text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// Base64 of a captured body when it is not valid UTF-8, so the exact bytes survive storage.
pub fn body_b64(b: &[u8]) -> Option<String> {
    use base64::Engine;
    std::str::from_utf8(b).is_err().then(|| base64::engine::general_purpose::STANDARD.encode(b))
}

fn exact_bytes<'a>(text: &'a str, b64: &Option<String>) -> std::borrow::Cow<'a, [u8]> {
    use base64::Engine;
    match b64.as_deref().map(|s| base64::engine::general_purpose::STANDARD.decode(s)) {
        Some(Ok(v)) => std::borrow::Cow::Owned(v),
        _ => std::borrow::Cow::Borrowed(text.as_bytes()),
    }
}

impl SessionRecord {
    /// The request body exactly as captured (falls back to the stored text).
    pub fn request_bytes(&self) -> std::borrow::Cow<'_, [u8]> {
        exact_bytes(&self.request_body, &self.request_body_b64)
    }

    /// The response body exactly as captured (falls back to the stored text).
    pub fn response_bytes(&self) -> std::borrow::Cow<'_, [u8]> {
        exact_bytes(&self.response_body, &self.response_body_b64)
    }

    /// Store a request body: text plus the exact bytes when they are not UTF-8.
    pub fn set_request_body(&mut self, b: &[u8]) {
        self.request_body = body_text(b);
        self.request_body_b64 = body_b64(b);
    }

    /// Store a response body: text plus the exact bytes when they are not UTF-8.
    pub fn set_response_body(&mut self, b: &[u8]) {
        self.response_body = body_text(b);
        self.response_body_b64 = body_b64(b);
    }

    pub fn scheme(&self) -> &str {
        self.url.split("://").next().unwrap_or("")
    }

    fn after_scheme(&self) -> &str {
        self.url.split_once("://").map(|x| x.1).unwrap_or(&self.url)
    }

    pub fn host(&self) -> &str {
        let r = self.after_scheme();
        &r[..r.find('/').unwrap_or(r.len())]
    }

    pub fn path(&self) -> &str {
        let r = self.after_scheme();
        match r.find('/') {
            Some(i) => &r[i..],
            None => "/",
        }
    }

    pub fn response_header(&self, name: &str) -> Option<&str> {
        self.response_headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn request_header(&self, name: &str) -> Option<&str> {
        self.request_headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn content_type(&self) -> &str {
        self.response_header("content-type")
            .map(|v| v.split(';').next().unwrap_or("").trim())
            .unwrap_or("")
    }

    pub fn body_size(&self) -> usize {
        self.response_body.len()
    }

    /// Time of day (HH:MM:SS) from the RFC3339 start time.
    pub fn time_of_day(&self) -> &str {
        self.start_time.get(11..19).unwrap_or("")
    }

    /// Set or clear session mark.
    pub fn set_mark(&mut self, mark: Option<SessionMark>) {
        self.mark = mark;
    }

    /// Set session comment.
    pub fn set_comment(&mut self, comment: impl Into<String>) {
        self.comment = comment.into();
    }

    /// Search headers for a query term (case-insensitive unless specified).
    pub fn search_headers(&self, query: &str, case_sensitive: bool) -> bool {
        if query.is_empty() {
            return true;
        }
        let check = |s: &str| -> bool {
            if case_sensitive {
                s.contains(query)
            } else {
                s.to_lowercase().contains(&query.to_lowercase())
            }
        };
        self.request_headers.iter().any(|(k, v)| check(k) || check(v))
            || self.response_headers.iter().any(|(k, v)| check(k) || check(v))
    }

    /// Search bodies (request and response) for a query term.
    pub fn search_bodies(&self, query: &str, case_sensitive: bool) -> bool {
        if query.is_empty() {
            return true;
        }
        if case_sensitive {
            self.request_body.contains(query) || self.response_body.contains(query)
        } else {
            let q = query.to_lowercase();
            self.request_body.to_lowercase().contains(&q)
                || self.response_body.to_lowercase().contains(&q)
        }
    }

    /// Full-text search across headers, bodies, URL, method, status, and comment.
    pub fn full_text_search(&self, query: &str, include_headers: bool, include_bodies: bool, case_sensitive: bool) -> bool {
        if query.is_empty() {
            return true;
        }
        let q = if case_sensitive { query.to_string() } else { query.to_lowercase() };
        let check = |s: &str| -> bool {
            if case_sensitive {
                s.contains(&q)
            } else {
                s.to_lowercase().contains(&q)
            }
        };

        if check(&self.url) || check(&self.method) || check(&self.status.to_string()) || check(&self.comment) {
            return true;
        }
        if include_headers && self.search_headers(query, case_sensitive) {
            return true;
        }
        if include_bodies && self.search_bodies(query, case_sensitive) {
            return true;
        }
        false
    }
}

/// What a live source (proxy feed, demo, file) sends to the UI.
#[derive(Debug, Clone)]
pub enum SessionEvent {
    New(SessionRecord),
    Clear,
    Mark { id: usize, mark: Option<SessionMark> },
    Comment { id: usize, comment: String },
    /// A streamed session got more body (or finished): replaces the row with the same id.
    Update(SessionRecord),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortCol {
    Id,
    Status,
    Protocol,
    Host,
    Url,
    Method,
    Size,
    ContentType,
    Time,
    Duration,
    Comments,
    ConnectTime,
    HandshakeTime,
    Ttfb,
    TlsVersion,
    HttpVersion,
    ProcessName,
    Pid,
    ClientIp,
    RemoteIp,
    ReqSize,
}

/// Grid columns that can be shown/hidden and customized in order/width.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum GridColumn {
    Id,
    Status,
    Protocol,
    Host,
    Url,
    Method,
    Size,
    ContentType,
    Time,
    Duration,
    Comments,
    ConnectTime,
    HandshakeTime,
    Ttfb,
    TlsVersion,
    HttpVersion,
    ProcessName,
    Pid,
    ClientIp,
    RemoteIp,
    ReqSize,
}

impl GridColumn {
    pub const ALL: [GridColumn; 21] = [
        GridColumn::Id,
        GridColumn::Status,
        GridColumn::Protocol,
        GridColumn::Host,
        GridColumn::Url,
        GridColumn::Method,
        GridColumn::Size,
        GridColumn::ContentType,
        GridColumn::Time,
        GridColumn::Duration,
        GridColumn::Comments,
        GridColumn::ConnectTime,
        GridColumn::HandshakeTime,
        GridColumn::Ttfb,
        GridColumn::TlsVersion,
        GridColumn::HttpVersion,
        GridColumn::ProcessName,
        GridColumn::Pid,
        GridColumn::ClientIp,
        GridColumn::RemoteIp,
        GridColumn::ReqSize,
    ];

    /// Columns added after the first release start hidden, so existing layouts don't change.
    pub fn hidden_by_default(&self) -> bool {
        matches!(self, GridColumn::ProcessName | GridColumn::Pid | GridColumn::ClientIp | GridColumn::RemoteIp
            | GridColumn::ReqSize)
    }

    pub fn title(&self) -> &'static str {
        match self {
            GridColumn::Id => "#",
            GridColumn::Status => "Result",
            GridColumn::Protocol => "Protocol",
            GridColumn::Host => "Host",
            GridColumn::Url => "URL",
            GridColumn::Method => "Method",
            GridColumn::Size => "Body Size",
            GridColumn::ContentType => "Content-Type",
            GridColumn::Time => "Time",
            GridColumn::Duration => "Duration",
            GridColumn::Comments => "Comments",
            GridColumn::ConnectTime => "Connect",
            GridColumn::HandshakeTime => "Handshake",
            GridColumn::Ttfb => "TTFB",
            GridColumn::TlsVersion => "TLS",
            GridColumn::HttpVersion => "HTTP",
            GridColumn::ProcessName => "Process",
            GridColumn::Pid => "PID",
            GridColumn::ClientIp => "Client IP",
            GridColumn::RemoteIp => "Remote IP",
            GridColumn::ReqSize => "Request Size",
        }
    }

    pub fn default_width(&self) -> f32 {
        match self {
            GridColumn::Id => 50.0,
            GridColumn::Status => 60.0,
            GridColumn::Protocol => 70.0,
            GridColumn::Host => 150.0,
            GridColumn::Url => 250.0,
            GridColumn::Method => 70.0,
            GridColumn::Size => 80.0,
            GridColumn::ContentType => 120.0,
            GridColumn::Time => 80.0,
            GridColumn::Duration => 80.0,
            GridColumn::Comments => 150.0,
            GridColumn::ConnectTime => 70.0,
            GridColumn::HandshakeTime => 75.0,
            GridColumn::Ttfb => 65.0,
            GridColumn::TlsVersion => 70.0,
            GridColumn::HttpVersion => 70.0,
            GridColumn::ProcessName => 110.0,
            GridColumn::Pid => 60.0,
            GridColumn::ClientIp => 105.0,
            GridColumn::RemoteIp => 105.0,
            GridColumn::ReqSize => 85.0,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ColumnConfig {
    pub col: GridColumn,
    pub visible: bool,
    pub width: f32,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ColumnState {
    pub columns: Vec<ColumnConfig>,
}

impl Default for ColumnState {
    fn default() -> Self {
        Self {
            columns: GridColumn::ALL
                .iter()
                .map(|&col| ColumnConfig {
                    col,
                    visible: !col.hidden_by_default(),
                    width: col.default_width(),
                })
                .collect(),
        }
    }
}

impl ColumnState {
    /// Add any column this build knows but a saved layout doesn't (hidden), keeping the saved order.
    pub fn normalize(&mut self) {
        for col in GridColumn::ALL {
            if !self.columns.iter().any(|c| c.col == col) {
                self.columns.push(ColumnConfig { col, visible: false, width: col.default_width() });
            }
        }
    }

    /// Move a column up (delta < 0) or down (delta > 0) in the display order.
    pub fn move_col(&mut self, col: GridColumn, delta: i32) {
        if let Some(i) = self.columns.iter().position(|c| c.col == col) {
            let j = i as i32 + delta;
            if j >= 0 && (j as usize) < self.columns.len() {
                self.columns.swap(i, j as usize);
            }
        }
    }

    pub fn is_visible(&self, col: GridColumn) -> bool {
        self.columns
            .iter()
            .find(|c| c.col == col)
            .map(|c| c.visible)
            .unwrap_or(true)
    }

    pub fn set_visible(&mut self, col: GridColumn, visible: bool) {
        if let Some(c) = self.columns.iter_mut().find(|c| c.col == col) {
            c.visible = visible;
        }
    }

    pub fn toggle_visible(&mut self, col: GridColumn) {
        if let Some(c) = self.columns.iter_mut().find(|c| c.col == col) {
            c.visible = !c.visible;
        }
    }

    pub fn set_width(&mut self, col: GridColumn, width: f32) {
        if let Some(c) = self.columns.iter_mut().find(|c| c.col == col) {
            c.width = width;
        }
    }
}

/// Structured live filter criteria.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FilterCriteria {
    pub method: Option<String>,
    pub status: Option<u16>,
    pub url_substring: Option<String>,
    pub host_substring: Option<String>,
    pub content_type_substring: Option<String>,
    pub mark: Option<SessionMark>,
    pub has_comment: Option<bool>,
}

impl FilterCriteria {
    pub fn matches(&self, r: &SessionRecord) -> bool {
        if let Some(ref m) = self.method {
            if !r.method.eq_ignore_ascii_case(m) {
                return false;
            }
        }
        if let Some(st) = self.status {
            if r.status != st {
                return false;
            }
        }
        if let Some(ref u) = self.url_substring {
            if !r.url.to_lowercase().contains(&u.to_lowercase()) {
                return false;
            }
        }
        if let Some(ref h) = self.host_substring {
            if !r.host().to_lowercase().contains(&h.to_lowercase()) {
                return false;
            }
        }
        if let Some(ref ct) = self.content_type_substring {
            if !r.content_type().to_lowercase().contains(&ct.to_lowercase()) {
                return false;
            }
        }
        if let Some(target_mark) = self.mark {
            if r.mark != Some(target_mark) {
                return false;
            }
        }
        if let Some(has_c) = self.has_comment {
            let empty = r.comment.trim().is_empty();
            if has_c && empty {
                return false;
            }
            if !has_c && !empty {
                return false;
            }
        }
        true
    }
}

#[derive(Clone)]
pub struct SessionModel {
    pub rows: Vec<SessionRecord>,
    pub filter: String,
    /// Selected session id (stable across sorting/filtering).
    pub selected: Option<usize>,
    pub sort: Option<(SortCol, bool)>, // (column, ascending)
    pub column_state: ColumnState,
    pub structured_filter: FilterCriteria,
    /// Dedicated full-text search engine query.
    pub search_query: String,
    pub search_in_headers: bool,
    pub search_in_bodies: bool,
    pub search_case_sensitive: bool,
}

impl Default for SessionModel {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            filter: String::new(),
            selected: None,
            sort: None,
            column_state: ColumnState::default(),
            structured_filter: FilterCriteria::default(),
            search_query: String::new(),
            search_in_headers: true,
            search_in_bodies: true,
            search_case_sensitive: false,
        }
    }
}

impl SessionModel {
    pub fn apply(&mut self, ev: SessionEvent) {
        match ev {
            SessionEvent::New(s) => self.rows.push(s),
            SessionEvent::Clear => {
                self.rows.clear();
                self.selected = None;
            }
            SessionEvent::Mark { id, mark } => {
                self.mark_session(id, mark);
            }
            SessionEvent::Comment { id, comment } => {
                self.set_comment(id, comment);
            }
            SessionEvent::Update(s) => match self.rows.iter_mut().find(|r| r.id == s.id) {
                Some(r) => {
                    // keep a mark the user set while it was streaming
                    let mark = r.mark;
                    *r = s;
                    if r.mark.is_none() {
                        r.mark = mark;
                    }
                }
                None => self.rows.push(s),
            },
        }
    }

    /// Drop the oldest sessions until at most `keep` remain. Marked sessions and the selected one
    /// are never dropped (so more than `keep` can remain). Returns how many were removed.
    pub fn trim_to(&mut self, keep: usize) -> usize {
        let excess = self.rows.len().saturating_sub(keep);
        if excess == 0 {
            return 0;
        }
        let mut left = excess;
        let sel = self.selected;
        self.rows.retain(|r| {
            if left == 0 || r.mark.is_some() || Some(r.id) == sel {
                return true;
            }
            left -= 1;
            false
        });
        excess - left
    }

    pub fn selected_row(&self) -> Option<&SessionRecord> {
        let id = self.selected?;
        self.rows.iter().find(|r| r.id == id)
    }

    pub fn mark_session(&mut self, id: usize, mark: Option<SessionMark>) {
        if let Some(r) = self.rows.iter_mut().find(|r| r.id == id) {
            r.set_mark(mark);
        }
    }

    pub fn set_comment(&mut self, id: usize, comment: impl Into<String>) {
        if let Some(r) = self.rows.iter_mut().find(|r| r.id == id) {
            r.set_comment(comment);
        }
    }

    pub fn toggle_sort(&mut self, col: SortCol) {
        self.sort = match self.sort {
            Some((c, true)) if c == col => Some((col, false)),
            Some((c, false)) if c == col => None,
            _ => Some((col, true)),
        };
    }

    /// Indexes into `rows` that pass both string filters, structured criteria, and full-text search.
    pub fn visible(&self) -> Vec<usize> {
        let ft = self.filter.trim();
        // A filter starting with '~' is the mitmproxy-style expression language (shared with
        // breakpoints). Parse it once; while it is incomplete/invalid, don't hide anything.
        let dsl = ft.starts_with('~').then(|| crate::bpdsl::parse(ft).ok()).flatten();
        let dsl_mode = ft.starts_with('~');
        // Only merge headers / build the body string per row when the expression needs them.
        let (dsl_need_h, dsl_need_b) = dsl.as_ref().map(|e| (e.uses_headers(), e.uses_body())).unwrap_or((false, false));
        let terms: Vec<String> = if dsl_mode {
            Vec::new()
        } else {
            self.filter.split_whitespace().map(|t| t.to_lowercase()).collect()
        };
        let mut idx: Vec<usize> = (0..self.rows.len())
            .filter(|&i| {
                let r = &self.rows[i];
                if !self.structured_filter.matches(r) {
                    return false;
                }
                if !self.search_query.is_empty()
                    && !r.full_text_search(
                        &self.search_query,
                        self.search_in_headers,
                        self.search_in_bodies,
                        self.search_case_sensitive,
                    ) {
                        return false;
                    }
                if dsl_mode {
                    // Invalid/in-progress expression: leave rows shown until it parses.
                    return match &dsl {
                        Some(e) => {
                            // Merge request + response headers/body so ~h/~t/~b see the whole flow,
                            // but only pay for it when the expression actually inspects them.
                            let headers: Vec<(String, String)> = if dsl_need_h {
                                r.request_headers.iter().chain(r.response_headers.iter()).map(|(k, v)| (k.clone(), v.clone())).collect()
                            } else {
                                Vec::new()
                            };
                            let body = if dsl_need_b { format!("{}\n{}", r.request_body, r.response_body) } else { String::new() };
                            let ctx = crate::bpdsl::Ctx { method: &r.method, url: &r.url, status: r.status, response: r.status != 0, headers: &headers, body: &body };
                            e.eval_flow(&ctx)
                        }
                        None => true,
                    };
                }
                // A leading '-' excludes matches (e.g. `-host:ads.com`, `-png`).
                terms.iter().all(|t| match t.strip_prefix('-') {
                    Some("") => true,
                    Some(rest) => !Self::matches(r, rest),
                    None => Self::matches(r, t),
                })
            })
            .collect();
        if let Some((col, asc)) = self.sort {
            idx.sort_by(|&a, &b| {
                let (x, y) = (&self.rows[a], &self.rows[b]);
                let o = match col {
                    SortCol::Id => x.id.cmp(&y.id),
                    SortCol::ConnectTime => x.connect_ms.cmp(&y.connect_ms),
                    SortCol::HandshakeTime => x.tls_ms.cmp(&y.tls_ms),
                    SortCol::Ttfb => x.ttfb_ms.cmp(&y.ttfb_ms),
                    SortCol::TlsVersion => x.tls_version.cmp(&y.tls_version),
                    SortCol::HttpVersion => x.http_version.cmp(&y.http_version),
                    SortCol::Status => x.status.cmp(&y.status),
                    SortCol::Protocol => x.http_version.cmp(&y.http_version),
                    SortCol::Host => x.host().cmp(y.host()),
                    SortCol::Url => x.url.cmp(&y.url),
                    SortCol::Method => x.method.cmp(&y.method),
                    SortCol::Size => x.body_size().cmp(&y.body_size()),
                    SortCol::ContentType => x.content_type().cmp(y.content_type()),
                    SortCol::Time => x.start_time.cmp(&y.start_time),
                    SortCol::Duration => x.duration_ms.cmp(&y.duration_ms),
                    SortCol::Comments => x.comment.cmp(&y.comment),
                    SortCol::ProcessName => x.process_name.cmp(&y.process_name),
                    SortCol::Pid => x.pid.cmp(&y.pid),
                    SortCol::ClientIp => x.client_ip.cmp(&y.client_ip),
                    SortCol::RemoteIp => x.remote_ip.cmp(&y.remote_ip),
                    SortCol::ReqSize => x.request_body.len().cmp(&y.request_body.len()),
                };
                if asc { o } else { o.reverse() }
            });
        }
        idx
    }

    fn matches(r: &SessionRecord, t: &str) -> bool {
        if let Some((k, v)) = t.split_once(':') {
            match k {
                "size" | "sz" => return cmp_num(v, r.body_size() as i64),
                "dur" | "time" => return cmp_num(v, r.duration_ms as i64),
                "scheme" => return r.scheme().eq_ignore_ascii_case(v),
                "path" | "p" => return r.path().to_lowercase().contains(v),
                "ip" | "rip" => return r.remote_ip.as_deref().unwrap_or("").to_lowercase().contains(v),
                "method" | "m" => return r.method.to_lowercase() == v,
                // Exact code, class (5xx), range (400-499) or list (401,403).
                "status" | "s" => return !v.is_empty() && bp_status_matches(v, r.status),
                "url" | "u" => return r.url.to_lowercase().contains(v),
                "host" | "h" => return r.host().to_lowercase().contains(v),
                "type" | "ct" => return r.content_type().to_lowercase().contains(v),
                "mark" => {
                    return match r.mark {
                        Some(m) => m.as_str() == v,
                        None => v == "none" || v == "unmarked",
                    }
                }
                "comment" | "c" => return r.comment.to_lowercase().contains(v),
                "header" | "hdr" => return r.search_headers(v, false),
                "body" | "b" => return r.search_bodies(v, false),
                _ => {}
            }
        }
        r.method.to_lowercase().contains(t)
            || r.url.to_lowercase().contains(t)
            || r.status.to_string().contains(t)
            || r.content_type().to_lowercase().contains(t)
            || r.comment.to_lowercase().contains(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: usize, method: &str, url: &str, status: u16, ct: &str) -> SessionRecord {
        let mut r = SessionRecord {
            id,
            method: method.into(),
            url: url.into(),
            status,
            ..Default::default()
        };
        r.response_headers.insert("Content-Type".into(), format!("{ct}; charset=utf-8"));
        r
    }

    #[test]
    fn url_parts() {
        let r = rec(1, "GET", "https://example.com:8443/a/b?x=1", 200, "text/html");
        assert_eq!(r.host(), "example.com:8443");
        assert_eq!(r.path(), "/a/b?x=1");
        assert_eq!(r.scheme(), "https");
        assert_eq!(r.content_type(), "text/html");
    }

    #[test]
    fn filter_and_sort() {
        let mut m = SessionModel::default();
        m.apply(SessionEvent::New(rec(1, "GET", "http://a.com/x", 200, "text/html")));
        m.apply(SessionEvent::New(rec(2, "POST", "http://b.com/api", 404, "application/json")));
        m.apply(SessionEvent::New(rec(3, "GET", "http://b.com/img", 200, "image/png")));
        m.filter = "host:b.com get".into();
        assert_eq!(m.visible(), vec![2]);
        m.filter = "status:404".into();
        assert_eq!(m.visible(), vec![1]);
        // Status classes / ranges.
        m.filter = "status:2xx".into();
        assert_eq!(m.visible(), vec![0, 2]);
        m.filter = "status:200-404".into();
        assert_eq!(m.visible(), vec![0, 1, 2]);
        // Negation with '-'.
        m.filter = "-status:404".into();
        assert_eq!(m.visible(), vec![0, 2]);
        m.filter = "-host:b.com".into();
        assert_eq!(m.visible(), vec![0]);
        m.filter = "get -host:a.com".into();
        assert_eq!(m.visible(), vec![2]);
        // Numeric comparators on size (response body) and duration.
        m.rows[0].response_body = "x".repeat(5000);
        m.rows[0].duration_ms = 10;
        m.rows[1].response_body = "y".repeat(100);
        m.rows[1].duration_ms = 2000;
        m.filter = "size:>1000".into();
        assert_eq!(m.visible(), vec![0]);
        m.filter = "size:<=100".into();
        assert_eq!(m.visible(), vec![1, 2]); // row 2 has an empty body (0)
        m.filter = "dur:>=2000".into();
        assert_eq!(m.visible(), vec![1]);
        m.filter = "size:notanumber".into();
        assert!(m.visible().is_empty());
        // scheme / path keys.
        m.rows[0].url = "https://a.com/secure".into();
        m.filter = "scheme:https".into();
        assert_eq!(m.visible(), vec![0]);
        m.filter = "path:/api".into();
        assert_eq!(m.visible(), vec![1]); // id2 is http://b.com/api
        // mitmproxy-style `~` expression filter on the grid (shares the breakpoint engine).
        m.filter = "~m GET & ~c 2xx".into();
        assert_eq!(m.visible(), vec![0, 2]); // GET 200s
        m.filter = "~d b\\.com".into(); // regex host
        assert_eq!(m.visible(), vec![1, 2]);
        m.filter = "~c 404 | ~u img".into();
        assert_eq!(m.visible(), vec![1, 2]);
        m.filter = "~m POST & ~c 5xx".into();
        assert!(m.visible().is_empty());
        m.filter = "~t json".into(); // header-gated path
        assert_eq!(m.visible(), vec![1]);
        m.filter = "~b yyy".into(); // body-gated path (row 1's body is y's)
        assert_eq!(m.visible(), vec![1]);
        m.filter = "~u (".into(); // invalid regex -> filter inert, nothing hidden
        assert_eq!(m.visible(), vec![0, 1, 2]);
        m.filter.clear();
        m.toggle_sort(SortCol::Id);
        m.toggle_sort(SortCol::Id); // descending
        assert_eq!(m.visible(), vec![2, 1, 0]);
        m.apply(SessionEvent::Clear);
        assert!(m.rows.is_empty());
    }

    #[test]
    fn test_structured_filter_criteria() {
        let mut m = SessionModel::default();
        m.apply(SessionEvent::New(rec(1, "GET", "https://api.test/v1/users", 200, "application/json")));
        m.apply(SessionEvent::New(rec(2, "POST", "https://api.test/v1/users", 201, "application/json")));
        m.apply(SessionEvent::New(rec(3, "GET", "https://api.test/v1/orders", 404, "text/plain")));

        m.structured_filter.method = Some("GET".into());
        assert_eq!(m.visible(), vec![0, 2]);

        m.structured_filter.status = Some(404);
        assert_eq!(m.visible(), vec![2]);

        m.structured_filter.status = None;
        m.structured_filter.url_substring = Some("users".into());
        assert_eq!(m.visible(), vec![0]);
    }

    #[test]
    fn test_header_and_body_full_text_search() {
        let mut r = rec(10, "POST", "https://service.local/submit", 200, "application/json");
        r.request_headers.insert("Authorization".into(), "Bearer secret_token_xyz".into());
        r.request_body = "{\"query\": \"needle_in_request_body\"}".into();
        r.response_headers.insert("X-Transaction-Id".into(), "tx-987654321".into());
        r.response_body = "{\"result\": \"needle_in_response_body\"}".into();

        assert!(r.search_headers("secret_token", false));
        assert!(r.search_headers("x-transaction-id", false));
        assert!(!r.search_headers("nonexistent_hdr", false));

        assert!(r.search_bodies("needle_in_request", false));
        assert!(r.search_bodies("needle_in_response", false));
        assert!(!r.search_bodies("nonexistent_body", false));

        assert!(r.full_text_search("secret_token_xyz", true, false, false));
        assert!(r.full_text_search("needle_in_response_body", false, true, false));
    }

    #[test]
    fn test_session_marking_and_comments() {
        let mut m = SessionModel::default();
        let mut r1 = rec(1, "GET", "https://a.org/", 200, "text/html");
        r1.set_mark(Some(SessionMark::Red));
        r1.set_comment("Investigate latency issue");
        m.apply(SessionEvent::New(r1));

        let r2 = rec(2, "GET", "https://b.org/", 500, "text/html");
        m.apply(SessionEvent::New(r2));
        m.mark_session(2, Some(SessionMark::Strikeout));
        m.set_comment(2, "Deprecated endpoint");

        assert_eq!(m.rows[0].mark, Some(SessionMark::Red));
        assert_eq!(m.rows[0].comment, "Investigate latency issue");
        assert_eq!(m.rows[1].mark, Some(SessionMark::Strikeout));
        assert_eq!(m.rows[1].comment, "Deprecated endpoint");

        m.filter = "mark:red".into();
        assert_eq!(m.visible(), vec![0]);

        m.filter = "comment:deprecated".into();
        assert_eq!(m.visible(), vec![1]);
    }

    #[test]
    fn test_column_state_customization() {
        let mut state = ColumnState::default();
        assert!(state.is_visible(GridColumn::Id));
        assert!(state.is_visible(GridColumn::Comments));

        state.toggle_visible(GridColumn::Protocol);
        assert!(!state.is_visible(GridColumn::Protocol));

        state.set_width(GridColumn::Url, 350.0);
        let url_col = state.columns.iter().find(|c| c.col == GridColumn::Url).unwrap();
        assert_eq!(url_col.width, 350.0);
    }
}


// ===================== Composer: build a request and dispatch it through the capture proxy =====================
/// A request built in the Composer panel.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct ComposerRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

/// Result of dispatching a ComposerRequest (never panics; errors land in `error`).
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct ComposerResponse {
    pub status: u16,
    pub reason: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub elapsed_ms: u128,
    pub error: String,
}

/// Parse "Name: value" lines (one per line) into header pairs.
pub fn parse_header_lines(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            let k = k.trim();
            if k.is_empty() { None } else { Some((k.to_string(), v.trim().to_string())) }
        })
        .collect()
}

impl ComposerRequest {
    /// Authority (host[:port]) of an http:// or https:// URL. Both go to the capture proxy in
    /// absolute form; the proxy makes the TLS connection for https.
    fn authority(&self) -> Result<String, String> {
        let u = self.url.trim();
        let rest = u.strip_prefix("http://").or_else(|| u.strip_prefix("https://")).ok_or_else(|| "the URL must start with http:// or https://".to_string())?;
        let authority = rest.split('/').next().unwrap_or("").to_string();
        if authority.is_empty() {
            return Err("URL has no host".into());
        }
        Ok(authority)
    }

    /// Proxy-style request bytes (absolute-form request line) with auto Host / Content-Length / Connection.
    pub fn to_wire(&self) -> Result<Vec<u8>, String> {
        self.to_wire_version(false)
    }

    /// `to_wire` with an HTTP/1.0 request line when `http10`.
    pub fn to_wire_version(&self, http10: bool) -> Result<Vec<u8>, String> {
        let authority = self.authority()?;
        let method = if self.method.trim().is_empty() { "GET" } else { self.method.trim() }.to_uppercase();
        let has = |n: &str| self.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case(n));
        let mut s = format!("{} {} {}\r\n", method, self.url.trim(), if http10 { "HTTP/1.0" } else { "HTTP/1.1" });
        if !has("host") {
            s.push_str(&format!("Host: {authority}\r\n"));
        }
        for (k, v) in &self.headers {
            s.push_str(&format!("{k}: {v}\r\n"));
        }
        if !has("content-length") && (!self.body.is_empty() || matches!(method.as_str(), "POST" | "PUT" | "PATCH")) {
            s.push_str(&format!("Content-Length: {}\r\n", self.body.len()));
        }
        if !has("connection") {
            s.push_str("Connection: close\r\n");
        }
        s.push_str("\r\n");
        let mut b = s.into_bytes();
        b.extend_from_slice(self.body.as_bytes());
        Ok(b)
    }
}

fn dechunk(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        let Some(p) = raw[i..].windows(2).position(|w| w == b"\r\n") else { break };
        let size_s = String::from_utf8_lossy(&raw[i..i + p]).to_string();
        let Ok(n) = usize::from_str_radix(size_s.split(';').next().unwrap_or("").trim(), 16) else { break };
        i += p + 2;
        if n == 0 { break; }
        if i + n > raw.len() {
            out.extend_from_slice(&raw[i..]);
            break;
        }
        out.extend_from_slice(&raw[i..i + n]);
        i += n + 2;
    }
    out
}

/// Send `req` to the capture proxy at `proxy` (e.g. "127.0.0.1:8897") and parse the response.
pub fn composer_send(req: &ComposerRequest, proxy: &str) -> ComposerResponse {
    composer_send_timeout(req, proxy, 20)
}

/// Like `composer_send` with a read timeout of `timeout_secs` seconds (Composer settings).
pub fn composer_send_timeout(req: &ComposerRequest, proxy: &str, timeout_secs: u64) -> ComposerResponse {
    composer_send_opts(req, proxy, timeout_secs, false)
}

/// `composer_send_timeout` with a choice of HTTP/1.0.
pub fn composer_send_opts(req: &ComposerRequest, proxy: &str, timeout_secs: u64, http10: bool) -> ComposerResponse {
    use std::io::{Read, Write};
    let t0 = std::time::Instant::now();
    let fail = |e: String| ComposerResponse { error: e, elapsed_ms: t0.elapsed().as_millis(), ..Default::default() };
    let wire = match req.to_wire_version(http10) {
        Ok(w) => w,
        Err(e) => return fail(e),
    };
    let addr = match proxy.parse::<std::net::SocketAddr>() {
        Ok(a) => a,
        Err(e) => return fail(format!("bad proxy address {proxy}: {e}")),
    };
    let mut st = match std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_secs(5)) {
        Ok(s) => s,
        Err(e) => return fail(format!("cannot reach proxy {proxy}: {e}")),
    };
    let _ = st.set_read_timeout(Some(std::time::Duration::from_secs(timeout_secs.max(1))));
    if let Err(e) = st.write_all(&wire) {
        return fail(format!("write failed: {e}"));
    }
    let mut raw = Vec::new();
    if let Err(e) = st.read_to_end(&mut raw) {
        if raw.is_empty() {
            return fail(format!("read failed: {e}"));
        }
    }
    let Some(split) = raw.windows(4).position(|w| w == b"\r\n\r\n") else {
        return fail(format!("malformed response ({} bytes)", raw.len()));
    };
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let mut lines = head.lines();
    let status_line = lines.next().unwrap_or("").to_string();
    let mut parts = status_line.splitn(3, ' ');
    let _ver = parts.next();
    let status = parts.next().and_then(|s| s.parse::<u16>().ok()).unwrap_or(0);
    let reason = parts.next().unwrap_or("").to_string();
    let headers = parse_header_lines(&lines.collect::<Vec<_>>().join("\n"));
    let chunked = headers.iter().any(|(k, v)| k.eq_ignore_ascii_case("transfer-encoding") && v.to_lowercase().contains("chunked"));
    let body_raw = &raw[split + 4..];
    let body_bytes = if chunked { dechunk(body_raw) } else { body_raw.to_vec() };
    ComposerResponse {
        status,
        reason,
        headers,
        body: String::from_utf8_lossy(&body_bytes).to_string(),
        elapsed_ms: t0.elapsed().as_millis(),
        error: if status == 0 { format!("bad status line: {status_line}") } else { String::new() },
    }
}

#[cfg(test)]
mod composer_tests {
    use super::*;

    #[test]
    fn wire_format_has_auto_headers() {
        let r = ComposerRequest { method: "post".into(), url: "http://127.0.0.1:8898/x?a=1".into(), headers: parse_header_lines("X-A: 1\nbad line"), body: "hello".into() };
        let w = String::from_utf8(r.to_wire().unwrap()).unwrap();
        assert!(w.starts_with("POST http://127.0.0.1:8898/x?a=1 HTTP/1.1\r\nHost: 127.0.0.1:8898\r\nX-A: 1\r\n"));
        assert!(w.contains("Content-Length: 5\r\n") && w.ends_with("\r\n\r\nhello"));
        let s = String::from_utf8(ComposerRequest { url: "https://a/".into(), ..Default::default() }.to_wire().unwrap()).unwrap();
        assert!(s.starts_with("GET https://a/ HTTP/1.1\r\nHost: a\r\n"), "https goes to the proxy in absolute form: {s}");
        assert!(ComposerRequest { url: "ftp://a/".into(), ..Default::default() }.to_wire().is_err());
        let old = String::from_utf8(ComposerRequest { url: "http://a/".into(), ..Default::default() }.to_wire_version(true).unwrap()).unwrap();
        assert!(old.starts_with("GET http://a/ HTTP/1.0\r\n"), "{old}");
    }

    #[test]
    fn dechunk_works() {
        assert_eq!(dechunk(b"3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n"), b"abcde");
    }
}


// ===================== Composer: key-value editors, collections (JSON files), request history =====================
fn yes() -> bool { true }

/// One row of a key-value table editor (query parameter or header).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct KvRow {
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub value: String,
}

impl KvRow {
    pub fn new(k: &str, v: &str) -> Self { KvRow { enabled: true, key: k.to_string(), value: v.to_string() } }
}

pub fn pct_encode(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' | b':' | b',' | b'@' => o.push(b as char),
            _ => o.push_str(&format!("%{:02X}", b)),
        }
    }
    o
}

pub fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hv = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
            if let Some(v) = hv { out.push(v); i += 3; continue; }
        }
        out.push(if b[i] == b'+' { b' ' } else { b[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// Split a URL into (url without query, query parameters as rows).
pub fn parse_query(url: &str) -> (String, Vec<KvRow>) {
    match url.split_once('?') {
        None => (url.to_string(), Vec::new()),
        Some((base, q)) => {
            let rows = q.split('&').filter(|p| !p.is_empty()).map(|p| {
                let (k, v) = p.split_once('=').unwrap_or((p, ""));
                KvRow::new(&pct_decode(k), &pct_decode(v))
            }).collect();
            (base.to_string(), rows)
        }
    }
}

/// URL plus the enabled, non-empty-key parameter rows appended as a query string.
pub fn build_url(url: &str, rows: &[KvRow]) -> String {
    let parts: Vec<String> = rows.iter().filter(|r| r.enabled && !r.key.trim().is_empty())
        .map(|r| format!("{}={}", pct_encode(r.key.trim()), pct_encode(&r.value))).collect();
    if parts.is_empty() { return url.to_string(); }
    let sep = if !url.contains('?') { "?" } else if url.ends_with('?') || url.ends_with('&') { "" } else { "&" };
    format!("{}{}{}", url, sep, parts.join("&"))
}

pub fn rows_to_headers(rows: &[KvRow]) -> Vec<(String, String)> {
    rows.iter().filter(|r| r.enabled && !r.key.trim().is_empty()).map(|r| (r.key.trim().to_string(), r.value.trim().to_string())).collect()
}

pub fn rows_from_header_lines(text: &str) -> Vec<KvRow> {
    parse_header_lines(text).into_iter().map(|(k, v)| KvRow::new(&k, &v)).collect()
}

/// A Composer request as stored in a collection or in history.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct SavedRequest {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub method: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub params: Vec<KvRow>,
    #[serde(default)]
    pub headers: Vec<KvRow>,
    #[serde(default)]
    pub body: String,
    /// Response tests / assertions run after each send (G15).
    #[serde(default)]
    pub tests: Vec<crate::api_tools::Assertion>,
    /// Body editor mode: "" (raw) or "graphql".
    #[serde(default)]
    pub body_mode: String,
    /// Saved example response (served by the mock server, exported to Postman/OpenAPI).
    #[serde(default)]
    pub example: Option<crate::api_tools::SavedResponse>,
}

impl SavedRequest {
    /// The request that is actually sent (params appended to URL, only enabled headers).
    pub fn to_composer(&self) -> ComposerRequest {
        ComposerRequest {
            method: self.method.clone(),
            url: build_url(&self.url, &self.params),
            headers: rows_to_headers(&self.headers),
            body: self.body.clone(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct Collection {
    pub name: String,
    #[serde(default)]
    pub requests: Vec<SavedRequest>,
    /// Folders without requests yet (a request's folder is the part of its name before the last '/').
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub folders: Vec<String>,
}

impl Collection {
    pub fn new(name: &str) -> Self { Collection { name: name.to_string(), requests: Vec::new(), folders: Vec::new() } }
    /// Add a request, replacing one with the same name.
    pub fn upsert(&mut self, r: SavedRequest) {
        if let Some(x) = self.requests.iter_mut().find(|x| x.name == r.name) { *x = r; } else { self.requests.push(r); }
    }
    pub fn remove(&mut self, name: &str) -> bool {
        let n = self.requests.len();
        self.requests.retain(|x| x.name != name);
        self.requests.len() != n
    }
    pub fn find(&self, name: &str) -> Option<&SavedRequest> { self.requests.iter().find(|x| x.name == name) }

    /// Every folder path (from request names and empty folders), parents included, sorted.
    pub fn folder_list(&self) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        let names = self.requests.iter().filter_map(|r| r.name.rsplit_once('/').map(|(f, _)| f.to_string())).chain(self.folders.iter().cloned());
        for f in names {
            let parts: Vec<&str> = f.split('/').filter(|p| !p.is_empty()).collect();
            for i in 1..=parts.len() {
                let p = parts[..i].join("/");
                if !v.contains(&p) { v.push(p); }
            }
        }
        v.sort_by(|a, b| a.split('/').cmp(b.split('/')));
        v
    }

    pub fn create_folder(&mut self, path: &str) -> Result<(), String> {
        let p = path.trim().trim_matches('/').to_string();
        if p.is_empty() { return Err("folder name is empty".into()); }
        if self.folder_list().contains(&p) { return Err(format!("folder '{p}' already exists")); }
        self.folders.push(p);
        Ok(())
    }

    /// Rename (or move) a folder: its requests and sub-folders follow.
    pub fn rename_folder(&mut self, old: &str, new: &str) -> Result<usize, String> {
        let (old, new) = (old.trim().trim_matches('/'), new.trim().trim_matches('/'));
        if new.is_empty() { return Err("folder name is empty".into()); }
        if old == new { return Ok(0); }
        if new.starts_with(&format!("{old}/")) { return Err("a folder cannot move into itself".into()); }
        let under = |n: &str| n == old || n.starts_with(&format!("{old}/"));
        let mut moved = 0;
        for r in self.requests.iter_mut().filter(|r| r.name.starts_with(&format!("{old}/"))) {
            r.name = format!("{new}{}", &r.name[old.len()..]);
            moved += 1;
        }
        for f in self.folders.iter_mut().filter(|f| under(f)) {
            *f = format!("{new}{}", &f[old.len()..]);
        }
        if moved == 0 && !self.folders.iter().any(|f| f == new) { self.folders.push(new.to_string()); }
        Ok(moved)
    }

    /// Delete a folder and every request in it; returns how many requests went.
    pub fn delete_folder(&mut self, path: &str) -> usize {
        let p = path.trim().trim_matches('/');
        let under = |n: &str| n == p || n.starts_with(&format!("{p}/"));
        let before = self.requests.len();
        self.requests.retain(|r| !r.name.starts_with(&format!("{p}/")));
        self.folders.retain(|f| !under(f));
        before - self.requests.len()
    }

    /// Move a request into `folder` ("" = top level), keeping its own name.
    pub fn move_request(&mut self, name: &str, folder: &str) -> Result<String, String> {
        let leaf = name.rsplit('/').next().unwrap_or(name).to_string();
        let f = folder.trim().trim_matches('/');
        let new = if f.is_empty() { leaf } else { format!("{f}/{leaf}") };
        if new != name && self.find(&new).is_some() { return Err(format!("'{new}' already exists")); }
        let r = self.requests.iter_mut().find(|r| r.name == name).ok_or("no such request")?;
        r.name = new.clone();
        Ok(new)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct HistoryEntry {
    pub ts: u64,
    pub status: u16,
    #[serde(default)]
    pub error: String,
    #[serde(default)]
    pub elapsed_ms: u64,
    pub request: SavedRequest,
}

pub const HISTORY_MAX: usize = 100;

pub fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Newest first, capped at HISTORY_MAX.
pub fn history_push(h: &mut Vec<HistoryEntry>, e: HistoryEntry) {
    h.insert(0, e);
    h.truncate(HISTORY_MAX);
}

/// Folder holding collection files. Override with env FERROTAP_COLLECTIONS_DIR (used by tests).
pub fn collections_dir() -> std::path::PathBuf {
    match std::env::var_os("FERROTAP_COLLECTIONS_DIR") {
        Some(p) if !p.is_empty() => std::path::PathBuf::from(p),
        _ => std::path::PathBuf::from(r"D:\Projects\Ferrotap\collections"),
    }
}

pub fn safe_file_name(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' { c } else { '_' }).collect();
    let s = s.trim().to_string();
    if s.is_empty() { "collection".to_string() } else { s }
}

pub fn save_collection(dir: &std::path::Path, c: &Collection) -> Result<std::path::PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let p = dir.join(format!("{}.json", safe_file_name(&c.name)));
    let txt = serde_json::to_string_pretty(c).map_err(|e| e.to_string())?;
    std::fs::write(&p, txt).map_err(|e| format!("write {}: {e}", p.display()))?;
    Ok(p)
}

/// Load every *.json collection in `dir` (files starting with '_' are internal and skipped), sorted by name.
pub fn load_collections(dir: &std::path::Path) -> Vec<Collection> {
    let mut v: Vec<Collection> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            let fname = p.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
            if !fname.to_lowercase().ends_with(".json") || fname.starts_with('_') { continue; }
            if let Ok(t) = std::fs::read_to_string(&p) {
                if let Ok(c) = serde_json::from_str::<Collection>(&t) { v.push(c); }
            }
        }
    }
    v.sort_by_key(|a| a.name.to_lowercase());
    v
}

pub fn delete_collection(dir: &std::path::Path, name: &str) -> bool {
    std::fs::remove_file(dir.join(format!("{}.json", safe_file_name(name)))).is_ok()
}

pub fn save_history(dir: &std::path::Path, h: &[HistoryEntry]) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let txt = serde_json::to_string_pretty(h).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("_history.json"), txt).map_err(|e| e.to_string())
}

pub fn load_history(dir: &std::path::Path) -> Vec<HistoryEntry> {
    std::fs::read_to_string(dir.join("_history.json")).ok()
        .and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

#[cfg(test)]
mod coll_tests {
    use super::*;

    #[test]
    fn query_roundtrip() {
        let (b, rows) = parse_query("http://h/p?a=1&b=x%20y&c");
        assert_eq!(b, "http://h/p");
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1].value, "x y");
        assert_eq!(build_url(&b, &rows), "http://h/p?a=1&b=x%20y&c=");
        let mut off = rows.clone();
        off[0].enabled = false;
        assert_eq!(build_url("http://h/p?z=9", &off), "http://h/p?z=9&b=x%20y&c=");
    }

    #[test]
    fn collection_persist() {
        let d = tempfile::tempdir().unwrap();
        let mut c = Collection::new("My API");
        c.upsert(SavedRequest { name: "r1".into(), method: "POST".into(), url: "http://h/".into(), params: vec![KvRow::new("q", "1")], headers: vec![KvRow::new("X-A", "b")], body: "x".into(), ..Default::default() });
        c.upsert(SavedRequest { name: "r1".into(), method: "GET".into(), ..Default::default() });
        assert_eq!(c.requests.len(), 1);
        save_collection(d.path(), &c).unwrap();
        let mut h = Vec::new();
        history_push(&mut h, HistoryEntry { ts: 1, status: 200, request: c.requests[0].clone(), ..Default::default() });
        save_history(d.path(), &h).unwrap();
        let l = load_collections(d.path());
        assert_eq!(l, vec![c.clone()]);
        assert_eq!(load_history(d.path()), h);
        assert!(delete_collection(d.path(), "My API"));
        assert!(load_collections(d.path()).is_empty());
    }
}


// ===================== Rules (Auto Responder) =====================

fn rule_true() -> bool { true }
fn rule_200() -> u16 { 200 }

/// One Auto Responder rule: match (URL contains/regex + optional method) -> action (canned response or file).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Rule {
    #[serde(default)]
    pub name: String,
    #[serde(default = "rule_true")]
    pub enabled: bool,
    /// "contains" or "regex"
    #[serde(default)]
    pub match_kind: String,
    #[serde(default)]
    pub pattern: String,
    /// Empty = any method.
    #[serde(default)]
    pub method: String,
    /// "canned" (status/headers/body) or "file" (serve a local file).
    #[serde(default)]
    pub action: String,
    #[serde(default = "rule_200")]
    pub status: u16,
    #[serde(default)]
    pub headers: Vec<KvRow>,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub file: String,
    /// Stable id (hit counters); 0 in old files, assigned on load.
    #[serde(default)]
    pub id: u64,
    /// Group name ("" = ungrouped) and whether the whole group is switched off.
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub group_off: bool,
    /// Host pattern: `*.example.com`, `=exact.host` or a substring ("" = any host).
    #[serde(default)]
    pub host: String,
    /// "request" (default, "") or "response" (evaluated on the upstream response; can match status).
    #[serde(default)]
    pub stage: String,
    /// Extra conditions (header, cookie, body, process, status, ...): all must hold.
    #[serde(default)]
    pub conds: Vec<crate::rules_ext::RuleCond>,
    /// Redirect Location or Map Remote target.
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub req_headers: Vec<crate::rules_ext::HeaderEdit>,
    #[serde(default)]
    pub resp_headers: Vec<crate::rules_ext::HeaderEdit>,
    #[serde(default)]
    pub req_body: Vec<crate::rules_ext::TextReplace>,
    #[serde(default)]
    pub resp_body: Vec<crate::rules_ext::TextReplace>,
    /// Added latency in milliseconds.
    #[serde(default)]
    pub delay_ms: u64,
    /// Stop processing more rules after this one (pass-through actions).
    #[serde(default)]
    pub stop: bool,
    /// G12 extras: query/cookie rewrites, status, user-agent, no-cache, no-cookies, throttling.
    #[serde(default, skip_serializing_if = "crate::rules_more::RuleExtras::is_empty")]
    pub more: crate::rules_more::RuleExtras,
}

impl Default for Rule {
    fn default() -> Self {
        Rule {
            name: String::new(),
            enabled: true,
            match_kind: "contains".into(),
            pattern: String::new(),
            method: String::new(),
            action: "canned".into(),
            status: 200,
            headers: Vec::new(),
            body: String::new(),
            file: String::new(),
            id: 0,
            group: String::new(),
            group_off: false,
            host: String::new(),
            stage: String::new(),
            conds: Vec::new(),
            target: String::new(),
            req_headers: Vec::new(),
            resp_headers: Vec::new(),
            req_body: Vec::new(),
            resp_body: Vec::new(),
            delay_ms: 0,
            stop: false,
            more: Default::default(),
        }
    }
}

impl Rule {
    /// URL + method only (legacy check; the proxy uses `rules_ext::rule_matches`).
    pub fn matches(&self, method: &str, url: &str) -> bool {
        if !self.enabled || self.group_off || self.pattern.is_empty() {
            return false;
        }
        if !self.method.is_empty() && !self.method.eq_ignore_ascii_case(method) {
            return false;
        }
        if self.match_kind == "regex" {
            regex::Regex::new(&self.pattern).map(|r| r.is_match(url)).unwrap_or(false)
        } else {
            url.contains(&self.pattern)
        }
    }
}

/// The response a matched rule produces.
#[derive(Debug, Clone, PartialEq)]
pub struct RuleResponse {
    pub name: String,
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

pub fn rule_response(r: &Rule) -> RuleResponse {
    let mut headers: Vec<(String, String)> = r
        .headers
        .iter()
        .filter(|h| h.enabled && !h.key.trim().is_empty())
        .map(|h| (h.key.trim().to_string(), h.value.clone()))
        .collect();
    let has_ct = |h: &Vec<(String, String)>| h.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-type"));
    if r.action == "file" {
        match std::fs::read(&r.file) {
            Ok(b) => {
                if !has_ct(&headers) {
                    let ext = std::path::Path::new(&r.file)
                        .extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("")
                        .to_ascii_lowercase();
                    let ct = match ext.as_str() {
                        "json" => "application/json",
                        "html" | "htm" => "text/html; charset=utf-8",
                        "txt" => "text/plain; charset=utf-8",
                        "js" => "application/javascript",
                        "css" => "text/css",
                        "xml" => "application/xml",
                        "png" => "image/png",
                        "jpg" | "jpeg" => "image/jpeg",
                        "gif" => "image/gif",
                        "svg" => "image/svg+xml",
                        "webp" => "image/webp",
                        "ico" => "image/x-icon",
                        "pdf" => "application/pdf",
                        "wasm" => "application/wasm",
                        "woff2" => "font/woff2",
                        "mp4" => "video/mp4",
                        "csv" => "text/csv; charset=utf-8",
                        "mjs" => "application/javascript",
                        _ => "application/octet-stream",
                    };
                    headers.push(("Content-Type".into(), ct.into()));
                }
                RuleResponse { name: r.name.clone(), status: r.status, headers, body: b }
            }
            Err(e) => RuleResponse {
                name: r.name.clone(),
                status: 404,
                headers: vec![("Content-Type".into(), "text/plain; charset=utf-8".into())],
                body: format!("AutoResponder: cannot read file {}: {e}", r.file).into_bytes(),
            },
        }
    } else {
        if !has_ct(&headers) {
            headers.push(("Content-Type".into(), "text/plain; charset=utf-8".into()));
        }
        RuleResponse { name: r.name.clone(), status: r.status, headers, body: r.body.clone().into_bytes() }
    }
}

/// First enabled rule (in list order) matching method+url.
pub fn rules_first_match(rules: &[Rule], method: &str, url: &str) -> Option<RuleResponse> {
    rules.iter().find(|r| r.matches(method, url)).map(rule_response)
}

/// Folder holding rules.json.
pub fn rules_dir() -> std::path::PathBuf {
    match std::env::var_os("FERROTAP_RULES_DIR") {
        Some(p) if !p.is_empty() => std::path::PathBuf::from(p),
        _ => std::path::PathBuf::from(r"D:\Projects\Ferrotap\rules"),
    }
}

pub fn load_rules(dir: &std::path::Path) -> Vec<Rule> {
    std::fs::read_to_string(dir.join("rules.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Vec<Rule>>(&t).ok())
        .map(|mut v| {
            crate::rules_ext::ensure_ids(&mut v);
            v
        })
        .unwrap_or_default()
}

pub fn save_rules(dir: &std::path::Path, rules: &[Rule]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let j = serde_json::to_string_pretty(rules).map_err(|e| std::io::Error::other(e))?;
    std::fs::write(dir.join("rules.json"), j)
}

static GLOBAL_RULES: std::sync::OnceLock<std::sync::RwLock<Vec<Rule>>> = std::sync::OnceLock::new();

fn global_rules_lock() -> &'static std::sync::RwLock<Vec<Rule>> {
    GLOBAL_RULES.get_or_init(|| std::sync::RwLock::new(Vec::new()))
}

/// Replace the live rule set the proxy consults.
pub fn set_global_rules(mut rules: Vec<Rule>) {
    crate::rules_ext::ensure_ids(&mut rules);
    if let Ok(mut g) = global_rules_lock().write() {
        *g = rules;
    }
}

/// Snapshot of the live rule set (the proxy evaluates it with `rules_ext::plan_request` / `plan_response`).
pub fn global_rules() -> Vec<Rule> {
    global_rules_lock().read().map(|g| g.clone()).unwrap_or_default()
}

/// Used by the proxy: the canned response for this request, if an enabled rule matches.
pub fn rules_match(method: &str, url: &str) -> Option<RuleResponse> {
    global_rules_lock().read().ok().and_then(|g| rules_first_match(&g, method, url))
}

#[cfg(test)]
mod rule_tests {
    use super::*;

    #[test]
    fn rule_match_order_toggle_persist() {
        let mut a = Rule { name: "a".into(), pattern: "/api/".into(), body: "A".into(), status: 201, ..Default::default() };
        let b = Rule { name: "b".into(), pattern: r"/api/\d+$".into(), match_kind: "regex".into(), method: "POST".into(), body: "B".into(), ..Default::default() };
        let mut rs = vec![b.clone(), a.clone()];
        crate::rules_ext::ensure_ids(&mut rs);
        assert_eq!(rules_first_match(&rs, "POST", "http://h/api/12").unwrap().name, "b");
        assert_eq!(rules_first_match(&rs, "GET", "http://h/api/12").unwrap().status, 201);
        assert!(rules_first_match(&rs, "GET", "http://h/other").is_none());
        a.enabled = false;
        assert!(rules_first_match(&[a.clone()], "GET", "http://h/api/1").is_none());
        let d = tempfile::tempdir().unwrap();
        save_rules(d.path(), &rs).unwrap();
        assert_eq!(load_rules(d.path()), rs);
    }
}



// ===================== Breakpoints (Rules, part 2) =====================

fn bp_true() -> bool {
    true
}

/// Which half of an exchange a breakpoint pauses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BpStage {
    Request,
    Response,
}

impl BpStage {
    pub fn label(&self) -> &'static str {
        match self {
            BpStage::Request => "request",
            BpStage::Response => "response",
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BreakpointRule {
    #[serde(default)]
    pub name: String,
    #[serde(default = "bp_true")]
    pub enabled: bool,
    /// "contains" or "regex"
    #[serde(default)]
    pub match_kind: String,
    /// URL pattern; empty = any URL.
    #[serde(default)]
    pub pattern: String,
    /// Empty = any method.
    #[serde(default)]
    pub method: String,
    /// Response status filter: "" any, "404", "5xx", "400-499", or a comma list like "401,403,5xx".
    #[serde(default)]
    pub status: String,
    #[serde(default = "bp_true")]
    pub on_request: bool,
    #[serde(default)]
    pub on_response: bool,
}

impl Default for BreakpointRule {
    fn default() -> Self {
        BreakpointRule {
            name: String::new(),
            enabled: true,
            match_kind: "contains".into(),
            pattern: String::new(),
            method: String::new(),
            status: String::new(),
            on_request: true,
            on_response: false,
        }
    }
}

/// True if `status` satisfies a spec such as "", "404", "5xx", "400-499", "401,403,5xx".
/// Compare `val` against a numeric spec like `>1000`, `<=5`, `100` (bare = exact). False on a
/// non-numeric spec. Used by the grid filter's `size:`/`dur:` keys.
fn cmp_num(spec: &str, val: i64) -> bool {
    let s = spec.trim();
    let (op, rest) = if let Some(r) = s.strip_prefix(">=") {
        (">=", r)
    } else if let Some(r) = s.strip_prefix("<=") {
        ("<=", r)
    } else if let Some(r) = s.strip_prefix('>') {
        (">", r)
    } else if let Some(r) = s.strip_prefix('<') {
        ("<", r)
    } else {
        ("=", s)
    };
    match rest.trim().parse::<i64>() {
        Ok(n) => match op {
            ">" => val > n,
            "<" => val < n,
            ">=" => val >= n,
            "<=" => val <= n,
            _ => val == n,
        },
        Err(_) => false,
    }
}

pub fn bp_status_matches(spec: &str, status: u16) -> bool {
    let spec = spec.trim();
    if spec.is_empty() {
        return true;
    }
    spec.split(',').map(|p| p.trim().to_ascii_lowercase()).filter(|p| !p.is_empty()).any(|p| {
        if p.len() == 3 && p.ends_with("xx") {
            p[..1].parse::<u16>().map(|d| status / 100 == d).unwrap_or(false)
        } else if let Some((a, b)) = p.split_once('-') {
            match (a.trim().parse::<u16>(), b.trim().parse::<u16>()) {
                (Ok(a), Ok(b)) => status >= a && status <= b,
                _ => false,
            }
        } else {
            p.parse::<u16>().map(|n| n == status).unwrap_or(false)
        }
    })
}

impl BreakpointRule {
    fn url_method_ok(&self, method: &str, url: &str) -> bool {
        if !self.enabled {
            return false;
        }
        if !self.method.is_empty() && !self.method.eq_ignore_ascii_case(method) {
            return false;
        }
        if self.pattern.is_empty() {
            return true;
        }
        if self.match_kind == "regex" {
            regex::Regex::new(&self.pattern).map(|r| r.is_match(url)).unwrap_or(false)
        } else {
            url.contains(&self.pattern)
        }
    }
    /// Should the request (before it goes upstream) be paused? `headers`/`body` are the request's
    /// (for the DSL `~h`/`~b`); pass `&[]`/`""` when unavailable.
    pub fn matches_request(&self, method: &str, url: &str, headers: &[(String, String)], body: &str) -> bool {
        if self.match_kind == "dsl" {
            return self.enabled && self.on_request && crate::bpdsl::eval_str(&self.pattern, &crate::bpdsl::Ctx { method, url, status: 0, response: false, headers, body });
        }
        self.on_request && self.url_method_ok(method, url)
    }
    /// Should the response (before it reaches the client) be paused? `headers`/`body` are the
    /// response's (for the DSL `~h`/`~b`); pass `&[]`/`""` when unavailable.
    pub fn matches_response(&self, method: &str, url: &str, status: u16, headers: &[(String, String)], body: &str) -> bool {
        if self.match_kind == "dsl" {
            return self.enabled && self.on_response && crate::bpdsl::eval_str(&self.pattern, &crate::bpdsl::Ctx { method, url, status, response: true, headers, body });
        }
        self.on_response && self.url_method_ok(method, url) && bp_status_matches(&self.status, status)
    }
}

/// First enabled breakpoint matching; returns its name (or pattern when unnamed).
pub fn bp_first_match(
    rules: &[BreakpointRule],
    stage: BpStage,
    method: &str,
    url: &str,
    status: u16,
    headers: &[(String, String)],
    body: &str,
) -> Option<String> {
    rules
        .iter()
        .find(|r| match stage {
            BpStage::Request => r.matches_request(method, url, headers, body),
            BpStage::Response => r.matches_response(method, url, status, headers, body),
        })
        .map(|r| if r.name.is_empty() { r.pattern.clone() } else { r.name.clone() })
}

pub fn load_breakpoints(dir: &std::path::Path) -> Vec<BreakpointRule> {
    std::fs::read_to_string(dir.join("breakpoints.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Vec<BreakpointRule>>(&t).ok())
        .unwrap_or_default()
}

pub fn save_breakpoints(dir: &std::path::Path, rules: &[BreakpointRule]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let j = serde_json::to_string_pretty(rules).map_err(|e| std::io::Error::other(e))?;
    std::fs::write(dir.join("breakpoints.json"), j)
}

static GLOBAL_BPS: std::sync::OnceLock<std::sync::RwLock<Vec<BreakpointRule>>> = std::sync::OnceLock::new();

fn global_bps_lock() -> &'static std::sync::RwLock<Vec<BreakpointRule>> {
    GLOBAL_BPS.get_or_init(|| std::sync::RwLock::new(Vec::new()))
}

/// Replace the live breakpoint set the proxy consults.
pub fn set_global_breakpoints(rules: Vec<BreakpointRule>) {
    if let Ok(mut g) = global_bps_lock().write() {
        *g = rules;
    }
}

pub fn global_breakpoints() -> Vec<BreakpointRule> {
    global_bps_lock().read().map(|g| g.clone()).unwrap_or_default()
}

/// Used by the proxy: name of the breakpoint that pauses this exchange stage, if any.
pub fn breakpoint_match(stage: BpStage, method: &str, url: &str, status: u16, headers: &[(String, String)], body: &str) -> Option<String> {
    global_bps_lock().read().ok().and_then(|g| bp_first_match(&g, stage, method, url, status, headers, body))
}

/// Does any enabled response-stage DSL breakpoint inspect the body? If so the proxy must buffer
/// (and decode) the response body before it can evaluate the breakpoint.
pub fn bp_response_wants_body() -> bool {
    global_bps_lock()
        .read()
        .ok()
        .map(|g| {
            g.iter().any(|r| {
                r.enabled
                    && r.on_response
                    && r.match_kind == "dsl"
                    && crate::bpdsl::parse_cached(&r.pattern).map(|e| e.uses_body()).unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

// ---- pause / resume / abort synchronization ----

/// Edits the user applies to a paused message before resuming it. `None` = keep as is.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BpEdit {
    pub method: Option<String>,
    pub url: Option<String>,
    pub status: Option<u16>,
    pub headers: Option<Vec<(String, String)>>,
    pub body: Option<Vec<u8>>,
}

#[derive(Debug)]
pub enum BpDecision {
    Resume(BpEdit),
    Abort,
}

/// Snapshot of a paused message, shown in the UI while it is held.
#[derive(Debug, Clone, PartialEq)]
pub struct PausedInfo {
    pub id: u64,
    pub stage: BpStage,
    pub rule: String,
    pub method: String,
    pub url: String,
    /// 0 for a request stage.
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub since_ms: i64,
}

struct PausedSlot {
    info: PausedInfo,
    tx: Option<tokio::sync::oneshot::Sender<BpDecision>>,
}

/// Shared between proxy tasks (which `pause().await`) and the UI (which resumes/aborts).
#[derive(Default)]
pub struct BreakpointHub {
    next: std::sync::atomic::AtomicU64,
    slots: std::sync::Mutex<std::collections::BTreeMap<u64, PausedSlot>>,
}

/// Why a pause ended.
#[derive(Debug)]
pub enum PauseOutcome {
    Resumed(BpEdit),
    Aborted,
    /// Nobody answered within the timeout; the message continues unmodified.
    TimedOut,
}

impl BreakpointHub {
    pub fn new() -> Self {
        Self::default()
    }

    /// Park this message until the UI resumes or aborts it (or `timeout` passes).
    /// Dropping the future (client disconnect) removes the slot.
    pub async fn pause(&self, mut info: PausedInfo, timeout: std::time::Duration) -> PauseOutcome {
        let id = self.next.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        info.id = id;
        info.since_ms = chrono::Utc::now().timestamp_millis();
        let (tx, rx) = tokio::sync::oneshot::channel();
        if let Ok(mut m) = self.slots.lock() {
            m.insert(id, PausedSlot { info, tx: Some(tx) });
        }
        struct Guard<'a>(&'a BreakpointHub, u64);
        impl Drop for Guard<'_> {
            fn drop(&mut self) {
                if let Ok(mut m) = self.0.slots.lock() {
                    m.remove(&self.1);
                }
            }
        }
        let _g = Guard(self, id);
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(BpDecision::Resume(e))) => PauseOutcome::Resumed(e),
            Ok(Ok(BpDecision::Abort)) => PauseOutcome::Aborted,
            Ok(Err(_)) => PauseOutcome::Aborted,
            Err(_) => PauseOutcome::TimedOut,
        }
    }

    /// Currently paused messages, oldest first.
    pub fn list(&self) -> Vec<PausedInfo> {
        self.slots.lock().map(|m| m.values().map(|s| s.info.clone()).collect()).unwrap_or_default()
    }

    pub fn count(&self) -> usize {
        self.slots.lock().map(|m| m.len()).unwrap_or(0)
    }

    fn answer(&self, id: u64, d: BpDecision) -> bool {
        let tx = self.slots.lock().ok().and_then(|mut m| m.get_mut(&id).and_then(|s| s.tx.take()));
        match tx {
            Some(tx) => tx.send(d).is_ok(),
            None => false,
        }
    }

    /// Let a paused message continue, applying `edit`. False if it is gone.
    pub fn resume(&self, id: u64, edit: BpEdit) -> bool {
        self.answer(id, BpDecision::Resume(edit))
    }

    /// Kill the paused exchange (client gets a 502 from the proxy).
    pub fn abort(&self, id: u64) -> bool {
        self.answer(id, BpDecision::Abort)
    }

    pub fn resume_all(&self) -> usize {
        self.list().iter().filter(|p| self.resume(p.id, BpEdit::default())).count()
    }
}

static GLOBAL_HUB: std::sync::OnceLock<std::sync::Arc<BreakpointHub>> = std::sync::OnceLock::new();

/// The process-wide hub shared by proxy and UI.
pub fn breakpoint_hub() -> std::sync::Arc<BreakpointHub> {
    GLOBAL_HUB.get_or_init(|| std::sync::Arc::new(BreakpointHub::new())).clone()
}

pub fn bp_match_req(method: &str, url: &str, headers: &[(String, String)], body: &str) -> Option<String> {
    breakpoint_match(BpStage::Request, method, url, 0, headers, body)
}

pub fn bp_match_resp(method: &str, url: &str, status: u16, headers: &[(String, String)], body: &str) -> Option<String> {
    breakpoint_match(BpStage::Response, method, url, status, headers, body)
}

#[cfg(test)]
mod breakpoint_tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn status_specs() {
        assert!(bp_status_matches("", 200));
        assert!(bp_status_matches("404", 404));
        assert!(!bp_status_matches("404", 403));
        assert!(bp_status_matches("5xx", 503));
        assert!(!bp_status_matches("5xx", 404));
        assert!(bp_status_matches("400-499", 418));
        assert!(bp_status_matches("401, 5XX", 500));
        assert!(!bp_status_matches("abc", 200));
    }

    #[test]
    fn rule_matching() {
        let req = BreakpointRule { pattern: "/api/".into(), method: "post".into(), ..Default::default() };
        let resp = BreakpointRule { pattern: r"/v\d+/".into(), match_kind: "regex".into(), on_request: false, on_response: true, status: "5xx".into(), ..Default::default() };
        let rs = vec![req, resp];
        let nh: &[(String, String)] = &[];
        assert!(bp_first_match(&rs, BpStage::Request, "POST", "http://h/api/x", 0, nh, "").is_some());
        assert!(bp_first_match(&rs, BpStage::Request, "GET", "http://h/api/x", 0, nh, "").is_none());
        assert!(bp_first_match(&rs, BpStage::Response, "GET", "http://h/v2/x", 502, nh, "").is_some());
        assert!(bp_first_match(&rs, BpStage::Response, "GET", "http://h/v2/x", 200, nh, "").is_none());
        let mut off = rs[0].clone();
        off.enabled = false;
        assert!(bp_first_match(&[off], BpStage::Request, "POST", "http://h/api/x", 0, nh, "").is_none());
        // DSL header matching: `~h` tests the headers passed in for the stage.
        let dsl = BreakpointRule { pattern: "~q & ~h authorization".into(), match_kind: "dsl".into(), ..Default::default() };
        let auth = [("Authorization".to_string(), "Bearer t".to_string())];
        assert!(bp_first_match(std::slice::from_ref(&dsl), BpStage::Request, "GET", "http://h/x", 0, &auth, "").is_some());
        assert!(bp_first_match(std::slice::from_ref(&dsl), BpStage::Request, "GET", "http://h/x", 0, nh, "").is_none());
        // DSL body matching: `~b` tests the body passed in for the stage.
        let bdsl = BreakpointRule { pattern: "~s & ~b boom".into(), match_kind: "dsl".into(), on_request: false, on_response: true, ..Default::default() };
        assert!(bp_first_match(std::slice::from_ref(&bdsl), BpStage::Response, "GET", "http://h/x", 500, nh, "oh boom").is_some());
        assert!(bp_first_match(std::slice::from_ref(&bdsl), BpStage::Response, "GET", "http://h/x", 500, nh, "fine").is_none());
        let d = tempfile::tempdir().unwrap();
        save_breakpoints(d.path(), &rs).unwrap();
        assert_eq!(load_breakpoints(d.path()), rs);
    }

    fn info() -> PausedInfo {
        PausedInfo { id: 0, stage: BpStage::Request, rule: "r".into(), method: "GET".into(), url: "http://h/".into(), status: 0, headers: vec![], body: vec![], since_ms: 0 }
    }

    #[tokio::test]
    async fn pause_resume_abort_timeout() {
        let hub = std::sync::Arc::new(BreakpointHub::new());
        let h = hub.clone();
        let t = tokio::spawn(async move { h.pause(info(), Duration::from_secs(5)).await });
        let id = loop {
            if let Some(p) = hub.list().first() { break p.id; }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        assert_eq!(hub.count(), 1);
        assert!(hub.resume(id, BpEdit { body: Some(b"x".to_vec()), ..Default::default() }));
        match t.await.unwrap() {
            PauseOutcome::Resumed(e) => assert_eq!(e.body.unwrap(), b"x"),
            o => panic!("{o:?}"),
        }
        assert_eq!(hub.count(), 0);
        assert!(!hub.resume(id, BpEdit::default()));
        let h = hub.clone();
        let t = tokio::spawn(async move { h.pause(info(), Duration::from_secs(5)).await });
        let id = loop {
            if let Some(p) = hub.list().first() { break p.id; }
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        assert!(hub.abort(id));
        assert!(matches!(t.await.unwrap(), PauseOutcome::Aborted));
        assert!(matches!(hub.pause(info(), Duration::from_millis(30)).await, PauseOutcome::TimedOut));
        assert_eq!(hub.count(), 0);
    }
}
