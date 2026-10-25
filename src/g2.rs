pub type AutocompleteIndex = FilterAutocompleteIndex;
// Group G2 Features: Grid columns, advanced filters, saved filters,
// custom columns, traffic grouping, autocomplete, streaming mode, and traffic tabs.

use std::collections::{BTreeMap, HashSet};
use serde::{Deserialize, Serialize};
use regex::Regex;
use crate::model::SessionRecord;

// ---------------------------------------------------------------------------
// 1. Custom Columns
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum ColumnSource {
    Header(String),
    ReqHeader(String),
    ResHeader(String),
    BodyRegex(String),
    QueryParam(String),
    Cookie(String),
    JsonPath(String),
    Duration,
    TlsCipher,
    ProcessId,
    ProcessName,
    TlsVersion,
    ClientIp,
    RemoteIp,
    /// The `column(s)` hook of a script / plugin (by name).
    Script(String),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CustomColumnDef {
    pub id: String,
    pub title: String,
    pub source: ColumnSource,
    pub width: f32,
    pub visible: bool,
    pub align_right: bool,
}

impl CustomColumnDef {
    pub fn new(id: impl Into<String>, title: impl Into<String>, source: ColumnSource) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            source,
            width: 100.0,
            visible: true,
            align_right: false,
        }
    }

    pub fn extract_value(&self, session: &SessionRecord) -> String {
        match &self.source {
            ColumnSource::Script(name) => crate::scripting::call_session_fn(name, "column", session).unwrap_or_else(|e| format!("error: {e}")),
            ColumnSource::Header(name) => {
                let needle = name.to_ascii_lowercase();
                for (k, v) in &session.request_headers {
                    if k.to_ascii_lowercase() == needle {
                        return v.clone();
                    }
                }
                for (k, v) in &session.response_headers {
                    if k.to_ascii_lowercase() == needle {
                        return v.clone();
                    }
                }
                String::new()
            }
            ColumnSource::ReqHeader(name) => session.request_headers.iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone()).unwrap_or_default(),
            ColumnSource::ResHeader(name) => session.response_headers.iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone()).unwrap_or_default(),
            ColumnSource::BodyRegex(pat) => {
                // first capture group (or the whole match) in the response body, else the request body
                let Ok(re) = regex::Regex::new(pat) else { return String::new() };
                for body in [&session.response_body, &session.request_body] {
                    if let Some(c) = re.captures(body) {
                        return c.get(1).or_else(|| c.get(0)).map(|x| x.as_str().to_string()).unwrap_or_default();
                    }
                }
                String::new()
            }
            ColumnSource::QueryParam(param) => {
                if let Some(pos) = session.url.find('?') {
                    let query = &session.url[pos + 1..];
                    for pair in query.split('&') {
                        let mut parts = pair.splitn(2, '=');
                        if let (Some(k), Some(v)) = (parts.next(), parts.next()) {
                            if k == param {
                                return v.to_string();
                            }
                        }
                    }
                }
                String::new()
            }
            ColumnSource::Cookie(cookie_name) => {
                let needle = cookie_name.as_str();
                let mut req_cookie = String::new();
                for (k, v) in &session.request_headers {
                    if k.eq_ignore_ascii_case("cookie") {
                        req_cookie = v.clone();
                        break;
                    }
                }
                for (k, v) in &session.response_headers { if k.eq_ignore_ascii_case("set-cookie") { req_cookie.push_str("; "); req_cookie.push_str(v); } }
                for pair in req_cookie.split(';') {
                    let mut parts = pair.trim().splitn(2, '=');
                    if let (Some(k), Some(v)) = (parts.next(), parts.next()) {
                        if k == needle {
                            return v.to_string();
                        }
                    }
                }
                String::new()
            }
            ColumnSource::JsonPath(field) => {
                // dotted path into the response JSON: "user.name", "items.0.id"
                let Ok(mut val) = serde_json::from_str::<serde_json::Value>(&session.response_body) else {
                    return String::new();
                };
                for part in field.split('.').filter(|p| !p.is_empty()) {
                    let next = match part.parse::<usize>() {
                        Ok(i) if val.is_array() => val.get(i).cloned(),
                        _ => val.get(part).cloned(),
                    };
                    match next {
                        Some(v) => val = v,
                        None => return String::new(),
                    }
                }
                match val {
                    serde_json::Value::String(s) => s,
                    v => v.to_string(),
                }
            }
            ColumnSource::Duration => format!("{}ms", session.duration_ms),
            ColumnSource::TlsCipher => session.tls_version.clone().unwrap_or_default(),
            ColumnSource::ProcessId => session.pid.map(|p| p.to_string()).unwrap_or_default(),
            ColumnSource::ProcessName => session.process_name.clone().unwrap_or_default(),
            ColumnSource::TlsVersion => session.tls_version.clone().unwrap_or_default(),
            ColumnSource::ClientIp => session.client_ip.clone().unwrap_or_default(),
            ColumnSource::RemoteIp => session.remote_ip.clone().unwrap_or_default(),
        }
    }
}

