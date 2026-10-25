//! G18 (+ G3 overview): passive traffic insights - errors, duplicates, compression, sizes, headers, cookies.
//! Owned by one builder; the app calls `insights_menu` from the toolbar and `insights_windows` every frame.
//!
//! Everything in this file is a pure function over captured sessions, so it can be tested on hand-built
//! `SessionRecord`s; the menu and the windows that show the results live in `insights_ui.rs`.
use crate::model::SessionRecord;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// This module's UI state (kept in `GridApp::ext.insights`).
pub struct State {
    pub show_overview: bool,
    pub show_waterfall: bool,
    pub show_errors: bool,
    pub show_requests: bool,
    pub show_sizes: bool,
    pub show_hygiene: bool,
    pub show_cookies: bool,
    /// Overview: statistics of the selected session only (instead of every visible session).
    pub overview_selected_only: bool,
    /// Waterfall: sessions at or above this duration are highlighted as slow.
    pub slow_ms: u32,
    /// Waterfall: index of the page shown.
    pub page: usize,
    /// Sizes window: how many entries the "largest payloads" list shows.
    pub top_n: usize,
    /// Third-party summary: the page's main site; empty = detect automatically.
    pub main_site: String,
    /// Overview preferences: units and which timing phases to show.
    pub prefs: OverviewPrefs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TimeUnit {
    #[default]
    Ms,
    Seconds,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SizeUnit {
    #[default]
    Auto,
    Bytes,
    Kb,
    Mb,
}

/// Which timing phases the Overview shows: DNS, connect, TLS, TTFB, receive.
#[derive(Debug, Clone, PartialEq)]
pub struct OverviewPrefs {
    pub time: TimeUnit,
    pub size: SizeUnit,
    pub phases: [bool; 5],
}

impl Default for OverviewPrefs {
    fn default() -> Self {
        OverviewPrefs { time: TimeUnit::Ms, size: SizeUnit::Auto, phases: [true; 5] }
    }
}

pub const PHASES: [&str; 5] = ["DNS", "TCP connect", "TLS handshake", "Server (TTFB)", "Receive"];

pub fn fmt_time(ms: f64, u: TimeUnit) -> String {
    match u {
        TimeUnit::Ms => format!("{ms:.0} ms"),
        TimeUnit::Seconds => format!("{:.2} s", ms / 1000.0),
    }
}

pub fn fmt_size(n: u64, u: SizeUnit) -> String {
    match u {
        SizeUnit::Auto => fmt_bytes(n),
        SizeUnit::Bytes => format!("{n} B"),
        SizeUnit::Kb => format!("{:.1} KB", n as f64 / 1024.0),
        SizeUnit::Mb => format!("{:.3} MB", n as f64 / (1024.0 * 1024.0)),
    }
}

/// Average of each timing phase over the sessions that measured it: (phase, average ms, sessions).
pub fn phase_averages(rows: &[&SessionRecord]) -> Vec<(&'static str, f64, usize)> {
    let mut sums = [0f64; 5];
    let mut counts = [0usize; 5];
    for r in rows {
        let measured = [r.dns_ms, r.connect_ms, r.tls_ms, r.ttfb_ms];
        let mut before = 0u64;
        for (i, v) in measured.iter().enumerate() {
            if let Some(v) = v {
                sums[i] += *v as f64;
                counts[i] += 1;
                before += v;
            }
        }
        if r.ttfb_ms.is_some() {
            let rest = (r.duration_ms as u64).saturating_sub(before);
            sums[4] += rest as f64;
            counts[4] += 1;
        }
    }
    (0..5).map(|i| (PHASES[i], if counts[i] > 0 { sums[i] / counts[i] as f64 } else { 0.0 }, counts[i])).collect()
}

impl Default for State {
    fn default() -> Self {
        Self {
            show_overview: false,
            show_waterfall: false,
            show_errors: false,
            show_requests: false,
            show_sizes: false,
            show_hygiene: false,
            show_cookies: false,
            overview_selected_only: false,
            slow_ms: 1000,
            page: 0,
            top_n: 10,
            main_site: String::new(),
            prefs: OverviewPrefs::default(),
        }
    }
}

impl State {
    pub fn any_open(&self) -> bool {
        self.show_overview || self.show_waterfall || self.show_errors || self.show_requests || self.show_sizes || self.show_hygiene || self.show_cookies
    }
}

// ---------------------------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------------------------

/// Start time as milliseconds since the Unix epoch (RFC 3339 as the proxy writes it, or "YYYY-MM-DD HH:MM:SS.fff").
pub fn start_epoch_ms(r: &SessionRecord) -> Option<i64> {
    let s = r.start_time.trim();
    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(t.timestamp_millis());
    }
    for f in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(t) = chrono::NaiveDateTime::parse_from_str(s, f) {
            return Some(t.and_utc().timestamp_millis());
        }
    }
    None
}

/// Bytes of the response as transferred (wire size when known, otherwise the stored body).
pub fn response_size(r: &SessionRecord) -> usize {
    r.response_raw_size.unwrap_or(r.response_body.len())
}

/// Bytes of the response after decoding (the size a client works with).
pub fn decoded_size(r: &SessionRecord) -> usize {
    r.response_decoded_size.or(r.response_raw_size).unwrap_or(r.response_body.len())
}

/// Host name without the port (keeps IPv6 brackets), lowercase.
pub fn host_name(r: &SessionRecord) -> String {
    strip_port(r.host()).to_ascii_lowercase()
}

fn strip_port(h: &str) -> &str {
    if h.starts_with('[') {
        return match h.find(']') { Some(i) => &h[..=i], None => h };
    }
    match h.rsplit_once(':') {
        Some((a, p)) if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) => a,
        _ => h,
    }
}

/// Tunnel placeholders (CONNECT rows that were passed through without decryption) carry no HTTP content.
pub fn is_tunnel(r: &SessionRecord) -> bool {
    r.method.eq_ignore_ascii_case("CONNECT") || r.url.starts_with("tunnel://")
}

/// "2xx Success", "4xx Client error", ... or "No response" for status 0.
pub fn status_class(status: u16) -> &'static str {
    match status {
        0 => "No response",
        100..=199 => "1xx Informational",
        200..=299 => "2xx Success",
        300..=399 => "3xx Redirect",
        400..=499 => "4xx Client error",
        500..=599 => "5xx Server error",
        _ => "Other",
    }
}

/// "1.5 KB" style size text.
pub fn fmt_bytes(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{:.2} MB", n as f64 / (1024.0 * 1024.0))
    }
}

fn count_sorted(map: HashMap<String, usize>) -> Vec<(String, usize)> {
    let mut v: Vec<(String, usize)> = map.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v
}

// ---------------------------------------------------------------------------------------------
// G3 Overview
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Overview {
    pub sessions: usize,
    pub by_status: Vec<(String, usize)>,
    pub by_content_type: Vec<(String, usize)>,
    pub by_host: Vec<(String, usize)>,
    pub by_process: Vec<(String, usize)>,
    /// Sum of request body bytes.
    pub request_bytes: u64,
    /// Sum of response bytes as transferred.
    pub response_bytes: u64,
    /// First start time (as captured) and the span from the first start to the last end, in ms.
    pub first_start: Option<String>,
    pub span_ms: Option<i64>,
    pub avg_ms: f64,
    pub max_ms: u128,
    /// Session with the longest duration.
    pub max_id: Option<usize>,
    /// Duration histogram (bucket label, count), every bucket in order.
    pub duration_histogram: Vec<(String, usize)>,
    /// Response size histogram (bucket label, count), every bucket in order.
    pub size_histogram: Vec<(String, usize)>,
}

pub const DURATION_BUCKETS: [(&str, u128); 6] =
    [("< 50 ms", 50), ("50-200 ms", 200), ("200-500 ms", 500), ("0.5-1 s", 1000), ("1-3 s", 3000), ("> 3 s", u128::MAX)];
pub const SIZE_BUCKETS: [(&str, u64); 5] =
    [("< 1 KB", 1024), ("1-10 KB", 10 * 1024), ("10-100 KB", 100 * 1024), ("100 KB-1 MB", 1024 * 1024), ("> 1 MB", u64::MAX)];

pub fn overview(rows: &[&SessionRecord]) -> Overview {
    let mut o = Overview { sessions: rows.len(), ..Default::default() };
    let (mut st, mut ct, mut hosts, mut procs) = (HashMap::new(), HashMap::new(), HashMap::new(), HashMap::new());
    let mut dur = vec![0usize; DURATION_BUCKETS.len()];
    let mut size = vec![0usize; SIZE_BUCKETS.len()];
    let mut total_ms: u128 = 0;
    let (mut first, mut last): (Option<(i64, &str)>, Option<i64>) = (None, None);
    for r in rows {
        *st.entry(status_class(r.status).to_string()).or_insert(0) += 1;
        let c = r.content_type();
        *ct.entry(if c.is_empty() { "(none)".to_string() } else { c.to_ascii_lowercase() }).or_insert(0) += 1;
        *hosts.entry(host_name(r)).or_insert(0) += 1;
        *procs.entry(r.process_name.clone().filter(|p| !p.is_empty()).unwrap_or_else(|| "(unknown)".into())).or_insert(0) += 1;
        o.request_bytes += r.request_body.len() as u64;
        let rs = response_size(r) as u64;
        o.response_bytes += rs;
        total_ms += r.duration_ms;
        if r.duration_ms > o.max_ms || o.max_id.is_none() {
            o.max_ms = r.duration_ms;
            o.max_id = Some(r.id);
        }
        dur[DURATION_BUCKETS.iter().position(|(_, lim)| r.duration_ms < *lim).unwrap_or(DURATION_BUCKETS.len() - 1)] += 1;
        size[SIZE_BUCKETS.iter().position(|(_, lim)| rs < *lim).unwrap_or(SIZE_BUCKETS.len() - 1)] += 1;
        if let Some(t) = start_epoch_ms(r) {
            if first.is_none_or(|(f, _)| t < f) {
                first = Some((t, r.start_time.as_str()));
            }
            let end = t + r.duration_ms as i64;
            if last.is_none_or(|l| end > l) {
                last = Some(end);
            }
        }
    }
    if !rows.is_empty() {
        o.avg_ms = total_ms as f64 / rows.len() as f64;
    }
    if let (Some((f, s)), Some(l)) = (first, last) {
        o.first_start = Some(s.to_string());
        o.span_ms = Some(l - f);
    }
    o.by_status = count_sorted(st);
    o.by_content_type = count_sorted(ct);
    o.by_host = count_sorted(hosts);
    o.by_process = count_sorted(procs);
    o.duration_histogram = DURATION_BUCKETS.iter().zip(dur).map(|((l, _), n)| (l.to_string(), n)).collect();
    o.size_histogram = SIZE_BUCKETS.iter().zip(size).map(|((l, _), n)| (l.to_string(), n)).collect();
    o
}