// ---------------------------------------------------------------------------
// 2. Structured Filter Conditions & Advanced Filter Rule
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum FilterField {
    Method,
    Host,
    Path,
    Url,
    Status,
    ContentType,
    RequestBody,
    ResponseBody,
    ReqHeader(String),
    ResHeader(String),
    DurationMs,
    ResponseSize,
    ProcessName,
    ClientIp,
    RemoteIp,
    Duration,
    ContentLength,
    ReqBody,
    ResBody,
    Cookie(String),
    Pid,
    ConnectMs,
    TlsMs,
    TtfbMs,
    TlsVersion,
    /// A query parameter's value ("" = the whole query string).
    QueryParam(String),
    Comment,
    /// Client HTTP version (e.g. HTTP/2.0).
    HttpVersion,
    /// Server (response) HTTP version.
    ResponseHttpVersion,
}

impl FilterField {
    /// Readable field name for menus.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Method => "Method",
            Self::Host => "Host",
            Self::Path => "Path",
            Self::Url => "URL",
            Self::Status => "Status Code",
            Self::ContentType => "Content-Type",
            Self::RequestBody | Self::ReqBody => "Request Body",
            Self::ResponseBody | Self::ResBody => "Response Body",
            Self::ReqHeader(_) => "Request Header",
            Self::ResHeader(_) => "Response Header",
            Self::Cookie(_) => "Cookie",
            Self::DurationMs | Self::Duration => "Duration (ms)",
            Self::ResponseSize | Self::ContentLength => "Response Size",
            Self::ProcessName => "Process Name",
            Self::Pid => "PID",
            Self::ClientIp => "Client IP",
            Self::RemoteIp => "Remote IP",
            Self::ConnectMs => "Connect (ms)",
            Self::TlsMs => "TLS Handshake (ms)",
            Self::TtfbMs => "TTFB (ms)",
            Self::TlsVersion => "TLS Version",
            Self::QueryParam(_) => "Query param",
            Self::Comment => "Comment",
            Self::HttpVersion => "Client HTTP version",
            Self::ResponseHttpVersion => "Server HTTP version",
        }
    }

    /// Name plus the header / cookie / parameter it is about, e.g. `Request Header "Accept"`.
    pub fn label(&self) -> String {
        match self {
            Self::ReqHeader(n) | Self::ResHeader(n) | Self::Cookie(n) | Self::QueryParam(n) if !n.is_empty() => format!("{} \"{}\"", self.name(), n),
            _ => self.name().to_string(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum ConditionOp {
    Equals,
    NotEquals,
    Contains,
    NotContains,
    StartsWith,
    EndsWith,
    GreaterThan,
    LessThan,
    RegexMatch,
    Regex,
    Exists,
    DoesNotExist,
}

impl ConditionOp {
    pub fn label(&self) -> &'static str {
        match self {
            ConditionOp::Equals => "equals",
            ConditionOp::NotEquals => "not equals",
            ConditionOp::Contains => "contains",
            ConditionOp::NotContains => "not contains",
            ConditionOp::StartsWith => "starts with",
            ConditionOp::EndsWith => "ends with",
            ConditionOp::GreaterThan => ">",
            ConditionOp::LessThan => "<",
            ConditionOp::RegexMatch | ConditionOp::Regex => "matches regex",
            ConditionOp::Exists => "exists",
            ConditionOp::DoesNotExist => "does not exist",
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum Combinator {
    All,
    Any,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FilterCondition {
    pub field: FilterField,
    pub op: ConditionOp,
    pub value: String,
}

impl FilterCondition {
    pub fn new(field: FilterField, op: ConditionOp, value: impl Into<String>) -> Self {
        Self {
            field,
            op,
            value: value.into(),
        }
    }


fn parse_filter_numeric(s: &str) -> Option<f64> {
    let trimmed = s.trim().to_ascii_lowercase();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(num) = trimmed.strip_suffix("ms") {
        return num.trim().parse::<f64>().ok();
    }
    if let Some(num) = trimmed.strip_suffix('s') {
        return num.trim().parse::<f64>().ok().map(|n| n * 1000.0);
    }
    if let Some(num) = trimmed.strip_suffix("kb") {
        return num.trim().parse::<f64>().ok().map(|n| n * 1024.0);
    }
    if let Some(num) = trimmed.strip_suffix('k') {
        return num.trim().parse::<f64>().ok().map(|n| n * 1024.0);
    }
    if let Some(num) = trimmed.strip_suffix("mb") {
        return num.trim().parse::<f64>().ok().map(|n| n * 1024.0 * 1024.0);
    }
    if let Some(num) = trimmed.strip_suffix('m') {
        return num.trim().parse::<f64>().ok().map(|n| n * 1024.0 * 1024.0);
    }
    if let Some(num) = trimmed.strip_suffix('b') {
        return num.trim().parse::<f64>().ok();
    }
    trimmed.parse::<f64>().ok()
}

    pub fn eval(&self, session: &SessionRecord) -> bool {
        let actual_str = match &self.field {
            FilterField::Method => session.method.clone(),
            FilterField::Host => session.host().to_string(),
            FilterField::Path => session.path().to_string(),
            FilterField::Url => session.url.clone(),
            FilterField::Status => session.status.to_string(),
            FilterField::ContentType => session.content_type().to_string(),
            FilterField::RequestBody | FilterField::ReqBody => session.request_body.clone(),
            FilterField::ResponseBody | FilterField::ResBody => session.response_body.clone(),
            FilterField::ReqHeader(h) => {
                session.request_headers.iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(h))
                    .map(|(_, v)| v.clone())
                    .unwrap_or_default()
            }
            FilterField::ResHeader(h) => {
                session.response_headers.iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(h))
                    .map(|(_, v)| v.clone())
                    .unwrap_or_default()
            }
            FilterField::DurationMs | FilterField::Duration => session.duration_ms.to_string(),
            FilterField::ResponseSize | FilterField::ContentLength => session.response_body.len().to_string(),
            FilterField::ProcessName => session.process_name.clone().unwrap_or_default(),
            FilterField::ClientIp => session.client_ip.clone().unwrap_or_default(),
            FilterField::RemoteIp => session.remote_ip.clone().unwrap_or_default(),
            FilterField::Cookie(c) => {
                let needle = c.trim();
                if needle.is_empty() {
                    let mut all = Vec::new();
                    for (k, v) in &session.request_headers {
                        if k.eq_ignore_ascii_case("cookie") {
                            all.push(v.clone());
                        }
                    }
                    for (k, v) in &session.response_headers {
                        if k.eq_ignore_ascii_case("set-cookie") {
                            all.push(v.clone());
                        }
                    }
                    all.join("; ")
                } else {
                    let mut cookie_val = String::new();
                    for (k, v) in &session.request_headers {
                        if k.eq_ignore_ascii_case("cookie") {
                            for pair in v.split(';') {
                                let mut parts = pair.splitn(2, '=');
                                if let (Some(name), Some(val)) = (parts.next(), parts.next()) {
                                    if name.trim().eq_ignore_ascii_case(needle) {
                                        cookie_val = val.trim().to_string();
                                        break;
                                    }
                                }
                            }
                        }
                        if !cookie_val.is_empty() {
                            break;
                        }
                    }
                    if cookie_val.is_empty() {
                        for (k, v) in &session.response_headers {
                            if k.eq_ignore_ascii_case("set-cookie") {
                                for pair in v.split(';') {
                                    let mut parts = pair.splitn(2, '=');
                                    if let (Some(name), Some(val)) = (parts.next(), parts.next()) {
                                        if name.trim().eq_ignore_ascii_case(needle) {
                                            cookie_val = val.trim().to_string();
                                            break;
                                        }
                                    }
                                }
                            }
                            if !cookie_val.is_empty() {
                                break;
                            }
                        }
                    }
                    cookie_val
                }
            }
            FilterField::Pid => session.pid.map(|p| p.to_string()).unwrap_or_default(),
            FilterField::ConnectMs => session.connect_ms.map(|v| v.to_string()).unwrap_or_default(),
            FilterField::TlsMs => session.tls_ms.map(|v| v.to_string()).unwrap_or_default(),
            FilterField::TtfbMs => session.ttfb_ms.map(|v| v.to_string()).unwrap_or_default(),
            FilterField::TlsVersion => session.tls_version.clone().unwrap_or_default(),
            FilterField::QueryParam(name) => {
                let q = session.url.split_once('?').map(|(_, q)| q.split('#').next().unwrap_or(q)).unwrap_or("");
                if name.trim().is_empty() {
                    q.to_string()
                } else {
                    crate::inspect::parse_query(&session.url).into_iter().find(|(k, _)| k == name.trim()).map(|(_, v)| v).unwrap_or_default()
                }
            }
            FilterField::Comment => session.comment.clone(),
            FilterField::HttpVersion => session.http_version.clone(),
            FilterField::ResponseHttpVersion => session.response_http_version.clone(),
        };

        match &self.op {
            ConditionOp::Equals => actual_str.eq_ignore_ascii_case(&self.value),
            ConditionOp::NotEquals => !actual_str.eq_ignore_ascii_case(&self.value),
            ConditionOp::Contains => actual_str.to_ascii_lowercase().contains(&self.value.to_ascii_lowercase()),
            ConditionOp::NotContains => !actual_str.to_ascii_lowercase().contains(&self.value.to_ascii_lowercase()),
            ConditionOp::StartsWith => actual_str.to_ascii_lowercase().starts_with(&self.value.to_ascii_lowercase()),
            ConditionOp::EndsWith => actual_str.to_ascii_lowercase().ends_with(&self.value.to_ascii_lowercase()),
            ConditionOp::GreaterThan => {
                match (Self::parse_filter_numeric(&actual_str), Self::parse_filter_numeric(&self.value)) {
                    (Some(act), Some(exp)) => act > exp,
                    _ => false,
                }
            }
            ConditionOp::LessThan => {
                match (Self::parse_filter_numeric(&actual_str), Self::parse_filter_numeric(&self.value)) {
                    (Some(act), Some(exp)) => act < exp,
                    _ => false,
                }
            }
            ConditionOp::RegexMatch | ConditionOp::Regex => {
                if let Ok(re) = Regex::new(&self.value) {
                    re.is_match(&actual_str)
                } else {
                    false
                }
            }
            ConditionOp::Exists => !actual_str.is_empty(),
            ConditionOp::DoesNotExist => actual_str.is_empty(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AdvancedFilterRule {
    pub name: String,
    pub combinator: Combinator,
    pub conditions: Vec<FilterCondition>,
    pub enabled: bool,
}

impl AdvancedFilterRule {
    pub fn single(cond: FilterCondition) -> Self {
        Self {
            name: "Quick Filter".to_string(),
            combinator: Combinator::All,
            conditions: vec![cond],
            enabled: true,
        }
    }

    pub fn describe(&self) -> String {
        let joiner = match self.combinator { Combinator::All => " AND ", _ => " OR " };
        self.conditions.iter()
            .map(|c| format!("{} {} '{}'", c.field.label(), c.op.label(), c.value))
            .collect::<Vec<_>>().join(joiner)
    }

    pub fn matches(&self, session: &SessionRecord) -> bool {
        if !self.enabled || self.conditions.is_empty() {
            return true;
        }
        match self.combinator {
            Combinator::All => self.conditions.iter().all(|c| c.eval(session)),
            Combinator::Any => self.conditions.iter().any(|c| c.eval(session)),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SavedFilter {
    pub id: String,
    pub name: String,
    pub rule: AdvancedFilterRule,
    pub enabled: bool,
}

impl SavedFilter {
    pub fn new(name: impl Into<String>, rule: AdvancedFilterRule) -> Self {
        Self {
            id: format!("sf-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)),
            name: name.into(),
            rule,
            enabled: true,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct SavedFiltersManager {
    pub filters: Vec<SavedFilter>,
    pub active_filter_id: Option<String>,
    #[serde(default)]
    pub active_rule: Option<AdvancedFilterRule>,
}

impl SavedFiltersManager {
    pub fn default_presets() -> Vec<SavedFilter> {
        vec![
            SavedFilter {
                id: "preset-errors".to_string(),
                name: "HTTP 4xx / 5xx Errors".to_string(),
                rule: AdvancedFilterRule {
                    name: "Errors (>=400)".to_string(),
                    combinator: Combinator::Any,
                    conditions: vec![
                        FilterCondition::new(FilterField::Status, ConditionOp::GreaterThan, "399"),
                    ],
                    enabled: true,
                },
                enabled: false,
            },
            SavedFilter {
                id: "preset-json".to_string(),
                name: "JSON Responses".to_string(),
                rule: AdvancedFilterRule {
                    name: "JSON Responses".to_string(),
                    combinator: Combinator::All,
                    conditions: vec![
                        FilterCondition::new(FilterField::ContentType, ConditionOp::Contains, "json"),
                    ],
                    enabled: true,
                },
                enabled: false,
            },
        ]
    }


    pub fn filters(&self) -> &[SavedFilter] {
        &self.filters
    }
    pub fn filters_mut(&mut self) -> &mut Vec<SavedFilter> {
        &mut self.filters
    }

    pub fn new() -> Self {
        Self {
            filters: Vec::new(),
            active_filter_id: None,
            active_rule: None,
        }
    }

    pub fn add(&mut self, name: &str, rule: AdvancedFilterRule) -> String {
        let id = format!("filter-{}", self.filters.len() + 1);
        self.filters.push(SavedFilter {
            id: id.clone(),
            name: name.to_string(),
            rule,
            enabled: true,
        });
        id
    }

        pub fn rename(&mut self, id: &str, new_name: &str) -> bool {
        if let Some(f) = self.filters.iter_mut().find(|f| f.id == id) {
            f.name = new_name.trim().to_string();
            true
        } else {
            false
        }
    }

    pub fn duplicate(&mut self, id: &str) -> Option<String> {
        if let Some(pos) = self.filters.iter().position(|f| f.id == id) {
            let mut clone = self.filters[pos].clone();
            clone.id = format!("{}-copy", clone.id);
            clone.name = format!("{} (Copy)", clone.name);
            let new_id = clone.id.clone();
            self.filters.insert(pos + 1, clone);
            Some(new_id)
        } else {
            None
        }
    }

    pub fn delete(&mut self, id: &str) -> bool {
        if let Some(pos) = self.filters.iter().position(|f| f.id == id) {
            self.filters.remove(pos);
            if self.active_filter_id.as_deref() == Some(id) {
                self.active_filter_id = None;
            }
            true
        } else {
            false
        }
    }

    pub fn remove(&mut self, id: &str) -> bool {
        self.delete(id)
    }

    pub fn toggle_enabled(&mut self, id: &str) -> Option<bool> {
        if let Some(f) = self.filters.iter_mut().find(|f| f.id == id) {
            f.enabled = !f.enabled;
            Some(f.enabled)
        } else {
            None
        }
    }

    pub fn reset_filters(&mut self) {
        self.active_filter_id = None;
        for f in &mut self.filters {
            f.enabled = false;
        }
    }

    pub fn revert(&mut self, default_filters: Vec<SavedFilter>) {
        self.filters = default_filters;
        self.active_filter_id = None;
    }

    pub fn matches(&self, session: &SessionRecord) -> bool {
        let active_filters: Vec<&SavedFilter> = self.filters.iter().filter(|f| f.enabled).collect();
        if active_filters.is_empty() {
            return true;
        }
        active_filters.iter().all(|f| f.rule.matches(session))
    }
}

// ---------------------------------------------------------------------------
// 3. Traffic Groups Panel (Group by Host, Process, Protocol, Method, StatusClass, ContentType)
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupBy {
    Host,
    Process,
    Protocol,
    Method,
    StatusClass,
    ContentType,
}

impl GroupBy {
    pub const ALL: [GroupBy; 6] = [GroupBy::Host, GroupBy::Process, GroupBy::Protocol, GroupBy::Method,
                                   GroupBy::StatusClass, GroupBy::ContentType];

    pub fn label(&self) -> &'static str {
        match self {
            GroupBy::Host => "Host",
            GroupBy::Process => "Process",
            GroupBy::Protocol => "Protocol",
            GroupBy::Method => "Method",
            GroupBy::StatusClass => "Status class",
            GroupBy::ContentType => "Content type",
        }
    }

    pub fn key_for(&self, s: &SessionRecord) -> String {
        match self {
            GroupBy::Host => {
                if s.host().is_empty() { "(no host)".to_string() } else { s.host().to_string() }
            }
            GroupBy::Process => {
                s.process_name.clone().unwrap_or_else(|| "(unknown)".to_string())
            }
            GroupBy::Protocol => {
                if s.url.starts_with("https:") || s.tls_version.is_some() {
                    "HTTPS".to_string()
                } else if s.url.starts_with("ws:") || s.url.starts_with("wss:") {
                    "WebSocket".to_string()
                } else {
                    "HTTP".to_string()
                }
            }
            GroupBy::Method => s.method.clone(),
            GroupBy::StatusClass => {
                let st = s.status;
                if (200..300).contains(&st) {
                    "2xx Success".to_string()
                } else if (300..400).contains(&st) {
                    "3xx Redirection".to_string()
                } else if (400..500).contains(&st) {
                    "4xx Client Error".to_string()
                } else if (500..600).contains(&st) {
                    "5xx Server Error".to_string()
                } else {
                    "Other".to_string()
                }
            }
            GroupBy::ContentType => {
                let c = s.content_type().split(';').next().unwrap_or("").trim();
                if c.is_empty() { "(none)".to_string() } else { c.to_string() }
            }
        }
    }
}


#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TrafficGroup {
    pub key: String,
    pub display_name: String,
    pub session_ids: Vec<usize>,
    pub pinned: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct TrafficGroupsManager {
    pub group_by: Option<GroupBy>,
    pub pinned_keys: HashSet<String>,
    pub selected_group: Option<String>,
    pub groups: Vec<TrafficGroup>,
    /// What `groups` was last built from (see `refresh`), so the panel doesn't regroup every frame.
    #[serde(skip)]
    pub built_from: Option<(usize, usize, usize, u64, Option<GroupBy>, Vec<String>)>,
}

impl TrafficGroupsManager {
    pub fn new() -> Self {
        Self {
            group_by: Some(GroupBy::Host),
            pinned_keys: HashSet::new(),
            selected_group: None,
            groups: Vec::new(),
            built_from: None,
        }
    }

    /// Rebuild the groups only if the sessions, the grouping or the pins changed since the last build
    /// (UI-REVIEW.md: the panel regrouped and cloned every row 60 times a second). Returns true if it rebuilt.
    pub fn refresh(&mut self, sessions: &[SessionRecord]) -> bool {
        let mut pins: Vec<String> = self.pinned_keys.iter().cloned().collect();
        pins.sort();
        // statuses and durations change while a session completes, so they are part of the fingerprint
        let mix = sessions.iter().fold(0u64, |h, s| {
            h.wrapping_mul(31).wrapping_add(s.status as u64 * 1_000_003 + s.duration_ms as u64 + s.id as u64)
        });
        let stamp = (sessions.len(), sessions.first().map_or(0, |s| s.id), sessions.last().map_or(0, |s| s.id),
                     mix, self.group_by, pins);
        if self.built_from.as_ref() == Some(&stamp) {
            return false;
        }
        self.rebuild(sessions);
        self.built_from = Some(stamp);
        true
    }

    /// Session ids in the group with this key (from the last build).
    pub fn ids_of(&self, key: &str) -> Vec<usize> {
        self.groups.iter().find(|g| g.key == key).map(|g| g.session_ids.clone()).unwrap_or_default()
    }

    pub fn set_group_by(&mut self, gb: GroupBy) {
        self.group_by = Some(gb);
    }

    pub fn pin_group(&mut self, key: &str) {
        self.pinned_keys.insert(key.to_string());
    }

    pub fn unpin_group(&mut self, key: &str) {
        self.pinned_keys.remove(key);
    }

    pub fn toggle_pin(&mut self, key: &str) -> bool {
        if self.pinned_keys.contains(key) {
            self.pinned_keys.remove(key);
            false
        } else {
            self.pinned_keys.insert(key.to_string());
            true
        }
    }

    pub fn rebuild(&mut self, sessions: &[SessionRecord]) {
        self.groups = self.group_sessions(sessions);
    }

    pub fn group_sessions(&self, sessions: &[SessionRecord]) -> Vec<TrafficGroup> {
        let mode = self.group_by.unwrap_or(GroupBy::Host);
        let mut map: BTreeMap<String, Vec<usize>> = BTreeMap::new();

        for s in sessions {
            let key = match mode {
                GroupBy::Host => {
                    if s.host().is_empty() { "(no host)".to_string() } else { s.host().to_string() }
                }
                GroupBy::Process => {
                    s.process_name.clone().unwrap_or_else(|| "(unknown)".to_string())
                }
                GroupBy::Protocol => {
                    if s.url.starts_with("https:") || s.tls_version.is_some() {
                        "HTTPS".to_string()
                    } else if s.url.starts_with("ws:") || s.url.starts_with("wss:") {
                        "WebSocket".to_string()
                    } else {
                        "HTTP".to_string()
                    }
                }
                GroupBy::Method => s.method.clone(),
                GroupBy::StatusClass => {
                    let st = s.status;
                    if (200..300).contains(&st) { "2xx Success".to_string() }
                    else if (300..400).contains(&st) { "3xx Redirection".to_string() }
                    else if (400..500).contains(&st) { "4xx Client Error".to_string() }
                    else if (500..600).contains(&st) { "5xx Server Error".to_string() }
                    else { "Other".to_string() }
                }
                GroupBy::ContentType => {
                    { let c = s.content_type().split(';').next().unwrap_or("").trim(); if c.is_empty() { "(none)".to_string() } else { c.to_string() } }
                }
            };
            map.entry(key).or_default().push(s.id);
        }

        let mut result = Vec::new();
        for (key, ids) in map {
            let pinned = self.pinned_keys.contains(&key);
            let display_name = format!("{} ({})", key, ids.len());
            result.push(TrafficGroup {
                key,
                display_name,
                session_ids: ids,
                pinned,
            });
        }

        // Pinned groups sort to top
        result.sort_by(|a, b| b.pinned.cmp(&a.pinned).then_with(|| a.key.cmp(&b.key)));
        result
    }
}

// ---------------------------------------------------------------------------
// 4. Streaming Mode State & Buffer
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct StreamBuffer {
    pub session_id: usize,
    pub chunks: Vec<Vec<u8>>,
    pub total_bytes: usize,
    pub complete: bool,
}

impl StreamBuffer {
    pub fn new(session_id: usize) -> Self {
        Self {
            session_id,
            chunks: Vec::new(),
            total_bytes: 0,
            complete: false,
        }
    }

    pub fn append_chunk(&mut self, chunk: &[u8]) {
        self.total_bytes += chunk.len();
        self.chunks.push(chunk.to_vec());
    }

    pub fn mark_complete(&mut self) {
        self.complete = true;
    }
}

#[derive(Debug, Default)]
pub struct StreamingState {
    pub active_streams: BTreeMap<usize, StreamBuffer>,
}

impl StreamingState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn handle_chunk(&mut self, session_id: usize, chunk: &[u8], is_last: bool) {
        let buf = self.active_streams.entry(session_id).or_insert_with(|| StreamBuffer::new(session_id));
        buf.append_chunk(chunk);
        if is_last {
            buf.mark_complete();
        }
    }

    pub fn get_stream(&self, session_id: usize) -> Option<&StreamBuffer> {
        self.active_streams.get(&session_id)
    }
}

// ---------------------------------------------------------------------------
// 5. Autocomplete Index for Filter Inputs
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone)]
pub struct FilterAutocompleteIndex {
    pub hosts: HashSet<String>,
    pub methods: HashSet<String>,
    pub paths: HashSet<String>,
    pub headers: HashSet<String>,
    pub content_types: HashSet<String>,
    pub statuses: HashSet<String>,
}

impl FilterAutocompleteIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn index_session(&mut self, session: &SessionRecord) {
        self.statuses.insert(session.status.to_string());
        if !session.host().is_empty() {
            self.hosts.insert(session.host().to_string());
        }
        if !session.method.is_empty() {
            self.methods.insert(session.method.clone());
        }
        if !session.path().is_empty() {
            self.paths.insert(session.path().to_string());
        }
        if let Some(ct) = Some(session.content_type()).filter(|c| !c.is_empty()) {
            self.content_types.insert(ct.to_string());
        }
        for k in session.request_headers.keys() {
            self.headers.insert(k.clone());
        }
        for k in session.response_headers.keys() {
            self.headers.insert(k.clone());
        }
    }

    pub fn suggest(&self, field: &FilterField, prefix: &str) -> Vec<String> {
        let p = prefix.to_ascii_lowercase();
        let matches: Vec<String> = match field {
            FilterField::Host => self.hosts.iter().filter(|s| s.to_ascii_lowercase().starts_with(&p)).cloned().collect(),
            FilterField::Method => self.methods.iter().filter(|s| s.to_ascii_lowercase().starts_with(&p)).cloned().collect(),
            FilterField::Path => self.paths.iter().filter(|s| s.to_ascii_lowercase().starts_with(&p)).cloned().collect(),
            FilterField::Status => self.statuses.iter().filter(|s| s.starts_with(&p)).cloned().collect(),
            FilterField::ContentType => self.content_types.iter().filter(|s| s.to_ascii_lowercase().starts_with(&p)).cloned().collect(),
            FilterField::ReqHeader(_) | FilterField::ResHeader(_) => self.headers.iter().filter(|s| s.to_ascii_lowercase().starts_with(&p)).cloned().collect(),
            _ => Vec::new(),
        };
        matches
    }
}

// ---------------------------------------------------------------------------
// 6. Traffic Tabs (Multi-tab captures)
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TrafficTab {
    pub id: String,
    pub name: String,
    pub session_ids: Vec<usize>,
    pub is_recording: bool,
    pub filter: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TrafficTabsManager {
    pub tabs: Vec<TrafficTab>,
    pub active_tab_index: usize,
    pub show_side_panel: bool,
}

impl Default for TrafficTabsManager {
    fn default() -> Self {
        Self::new()
    }
}

impl TrafficTabsManager {
    pub fn new() -> Self {
        Self {
            tabs: vec![TrafficTab {
                id: "tab-default".to_string(),
                name: "Main Capture".to_string(),
                session_ids: Vec::new(),
                is_recording: true,
                filter: None,
            }],
            active_tab_index: 0,
            show_side_panel: false,
        }
    }

    pub fn active_tab(&self) -> &TrafficTab {
        &self.tabs[self.active_tab_index]
    }

    pub fn active_tab_mut(&mut self) -> &mut TrafficTab {
        &mut self.tabs[self.active_tab_index]
    }

    pub fn create_tab(&mut self, name: &str) -> usize {
        let id = format!("tab-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        self.tabs.push(TrafficTab {
            id,
            name: name.to_string(),
            session_ids: Vec::new(),
            is_recording: true,
            filter: None,
        });
        self.active_tab_index = self.tabs.len() - 1;
        self.active_tab_index
    }

    /// Close tab `index`; the main capture tab (index 0) stays.
    pub fn close_tab(&mut self, index: usize) {
        if index > 0 && index < self.tabs.len() {
            self.tabs.remove(index);
            if self.active_tab_index == index {
                self.active_tab_index = index - 1;
            } else if self.active_tab_index > index {
                self.active_tab_index -= 1;
            }
        }
    }

    pub fn toggle_side_panel(&mut self) -> bool {
        self.show_side_panel = !self.show_side_panel;
        self.show_side_panel
    }
}

impl SavedFiltersManager {
    /// Where saved filters are stored (override with FERROTAP_SAVED_FILTERS).
    pub fn default_path() -> std::path::PathBuf {
        std::env::var("FERROTAP_SAVED_FILTERS").map(std::path::PathBuf::from).unwrap_or_else(|_| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data").join("saved_filters.json")
        })
    }
    pub fn load(path: &std::path::Path) -> Self {
        std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }
    pub fn save(&self, path: &std::path::Path) -> Result<(), String> {
        if let Some(d) = path.parent() { let _ = std::fs::create_dir_all(d); }
        let j = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, j).map_err(|e| e.to_string())
    }
    /// The filter to apply at startup: the last applied rule, else the active saved filter.
    pub fn restored_rule(&self) -> Option<AdvancedFilterRule> {
        self.active_rule.clone().or_else(|| {
            let id = self.active_filter_id.as_ref()?;
            self.filters.iter().find(|f| &f.id == id).map(|f| f.rule.clone())
        })
    }
}