/// Plain-text form of an overview (for the Copy button).
pub fn overview_text(o: &Overview) -> String {
    let mut s = format!("Sessions: {}\n", o.sessions);
    s += &format!("Request bytes: {}\nResponse bytes: {}\n", o.request_bytes, o.response_bytes);
    if let (Some(f), Some(span)) = (&o.first_start, o.span_ms) {
        s += &format!("First start: {f}\nTime span: {span} ms\n");
    }
    s += &format!("Average duration: {:.0} ms\nMax duration: {} ms", o.avg_ms, o.max_ms);
    if let Some(id) = o.max_id {
        s += &format!(" (session #{id})");
    }
    s.push('\n');
    for (title, list) in [
        ("Status", &o.by_status),
        ("Content type", &o.by_content_type),
        ("Host", &o.by_host),
        ("Process", &o.by_process),
        ("Duration", &o.duration_histogram),
        ("Response size", &o.size_histogram),
    ] {
        s += &format!("\n{title}:\n");
        for (k, n) in list.iter() {
            s += &format!("  {k}\t{n}\n");
        }
    }
    s
}

// ---------------------------------------------------------------------------------------------
// Waterfall per page + slowest hosts
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Bar {
    pub id: usize,
    pub label: String,
    /// Offset of the start from the page's first request, in ms.
    pub offset_ms: i64,
    pub duration_ms: u128,
    pub status: u16,
    pub slow: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    /// Page URL, or `NO_PAGE`.
    pub url: String,
    pub bars: Vec<Bar>,
    /// From the first start to the last end, in ms.
    pub total_ms: i64,
}

pub const NO_PAGE: &str = "(requests without a page)";

/// An HTML document loaded with GET starts a new page.
pub fn is_document(r: &SessionRecord) -> bool {
    r.method.eq_ignore_ascii_case("GET") && r.content_type().to_ascii_lowercase().contains("html")
}

fn strip_fragment(u: &str) -> &str {
    u.split('#').next().unwrap_or(u)
}

fn url_host(u: &str) -> String {
    let rest = u.split_once("://").map(|x| x.1).unwrap_or(u);
    strip_port(&rest[..rest.find('/').unwrap_or(rest.len())]).to_ascii_lowercase()
}

/// Groups sessions into pages: each HTML document starts a page; other requests join the page named by their
/// Referer (exact URL, else the latest page on the Referer's host), or the latest page on their own host.
/// Bars are in start order; sessions at or above `slow_ms` are flagged slow.
pub fn pages(rows: &[&SessionRecord], slow_ms: u128) -> Vec<Page> {
    let mut order: Vec<&SessionRecord> = rows.iter().copied().filter(|r| !is_tunnel(r)).collect();
    order.sort_by_key(|r| (start_epoch_ms(r).unwrap_or(i64::MAX), r.id));
    let mut groups: Vec<(String, Vec<&SessionRecord>)> = Vec::new();
    for r in order {
        let target = if is_document(r) {
            None
        } else if let Some(rf) = r.request_header("referer") {
            let rf = strip_fragment(rf);
            let rh = url_host(rf);
            groups
                .iter()
                .rposition(|(u, _)| u == rf)
                .or_else(|| groups.iter().rposition(|(u, _)| u != NO_PAGE && url_host(u) == rh))
        } else {
            let h = host_name(r);
            groups.iter().rposition(|(u, _)| u != NO_PAGE && url_host(u) == h)
        };
        match target {
            Some(i) => groups[i].1.push(r),
            None if is_document(r) => groups.push((strip_fragment(&r.url).to_string(), vec![r])),
            None => match groups.iter().position(|(u, _)| u == NO_PAGE) {
                Some(i) => groups[i].1.push(r),
                None => groups.push((NO_PAGE.to_string(), vec![r])),
            },
        }
    }
    groups
        .into_iter()
        .map(|(url, list)| {
            let t0 = list.iter().filter_map(|r| start_epoch_ms(r)).min().unwrap_or(0);
            let bars: Vec<Bar> = list
                .iter()
                .map(|r| Bar {
                    id: r.id,
                    label: format!("{} {}", r.method, r.url),
                    offset_ms: start_epoch_ms(r).map(|t| t - t0).unwrap_or(0),
                    duration_ms: r.duration_ms,
                    status: r.status,
                    slow: r.duration_ms >= slow_ms,
                })
                .collect();
            let total_ms = bars.iter().map(|b| b.offset_ms + b.duration_ms as i64).max().unwrap_or(0);
            Page { url, bars, total_ms }
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct HostTiming {
    pub host: String,
    pub count: usize,
    pub avg_ms: f64,
    pub max_ms: u128,
    pub total_ms: u128,
    /// The host's slowest session.
    pub slowest_id: usize,
}

/// Hosts ordered by average duration, slowest first.
pub fn slowest_hosts(rows: &[&SessionRecord]) -> Vec<HostTiming> {
    let mut m: BTreeMap<String, HostTiming> = BTreeMap::new();
    for r in rows.iter().filter(|r| !is_tunnel(r)) {
        let e = m.entry(host_name(r)).or_insert_with(|| HostTiming { host: host_name(r), count: 0, avg_ms: 0.0, max_ms: 0, total_ms: 0, slowest_id: r.id });
        e.count += 1;
        e.total_ms += r.duration_ms;
        if r.duration_ms > e.max_ms {
            e.max_ms = r.duration_ms;
            e.slowest_id = r.id;
        }
    }
    let mut v: Vec<HostTiming> = m
        .into_values()
        .map(|mut h| {
            h.avg_ms = h.total_ms as f64 / h.count as f64;
            h
        })
        .collect();
    v.sort_by(|a, b| b.avg_ms.partial_cmp(&a.avg_ms).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.host.cmp(&b.host)));
    v
}

// ---------------------------------------------------------------------------------------------
// Error dashboard
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// No response at all (status 0), or a 502 because the server could not be reached.
    FailedConnection,
    /// The TLS handshake / certificate check with the server failed.
    TlsError,
    /// 504 / 408, or a recorded "timed out".
    Timeout,
    /// The user aborted the exchange at a breakpoint.
    Aborted,
}

impl FailureKind {
    pub fn title(self) -> &'static str {
        match self {
            FailureKind::FailedConnection => "Failed connection",
            FailureKind::TlsError => "TLS error",
            FailureKind::Timeout => "Timeout",
            FailureKind::Aborted => "Aborted at breakpoint",
        }
    }
}

/// How a failure shows up in a session: status 0 (no response), status 502/504/408, and the session's comment,
/// response body or Ferrotap's own `x-ferrotap*` headers naming the cause.
pub fn failure_kind(r: &SessionRecord) -> Option<FailureKind> {
    if is_tunnel(r) {
        return None;
    }
    let mut text = r.comment.to_ascii_lowercase();
    text.push(' ');
    text.push_str(&r.response_body.chars().take(512).collect::<String>().to_ascii_lowercase());
    for (k, v) in &r.response_headers {
        if k.to_ascii_lowercase().starts_with("x-ferrotap") {
            text.push(' ');
            text.push_str(&v.to_ascii_lowercase());
        }
    }
    let has = |words: &[&str]| words.iter().any(|w| text.contains(w));
    let failed_status = r.status == 0 || r.status >= 500;
    if has(&["breakpoint aborted", "aborted at breakpoint"]) {
        return Some(FailureKind::Aborted);
    }
    if failed_status && has(&["tls", "ssl", "certificate", "handshake"]) {
        return Some(FailureKind::TlsError);
    }
    if r.status == 504 || r.status == 408 || ((failed_status || !r.comment.is_empty()) && has(&["timed out", "timeout"])) {
        return Some(FailureKind::Timeout);
    }
    if r.status == 0
        || (r.status == 502
            && (r.response_headers.is_empty() || has(&["connect", "refused", "unreachable", "dns", "reset", "resolve"])))
    {
        return Some(FailureKind::FailedConnection);
    }
    None
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ErrorReport {
    pub client_errors: Vec<usize>,
    pub server_errors: Vec<usize>,
    pub failed_connections: Vec<usize>,
    pub tls_errors: Vec<usize>,
    pub timeouts: Vec<usize>,
    pub aborted: Vec<usize>,
    /// Count per error status code (4xx and 5xx), ascending by code.
    pub by_status: Vec<(u16, usize)>,
}

pub fn error_report(rows: &[&SessionRecord]) -> ErrorReport {
    let mut e = ErrorReport::default();
    let mut codes: BTreeMap<u16, usize> = BTreeMap::new();
    for r in rows.iter().filter(|r| !is_tunnel(r)) {
        match r.status {
            400..=499 => e.client_errors.push(r.id),
            500..=599 => e.server_errors.push(r.id),
            _ => {}
        }
        if (400..=599).contains(&r.status) {
            *codes.entry(r.status).or_insert(0) += 1;
        }
        match failure_kind(r) {
            Some(FailureKind::FailedConnection) => e.failed_connections.push(r.id),
            Some(FailureKind::TlsError) => e.tls_errors.push(r.id),
            Some(FailureKind::Timeout) => e.timeouts.push(r.id),
            Some(FailureKind::Aborted) => e.aborted.push(r.id),
            None => {}
        }
    }
    e.by_status = codes.into_iter().collect();
    e
}

// ---------------------------------------------------------------------------------------------
// Duplicates and caching
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct DuplicateGroup {
    pub method: String,
    pub url: String,
    pub body_len: usize,
    pub ids: Vec<usize>,
    /// Response bytes of the repeats (every copy after the first).
    pub repeat_bytes: u64,
}

/// Requests sent more than once with the same method, URL and body; most repeated first.
pub fn duplicates(rows: &[&SessionRecord]) -> Vec<DuplicateGroup> {
    let mut idx: HashMap<(String, &str, &str), usize> = HashMap::new();
    let mut groups: Vec<DuplicateGroup> = Vec::new();
    for r in rows.iter().filter(|r| !is_tunnel(r)) {
        let key = (r.method.to_ascii_uppercase(), r.url.as_str(), r.request_body.as_str());
        match idx.get(&key).copied() {
            Some(i) => {
                groups[i].ids.push(r.id);
                groups[i].repeat_bytes += response_size(r) as u64;
            }
            None => {
                idx.insert(key, groups.len());
                groups.push(DuplicateGroup { method: r.method.to_ascii_uppercase(), url: r.url.clone(), body_len: r.request_body.len(), ids: vec![r.id], repeat_bytes: 0 });
            }
        }
    }
    groups.retain(|g| g.ids.len() > 1);
    groups.sort_by(|a, b| b.ids.len().cmp(&a.ids.len()).then_with(|| b.repeat_bytes.cmp(&a.repeat_bytes)).then_with(|| a.ids[0].cmp(&b.ids[0])));
    groups
}

/// GET with a 200 response: the responses a browser could keep in its cache.
pub fn is_cache_candidate(r: &SessionRecord) -> bool {
    !is_tunnel(r) && r.method.eq_ignore_ascii_case("GET") && r.status == 200
}

/// A cacheable response that says nothing about caching: no Cache-Control, Expires, ETag or Last-Modified.
pub fn lacks_cache_headers(r: &SessionRecord) -> bool {
    is_cache_candidate(r) && ["cache-control", "expires", "etag", "last-modified"].iter().all(|h| r.response_header(h).is_none())
}

fn explain_directive(d: &str) -> Option<String> {
    let (name, val) = match d.split_once('=') {
        Some((n, v)) => (n.trim().to_ascii_lowercase(), v.trim().trim_matches('"').to_string()),
        None => (d.trim().to_ascii_lowercase(), String::new()),
    };
    Some(match name.as_str() {
        "max-age" => format!("max-age={val}: may be reused without asking the server for {val} seconds."),
        "s-maxage" => format!("s-maxage={val}: shared caches (CDNs, proxies) may reuse it for {val} seconds."),
        "no-cache" => "no-cache: may be stored, but must be revalidated with the server before every reuse.".into(),
        "no-store" => "no-store: must not be stored by any cache.".into(),
        "private" => "private: only the user's own browser may cache it, not shared caches.".into(),
        "public" => "public: shared caches may store it too.".into(),
        "must-revalidate" => "must-revalidate: once stale it must not be used without checking with the server.".into(),
        "proxy-revalidate" => "proxy-revalidate: like must-revalidate, for shared caches only.".into(),
        "immutable" => "immutable: will not change while fresh, so no revalidation on reload.".into(),
        "no-transform" => "no-transform: intermediaries must not modify (e.g. recompress) it.".into(),
        "stale-while-revalidate" => format!("stale-while-revalidate={val}: a stale copy may be shown for {val} s while a fresh one is fetched."),
        "stale-if-error" => format!("stale-if-error={val}: a stale copy may be used for {val} s if the server errors."),
        "" => return None,
        other => format!("{other}: unrecognised directive."),
    })
}

/// Plain-language explanation of the caching headers present on a response.
pub fn explain_cache_headers(r: &SessionRecord) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(cc) = r.response_header("cache-control") {
        out.push(format!("Cache-Control: {cc}"));
        out.extend(cc.split(',').filter_map(explain_directive).map(|s| format!("  - {s}")));
    }
    if let Some(v) = r.response_header("expires") {
        out.push(format!("Expires: {v} - considered fresh until this date (Cache-Control max-age wins if both are set)."));
    }
    if let Some(v) = r.response_header("etag") {
        out.push(format!("ETag: {v} - a version tag; the browser can ask \"has it changed?\" with If-None-Match and get a short 304."));
    }
    if let Some(v) = r.response_header("last-modified") {
        out.push(format!("Last-Modified: {v} - lets the browser revalidate with If-Modified-Since and get a short 304."));
    }
    if let Some(v) = r.response_header("vary") {
        out.push(format!("Vary: {v} - a cached copy is only reused for requests with the same values of these headers."));
    }
    if let Some(v) = r.response_header("age") {
        out.push(format!("Age: {v} - the response already spent {v} s in a shared cache."));
    }
    if let Some(v) = r.response_header("pragma") {
        out.push(format!("Pragma: {v} - legacy HTTP/1.0 header; \"no-cache\" acts like Cache-Control: no-cache for old caches."));
    }
    if out.is_empty() {
        out.push("No caching headers: browsers may guess a freshness lifetime (heuristic caching) and cannot revalidate cheaply. Adding Cache-Control (and an ETag or Last-Modified) makes the behaviour explicit.".into());
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
pub struct CacheEntry {
    pub id: usize,
    pub url: String,
    /// Cacheable but sent without any caching header.
    pub missing: bool,
    pub notes: Vec<String>,
}

/// Every GET 200 response with its caching headers explained; the ones without any caching header first.
pub fn cache_analysis(rows: &[&SessionRecord]) -> Vec<CacheEntry> {
    let mut v: Vec<CacheEntry> = rows
        .iter()
        .filter(|r| is_cache_candidate(r))
        .map(|r| CacheEntry { id: r.id, url: r.url.clone(), missing: lacks_cache_headers(r), notes: explain_cache_headers(r) })
        .collect();
    v.sort_by_key(|e| (!e.missing, e.id));
    v
}

// ---------------------------------------------------------------------------------------------
// Sizes: compression, large payloads, images
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct SizeItem {
    pub id: usize,
    pub url: String,
    pub content_type: String,
    pub bytes: usize,
}

fn size_item(r: &SessionRecord, bytes: usize) -> SizeItem {
    SizeItem { id: r.id, url: r.url.clone(), content_type: r.content_type().to_string(), bytes }
}

/// Text-like content types that compress well.
pub fn is_text_type(ct: &str) -> bool {
    let c = ct.to_ascii_lowercase();
    c.starts_with("text/") || ["json", "javascript", "ecmascript", "xml", "svg", "css", "html", "csv", "x-www-form-urlencoded", "graphql"].iter().any(|k| c.contains(k))
}

/// Text responses larger than `min_bytes` sent without a Content-Encoding (or with "identity"); largest first.
pub fn compression_issues(rows: &[&SessionRecord], min_bytes: usize) -> Vec<SizeItem> {
    let mut v: Vec<SizeItem> = rows
        .iter()
        .filter(|r| !is_tunnel(r) && is_text_type(r.content_type()))
        .filter(|r| r.response_header("content-encoding").is_none_or(|e| e.trim().is_empty() || e.trim().eq_ignore_ascii_case("identity")))
        .map(|r| size_item(r, decoded_size(r)))
        .filter(|i| i.bytes > min_bytes)
        .collect();
    v.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.id.cmp(&b.id)));
    v
}

/// The `n` largest responses (transferred bytes).
pub fn largest_payloads(rows: &[&SessionRecord], n: usize) -> Vec<SizeItem> {
    let mut v: Vec<SizeItem> = rows.iter().filter(|r| !is_tunnel(r)).map(|r| size_item(r, response_size(r))).collect();
    v.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.id.cmp(&b.id)));
    v.truncate(n);
    v
}

pub const LARGE_IMAGE_BYTES: usize = 200 * 1024;

pub fn is_image(r: &SessionRecord) -> bool {
    if r.content_type().to_ascii_lowercase().starts_with("image/") {
        return true;
    }
    let p = r.path().split(['?', '#']).next().unwrap_or("").to_ascii_lowercase();
    [".png", ".jpg", ".jpeg", ".gif", ".webp", ".avif", ".svg", ".ico", ".bmp"].iter().any(|e| p.ends_with(e))
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImageReport {
    pub count: usize,
    pub total_bytes: u64,
    /// Images over `LARGE_IMAGE_BYTES`.
    pub large: usize,
    /// Every image, largest first.
    pub items: Vec<SizeItem>,
    /// Bytes per image type (e.g. "image/png"), largest first.
    pub by_type: Vec<(String, u64)>,
}

pub fn image_report(rows: &[&SessionRecord]) -> ImageReport {
    let mut rep = ImageReport::default();
    let mut types: HashMap<String, u64> = HashMap::new();
    for r in rows.iter().filter(|r| !is_tunnel(r) && is_image(r)) {
        let b = response_size(r);
        rep.count += 1;
        rep.total_bytes += b as u64;
        if b > LARGE_IMAGE_BYTES {
            rep.large += 1;
        }
        let ct = r.content_type();
        *types.entry(if ct.is_empty() { "(unknown)".into() } else { ct.to_ascii_lowercase() }).or_insert(0) += b as u64;
        rep.items.push(size_item(r, b));
    }
    rep.items.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.id.cmp(&b.id)));
    let mut t: Vec<(String, u64)> = types.into_iter().collect();
    t.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    rep.by_type = t;
    rep
}

// ---------------------------------------------------------------------------------------------
// Response-header hygiene
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct HygieneNote {
    pub header: &'static str,
    /// Responses of this host the check applied to, and how many of them lacked the header.
    pub applicable: usize,
    pub missing_in: usize,
    pub explanation: &'static str,
    /// One of the responses without it.
    pub example_id: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HostHygiene {
    pub host: String,
    pub responses: usize,
    pub notes: Vec<HygieneNote>,
}

/// (header, applies to: 0 = every response, 1 = HTTPS responses, 2 = HTML documents, explanation)
pub const HYGIENE_CHECKS: [(&str, u8, &str); 7] = [
    ("Strict-Transport-Security", 1, "Tells browsers to use HTTPS only for this host from now on, so a later visit cannot be downgraded to plain HTTP."),
    ("Content-Security-Policy", 2, "Lists which sources scripts, styles, frames etc. may come from; limits the damage of injected markup (XSS)."),
    ("X-Content-Type-Options", 0, "\"nosniff\" stops browsers from guessing a different content type than the one declared."),
    ("Referrer-Policy", 2, "Controls how much of the page URL is sent to other sites in the Referer header."),
    ("X-Frame-Options", 2, "Controls whether other sites may show this page in a frame (clickjacking protection); CSP frame-ancestors does the same."),
    ("Permissions-Policy", 2, "Turns browser features (camera, geolocation, ...) on or off for the page and its frames."),
    ("Cache-Control", 0, "States how long the response may be cached and by whom; without it, browsers and proxies guess."),
];

fn has_header_for_check(r: &SessionRecord, header: &str) -> bool {
    if r.response_header(header).is_some() {
        return true;
    }
    header.eq_ignore_ascii_case("X-Frame-Options")
        && r.response_header("content-security-policy").is_some_and(|c| c.to_ascii_lowercase().contains("frame-ancestors"))
}

/// Per host: security / caching response headers missing from successful responses (2xx and 3xx).
/// Informational notes for checking your own sites; hosts with nothing missing are left out.
pub fn header_hygiene(rows: &[&SessionRecord]) -> Vec<HostHygiene> {
    let mut by_host: BTreeMap<String, Vec<&SessionRecord>> = BTreeMap::new();
    for r in rows.iter().filter(|r| !is_tunnel(r) && (200..400).contains(&r.status)) {
        by_host.entry(host_name(r)).or_default().push(r);
    }
    let mut out = Vec::new();
    for (host, list) in by_host {
        let mut notes = Vec::new();
        for (header, scope, why) in HYGIENE_CHECKS {
            let applicable: Vec<&SessionRecord> = list
                .iter()
                .copied()
                .filter(|r| match scope {
                    1 => r.scheme().eq_ignore_ascii_case("https"),
                    2 => is_document(r),
                    _ => true,
                })
                .collect();
            let missing: Vec<&SessionRecord> = applicable.iter().copied().filter(|r| !has_header_for_check(r, header)).collect();
            if let Some(first) = missing.first() {
                notes.push(HygieneNote { header, applicable: applicable.len(), missing_in: missing.len(), explanation: why, example_id: first.id });
            }
        }
        if !notes.is_empty() {
            out.push(HostHygiene { host, responses: list.len(), notes });
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Cookies and third parties
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Default)]
pub struct CookieInfo {
    pub name: String,
    /// name + value bytes.
    pub size: usize,
    pub domain: String,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: Option<String>,
    /// "Max-Age=N s" or the Expires date; None = session cookie (deleted when the browser closes).
    pub expiry: Option<String>,
    /// Session that set it.
    pub session_id: usize,
}

/// Parses one Set-Cookie value; `host` is the default domain.
pub fn parse_set_cookie(value: &str, host: &str, session_id: usize) -> Option<CookieInfo> {
    let mut parts = value.split(';');
    let (name, val) = parts.next()?.split_once('=')?;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let mut c = CookieInfo { name: name.to_string(), size: name.len() + val.trim().len(), domain: host.to_ascii_lowercase(), path: "/".into(), session_id, ..Default::default() };
    let (mut expires, mut max_age) = (None, None);
    for a in parts {
        let (k, v) = match a.split_once('=') {
            Some((k, v)) => (k.trim().to_ascii_lowercase(), v.trim().to_string()),
            None => (a.trim().to_ascii_lowercase(), String::new()),
        };
        match k.as_str() {
            "domain" if !v.is_empty() => c.domain = v.trim_start_matches('.').to_ascii_lowercase(),
            "path" if !v.is_empty() => c.path = v,
            "secure" => c.secure = true,
            "httponly" => c.http_only = true,
            "samesite" => c.same_site = Some(v),
            "expires" => expires = Some(v),
            "max-age" => max_age = Some(format!("Max-Age={v} s")),
            _ => {}
        }
    }
    c.expiry = max_age.or(expires);
    Some(c)
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct CookieDomain {
    pub domain: String,
    /// Cookies set by responses (latest per name + path).
    pub set: Vec<CookieInfo>,
    /// Sum of the sizes of the cookies set.
    pub set_bytes: usize,
    /// Distinct cookie names the client sent to this host (request Cookie header).
    pub sent_names: Vec<String>,
    /// Largest Cookie request header sent to this host, in bytes.
    pub max_sent_header: usize,
}

/// Cookies per domain: those set by responses (Set-Cookie, one per line if several were kept) and those sent.
pub fn cookie_overview(rows: &[&SessionRecord]) -> Vec<CookieDomain> {
    let mut m: BTreeMap<String, CookieDomain> = BTreeMap::new();
    let mut sent: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for r in rows.iter().filter(|r| !is_tunnel(r)) {
        let host = host_name(r);
        for (k, v) in &r.response_headers {
            if !k.eq_ignore_ascii_case("set-cookie") {
                continue;
            }
            for line in v.lines() {
                if let Some(c) = parse_set_cookie(line, &host, r.id) {
                    let d = m.entry(c.domain.clone()).or_insert_with(|| CookieDomain { domain: c.domain.clone(), ..Default::default() });
                    d.set.retain(|x| !(x.name == c.name && x.path == c.path));
                    d.set.push(c);
                }
            }
        }
        if let Some(h) = r.request_header("cookie") {
            let d = m.entry(host.clone()).or_insert_with(|| CookieDomain { domain: host.clone(), ..Default::default() });
            d.max_sent_header = d.max_sent_header.max(h.len());
            let names = sent.entry(host.clone()).or_default();
            for p in h.split(';') {
                if let Some((n, _)) = p.split_once('=') {
                    if !n.trim().is_empty() {
                        names.insert(n.trim().to_string());
                    }
                }
            }
        }
    }
    for (host, names) in sent {
        if let Some(d) = m.get_mut(&host) {
            d.sent_names = names.into_iter().collect();
        }
    }
    m.into_values()
        .map(|mut d| {
            d.set.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.path.cmp(&b.path)));
            d.set_bytes = d.set.iter().map(|c| c.size).sum();
            d
        })
        .collect()
}

const TWO_PART_SUFFIXES: [&str; 14] =
    ["co.uk", "org.uk", "ac.uk", "gov.uk", "com.au", "net.au", "org.au", "co.jp", "co.in", "co.nz", "com.br", "com.cn", "co.za", "com.mx"];

/// The registrable part of a host ("cdn.example.co.uk" -> "example.co.uk"); IP addresses are kept whole.
pub fn site_of(host: &str) -> String {
    let h = strip_port(host).trim_end_matches('.').to_ascii_lowercase();
    if h.starts_with('[') || h.parse::<std::net::IpAddr>().is_ok() {
        return h;
    }
    let labels: Vec<&str> = h.split('.').collect();
    if labels.len() <= 2 {
        return h;
    }
    let last2 = labels[labels.len() - 2..].join(".");
    let keep = if TWO_PART_SUFFIXES.contains(&last2.as_str()) { 3 } else { 2 };
    labels[labels.len().saturating_sub(keep)..].join(".")
}

/// The page's main site: the site of the first HTML document, else the most requested site.
pub fn main_site(rows: &[&SessionRecord]) -> Option<String> {
    let mut docs: Vec<&SessionRecord> = rows.iter().copied().filter(|r| !is_tunnel(r) && is_document(r)).collect();
    docs.sort_by_key(|r| (start_epoch_ms(r).unwrap_or(i64::MAX), r.id));
    if let Some(d) = docs.first() {
        return Some(site_of(&host_name(d)));
    }
    let mut counts: HashMap<String, usize> = HashMap::new();
    for r in rows.iter().filter(|r| !is_tunnel(r)) {
        *counts.entry(site_of(&host_name(r))).or_insert(0) += 1;
    }
    count_sorted(counts).into_iter().next().map(|x| x.0)
}

#[derive(Debug, Clone, PartialEq)]
pub struct ThirdParty {
    pub host: String,
    pub site: String,
    pub count: usize,
    pub bytes: u64,
    pub first_id: usize,
}

/// Hosts that do not belong to the `main` site, most requested first.
pub fn third_party(rows: &[&SessionRecord], main: &str) -> Vec<ThirdParty> {
    let main = site_of(main.trim());
    let mut m: BTreeMap<String, ThirdParty> = BTreeMap::new();
    for r in rows.iter().filter(|r| !is_tunnel(r)) {
        let host = host_name(r);
        let site = site_of(&host);
        if site == main {
            continue;
        }
        let e = m.entry(host.clone()).or_insert_with(|| ThirdParty { host, site, count: 0, bytes: 0, first_id: r.id });
        e.count += 1;
        e.bytes += response_size(r) as u64;
    }
    let mut v: Vec<ThirdParty> = m.into_values().collect();
    v.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.host.cmp(&b.host)));
    v
}
