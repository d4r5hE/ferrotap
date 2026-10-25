//! Proxy core as a library module: the same Hudsucker recorder as before, plus an optional
//! channel sink that streams every captured session (as a `SessionEvent`) to the UI event loop.
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use chrono::Utc;
use http_body_util::{BodyExt, Full};
use hudsucker::{
    certificate_authority::RcgenAuthority,
    hyper::{body::Bytes, Request, Response, Version},
    hyper_util::{
        client::legacy::Builder as ClientBuilder, rt::TokioExecutor,
        server::conn::auto::Builder as ServerBuilder,
    },
    rcgen::{
        BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, Issuer, KeyPair,
        KeyUsagePurpose,
    },
    rustls::crypto::aws_lc_rs,
    Body, HttpContext, HttpHandler, Proxy, RequestOrResponse,
};

use crate::bypass;
use crate::model::{SessionEvent, SessionRecord};
use crate::ws;

pub const PORT: u16 = 8897;
/// Most headers one request or response may have (hyper's default is 100).
pub const MAX_HEADERS: usize = 1000;

/// Optional debugging dump of every session (for tests/compare_sessions.py): set
/// `FERROTAP_SESSIONS_DUMP` to a file path, or to `1` for `sessions.json` in the project folder.
/// Off by default: it keeps a second copy of every session in memory and rewrites the whole
/// file while traffic flows (and it holds captured traffic).
pub fn sessions_dump_path() -> Option<PathBuf> {
    let v = std::env::var("FERROTAP_SESSIONS_DUMP").ok()?;
    let v = v.trim();
    match v {
        "" | "0" => None,
        "1" => Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("sessions.json")),
        p => Some(PathBuf::from(p)),
    }
}

#[derive(Clone)]
struct Store {
    next_id: Arc<std::sync::atomic::AtomicUsize>,
    /// The debugging dump: every session and its file (see `sessions_dump_path`).
    dump: Option<(Arc<Mutex<Vec<SessionRecord>>>, PathBuf)>,
    /// Live feed to the UI (None when running headless).
    tx: Option<Sender<SessionEvent>>,
    dirty: Arc<AtomicBool>,
}

impl Store {
    fn new(tx: Option<Sender<SessionEvent>>) -> Self {
        Store {
            next_id: Arc::new(std::sync::atomic::AtomicUsize::new(1)),
            dump: sessions_dump_path().map(|p| (Arc::new(Mutex::new(Vec::new())), p)),
            tx,
            dirty: Arc::new(AtomicBool::new(false)),
        }
    }

    fn flush(&self) {
        let Some((list, file)) = &self.dump else { return };
        if self.dirty.swap(false, Ordering::AcqRel) {
            let j = {
                let l = list.lock().unwrap();
                serde_json::to_string(&*l)
            };
            if let Ok(j) = j {
                let _ = std::fs::write(file, j);
            }
        }
    }

    /// Replace a recorded session (a streamed one that grew or finished).
    fn update(&self, s: SessionRecord) {
        if let Some((list, _)) = &self.dump {
            if let Some(r) = list.lock().unwrap().iter_mut().rev().find(|r| r.id == s.id) {
                *r = s.clone();
            }
            self.dirty.store(true, Ordering::Release);
        }
        if let Some(tx) = &self.tx {
            let _ = tx.send(SessionEvent::Update(s));
        }
    }

    /// Record a session and return its id.
    fn push_id(&self, s: SessionRecord) -> usize {
        self.push(s)
    }

    /// Record a session; returns the id it was given.
    fn push(&self, mut s: SessionRecord) -> usize {
        // the dump's lock (when on) keeps ids in the order sessions are listed
        let guard = self.dump.as_ref().map(|(l, _)| l.lock().unwrap());
        s.id = self.next_id.fetch_add(1, Ordering::AcqRel);
        let id = s.id;
        if let Some(tx) = &self.tx {
            // A closed receiver just means the UI went away.
            let _ = tx.send(SessionEvent::New(s.clone()));
        }
        if let Some(mut l) = guard {
            l.push(s);
            self.dirty.store(true, Ordering::Release);
        }
        id
    }
}

struct Pending {
    http_version: String,
    method: String,
    url: String,
    headers: BTreeMap<String, String>,
    body: Bytes,
    start_time: String,
    sent: Instant,
    started: Instant,
    /// Rules: names of request-stage rules applied, response edits they asked for, client process.
    rules_applied: Vec<String>,
    resp_headers: Vec<crate::rules_ext::HeaderEdit>,
    resp_body: Vec<crate::rules_ext::TextReplace>,
    process: Option<String>,
    /// G12: response status override, bandwidth limit (kbit/s) and drop chance (%).
    resp_status: Option<u16>,
    kbps: u32,
    loss_pct: u8,
}

#[derive(Clone)]
struct Recorder {
    store: Store,
    pending: Option<Arc<Pending>>,
    /// The local process (pid, image name) and address of the current client.
    client: Option<(u32, String)>,
    client_ip: Option<String>,
}

impl Recorder {
    /// Resolve who is connected from `addr` (cached by procinfo).
    async fn identify(&mut self, addr: SocketAddr) {
        self.client_ip = Some(addr.ip().to_canonical().to_string());
        self.client = tokio::task::spawn_blocking(move || crate::procinfo::lookup(&addr)).await.ok().flatten();
    }

    /// Record a session: fill in the client process / address, apply the capture filter.
    fn push(&self, mut s: SessionRecord) {
        if s.process_name.is_none() {
            if let Some((pid, name)) = &self.client {
                s.process_name = Some(name.clone());
                s.pid = Some(*pid);
            }
        }
        if s.client_ip.is_none() {
            s.client_ip = self.client_ip.clone();
        }
        if crate::conn::current().should_capture(&s) {
            self.store.push(s);
        }
    }

    /// Like `push`, returning the id (None when the capture filter drops it).
    fn push_streaming(&self, mut s: SessionRecord) -> Option<usize> {
        if s.process_name.is_none() {
            if let Some((pid, name)) = &self.client {
                s.process_name = Some(name.clone());
                s.pid = Some(*pid);
            }
        }
        if s.client_ip.is_none() {
            s.client_ip = self.client_ip.clone();
        }
        crate::conn::current().should_capture(&s).then(|| self.store.push_id(s))
    }
}

/// A response body fed from a channel (the streaming tee).
struct ChannelBody {
    rx: tokio::sync::mpsc::Receiver<Result<hudsucker::hyper::body::Frame<Bytes>, hudsucker::Error>>,
}

impl hudsucker::hyper::body::Body for ChannelBody {
    type Data = Bytes;
    type Error = hudsucker::Error;
    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<hudsucker::hyper::body::Frame<Bytes>, Self::Error>>> {
        self.rx.poll_recv(cx)
    }
}

/// Responses announcing more than this are passed straight through instead of buffered.
pub const LARGE_BODY: u64 = 16 * 1024 * 1024;
/// How much of a large response body the session keeps.
pub const LARGE_BODY_KEEP: usize = 8 * 1024 * 1024;

/// Content-Length of a response, if it is over `LARGE_BODY`.
fn large_body(h: &hudsucker::hyper::HeaderMap) -> Option<u64> {
    let n: u64 = h.get("content-length")?.to_str().ok()?.trim().parse().ok()?;
    (n > LARGE_BODY).then_some(n)
}

/// Should this response be streamed to the client instead of buffered?
fn wants_streaming(h: &hudsucker::hyper::HeaderMap, p: &Pending, rules: &[crate::model::Rule]) -> bool {
    let ct = h.get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_ascii_lowercase();
    let streamy = ct.starts_with("text/event-stream") || ct.starts_with("application/grpc") || ct.contains("x-ndjson");
    if !(streamy || crate::conn::current().streaming || large_body(h).is_some()) {
        return false;
    }
    !body_needed(p, rules, &bp_headers(h))
}

/// Does anything (rules, scripts, throttling, breakpoints) need the whole response body?
fn body_needed(p: &Pending, rules: &[crate::model::Rule], resp_headers: &[(String, String)]) -> bool {
    rules.iter().any(crate::rules_ext::is_response_rule)
        || !p.resp_body.is_empty()
        || p.resp_status.is_some()
        || p.kbps > 0
        || p.loss_pct > 0
        || crate::scripting::has_hook(|x| x.on_response)
        // A body-inspecting response breakpoint (`~b`) can only be judged once the body is here.
        || crate::model::bp_response_wants_body()
        || crate::model::breakpoint_match(crate::model::BpStage::Response, &p.method, &p.url, 0, resp_headers, "").is_some()
        || crate::model::breakpoint_match(crate::model::BpStage::Response, &p.method, &p.url, 200, resp_headers, "").is_some()
}

/// Frames already read from a response, then the rest of it.
struct PrefixBody {
    prefix: std::collections::VecDeque<hudsucker::hyper::body::Frame<Bytes>>,
    err: Option<hudsucker::Error>,
    rest: Option<std::pin::Pin<Box<Body>>>,
}

impl hudsucker::hyper::body::Body for PrefixBody {
    type Data = Bytes;
    type Error = hudsucker::Error;
    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<hudsucker::hyper::body::Frame<Bytes>, Self::Error>>> {
        if let Some(f) = self.prefix.pop_front() {
            return std::task::Poll::Ready(Some(Ok(f)));
        }
        if let Some(e) = self.err.take() {
            self.rest = None;
            return std::task::Poll::Ready(Some(Err(e)));
        }
        match self.rest.as_mut() {
            Some(b) => b.as_mut().poll_frame(cx),
            None => std::task::Poll::Ready(None),
        }
    }
}

/// Read a response of unknown length until it ends or passes `LARGE_BODY`. Returns the body
/// to use from here on (same bytes) and whether it was too large to buffer.
async fn read_ahead(mut body: Body) -> (Body, bool) {
    let mut prefix = std::collections::VecDeque::new();
    let mut n = 0usize;
    let mut err = None;
    let mut ended = false;
    loop {
        match body.frame().await {
            Some(Ok(f)) => {
                n += f.data_ref().map_or(0, |d| d.len());
                prefix.push_back(f);
                if n as u64 > LARGE_BODY {
                    break;
                }
            }
            Some(Err(e)) => {
                err = Some(e);
                ended = true;
                break;
            }
            None => {
                ended = true;
                break;
            }
        }
    }
    let big = !ended;
    let rest = (!ended).then(|| Box::pin(body));
    (Body::from(http_body_util::combinators::BoxBody::new(PrefixBody { prefix, err, rest })), big)
}

fn ver(v: Version) -> &'static str {
    if v == Version::HTTP_09 {
        "HTTP/0.9"
    } else if v == Version::HTTP_10 {
        "HTTP/1.0"
    } else if v == Version::HTTP_11 {
        "HTTP/1.1"
    } else if v == Version::HTTP_2 {
        "HTTP/2.0"
    } else if v == Version::HTTP_3 {
        "HTTP/3.0"
    } else {
        "unknown"
    }
}

/// Build scheme://authority/path, dropping the default port (:80 for http, :443 for https).
fn clean_url(scheme: &str, authority: &str, pq: &str) -> String {
    let def = match scheme {
        "http" => ":80",
        "https" => ":443",
        _ => "",
    };
    let a = if !def.is_empty() && authority.ends_with(def) {
        &authority[..authority.len() - def.len()]
    } else {
        authority
    };
    format!("{scheme}://{a}{pq}")
}

fn hdrs(h: &hudsucker::hyper::HeaderMap) -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    for (k, v) in h.iter() {
        let v = String::from_utf8_lossy(v.as_bytes());
        match out.get_mut(k.as_str()) {
            // Several Set-Cookie lines cannot be comma-joined safely (Expires has commas): use newlines.
            Some(prev) if k == hudsucker::hyper::header::SET_COOKIE => {
                prev.push('\n');
                prev.push_str(&v);
            }
            Some(prev) => {
                prev.push_str(", ");
                prev.push_str(&v);
            }
            None => {
                out.insert(k.to_string(), v.into_owned());
            }
        }
    }
    out
}

const BP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

fn bp_headers(h: &hudsucker::hyper::HeaderMap) -> Vec<(String, String)> {
    h.iter().map(|(k, v)| (k.to_string(), String::from_utf8_lossy(v.as_bytes()).to_string())).collect()
}

fn bp_set_headers(h: &mut hudsucker::hyper::HeaderMap, list: &[(String, String)]) {
    use hudsucker::hyper::header::{HeaderName, HeaderValue};
    h.clear();
    for (k, v) in list {
        if let (Ok(n), Ok(val)) = (HeaderName::from_bytes(k.trim().as_bytes()), HeaderValue::from_str(v.trim())) {
            h.append(n, val);
        }
    }
}

fn bp_fix_len(h: &mut hudsucker::hyper::HeaderMap, len: usize) {
    h.remove("transfer-encoding");
    h.insert("content-length", hudsucker::hyper::header::HeaderValue::from(len));
}

/// Response the client sees when the user aborts a paused exchange.
fn bp_abort_response() -> Response<Body> {
    let msg = "Aborted at breakpoint by Ferrotap";
    Response::builder()
        .status(502)
        .header("content-type", "text/plain")
        .header("content-length", msg.len().to_string())
        .header("x-ferrotap-breakpoint", "aborted")
        .body(Body::from(Full::new(Bytes::from(msg))))
        .unwrap_or_else(|_| Response::new(Body::from(Full::new(Bytes::new()))))
}

/// An error and all its sources, e.g. "client error (Connect): tcp connect error: connection refused".
pub fn error_chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut parts = vec![e.to_string()];
    let mut cur = e.source();
    while let Some(s) = cur {
        let t = s.to_string();
        if !parts.iter().any(|p| p.contains(&t)) {
            parts.push(t);
        }
        cur = s.source();
    }
    parts.join(": ")
}

fn trailer_list(h: &hudsucker::hyper::HeaderMap) -> Vec<(String, String)> {
    h.iter().map(|(k, v)| (k.to_string(), String::from_utf8_lossy(v.as_bytes()).into_owned())).collect()
}

/// 502 sent to the client when the upstream request failed.
fn upstream_error_response(reason: &str) -> Response<Body> {
    let msg = format!("Ferrotap could not reach the server: {reason}");
    Response::builder()
        .status(502)
        .header("content-type", "text/plain; charset=utf-8")
        .header("content-length", msg.len().to_string())
        .header("x-ferrotap-error", reason.replace(['\r', '\n'], " "))
        .body(Body::from(Full::new(Bytes::from(msg))))
        .unwrap_or_else(|_| Response::new(Body::from(Full::new(Bytes::new()))))
}

/// Body that fails at once: hyper then drops the client connection ("close connection" rule action).
struct AbortBody;

impl hudsucker::hyper::body::Body for AbortBody {
    type Data = Bytes;
    type Error = hudsucker::Error;
    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<hudsucker::hyper::body::Frame<Bytes>, Self::Error>>> {
        std::task::Poll::Ready(Some(Err(hudsucker::Error::Io(std::io::Error::new(
            std::io::ErrorKind::ConnectionAborted,
            "connection closed by Ferrotap rule",
        )))))
    }
}

/// Response whose body errors immediately, so the client connection is closed without a full answer.
fn rule_close_response() -> Response<Body> {
    Response::builder()
        .status(502)
        .header("content-length", "1")
        .body(Body::from(http_body_util::combinators::BoxBody::new(AbortBody)))
        .unwrap_or_else(|_| Response::new(Body::from(Full::new(Bytes::new()))))
}

/// Build the client response for a local rule answer.
fn rule_local_response(rr: &crate::model::RuleResponse, names: &str) -> (Response<Body>, BTreeMap<String, String>) {
    let mut b = Response::builder().status(rr.status);
    let mut rh: BTreeMap<String, String> = BTreeMap::new();
    for (k, v) in &rr.headers {
        if k.eq_ignore_ascii_case("content-length") {
            continue;
        }
        b = b.header(k.as_str(), v.as_str());
        rh.insert(k.to_ascii_lowercase(), v.clone());
    }
    rh.insert("x-ferrotap-autoresponder".to_string(), names.to_string());
    b = b.header("x-ferrotap-autoresponder", names);
    b = b.header("content-length", rr.body.len().to_string());
    let resp = b
        .body(Body::from(Full::new(Bytes::from(rr.body.clone()))))
        .unwrap_or_else(|_| Response::new(Body::from(Full::new(Bytes::new()))));
    (resp, rh)
}

impl HttpHandler for Recorder {
    /// Called for CONNECT: returning false makes Hudsucker pass bytes through as a raw TLS tunnel (no MITM).
    async fn should_intercept_connect(&mut self, ctx: &HttpContext, req: &Request<Body>) -> bool {
        let auth = req.uri().authority().map(|a| a.as_str().to_string()).unwrap_or_default();
        let list = bypass::global();
        if list.matches(&auth) {
            list.note_tunnel();
            self.identify(ctx.client_addr).await;
            self.push(SessionRecord {
                id: 0,
                method: "CONNECT".into(),
                url: format!("tunnel://{auth}"),
                status: 200,
                http_version: "HTTP/1.1".into(),
                response_http_version: "HTTP/1.1".into(),
                request_headers: BTreeMap::new(),
                response_headers: BTreeMap::from([("x-ferrotap".to_string(), "do-not-decrypt: raw TLS tunnel".to_string())]),
                request_body: String::new(),
                response_body: String::new(),
                start_time: Utc::now().to_rfc3339(),
                duration_ms: 0, ..Default::default()
            });
            return false;
        }
        true
    }
    async fn handle_request(&mut self, ctx: &HttpContext, req: Request<Body>) -> RequestOrResponse {
        if req.method() == hudsucker::hyper::Method::CONNECT {
            self.pending = None;
            return req.into();
        }
        let started = Instant::now();
        let start_time = Utc::now().to_rfc3339();
        self.identify(ctx.client_addr).await;
        let (mut parts, body) = req.into_parts();
        let mut bytes: Bytes = body.collect().await.map(|c| c.to_bytes()).unwrap_or_default();
        let scheme = parts.uri.scheme_str().unwrap_or("https").to_string();
        let authority = parts
            .uri
            .authority()
            .map(|a| a.as_str().to_string())
            .or_else(|| {
                parts
                    .headers
                    .get("host")
                    .and_then(|h| h.to_str().ok())
                    .map(|h| h.to_string())
            })
            .unwrap_or_else(|| "unknown".to_string());
        let pq = parts.uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
        let mut url = clean_url(&scheme, &authority, pq);
        let http_version = ver(parts.version).to_string();
        // http://ferrotap.ca/ through the proxy: device setup page + root CA download (answered here).
        if crate::conn::is_setup_host(&authority) {
            self.pending = None;
            let pem = std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ca").join("ferrotap-ca.pem")).ok();
            let (status, ct, body) = crate::conn::setup_page(pq, pem.as_deref());
            let mut rec = SessionRecord {
                id: 0,
                method: parts.method.to_string(),
                url: url.clone(),
                status,
                http_version: http_version.clone(),
                response_http_version: "HTTP/1.1".into(),
                request_headers: hdrs(&parts.headers),
                response_headers: BTreeMap::from([("content-type".to_string(), ct.to_string())]),
                start_time: start_time.clone(),
                duration_ms: started.elapsed().as_millis(),
                client_ip: Some(ctx.client_addr.ip().to_string()),
                comment: "Ferrotap device setup page".into(),
                ..Default::default()
            };
            rec.set_response_body(&body);
            self.push(rec);
            let resp = Response::builder()
                .status(status)
                .header("content-type", ct)
                .header("content-length", body.len().to_string())
                .header("cache-control", "no-store")
                .body(Body::from(Full::new(Bytes::from(body))))
                .unwrap_or_else(|_| Response::new(Body::from(Full::new(Bytes::new()))));
            return resp.into();
        }
        // Rules (Auto Responder / Map Local / Map Remote / rewrite / block / delay).
        let rules = crate::model::global_rules();
        let mut rules_applied: Vec<String> = Vec::new();
        let mut resp_edits_h: Vec<crate::rules_ext::HeaderEdit> = Vec::new();
        let mut resp_edits_b: Vec<crate::rules_ext::TextReplace> = Vec::new();
        let process: Option<String> = self.client.as_ref().map(|c| c.1.clone());
        let (mut resp_status, mut kbps, mut loss_pct) = (None, 0u32, 0u8);
        if !rules.is_empty() {
            let hl = bp_headers(&parts.headers);
            let plan = crate::rules_ext::plan_request(
                &rules,
                &crate::rules_ext::ReqCtx { method: parts.method.as_str(), url: &url, headers: &hl, body: &bytes, process: process.as_deref() },
            );
            if !plan.applied.is_empty() {
                let names = plan.applied.join(", ");
                if plan.delay_ms > 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(plan.delay_ms)).await;
                }
                if plan.close {
                    self.pending = None;
                    self.push(SessionRecord {
                        id: 0,
                        method: parts.method.to_string(),
                        url: url.clone(),
                        status: 0,
                        http_version: http_version.clone(),
                        request_headers: hdrs(&parts.headers),
                        request_body: crate::model::body_text(&bytes),
                        request_body_b64: crate::model::body_b64(&bytes),
                        start_time: start_time.clone(),
                        duration_ms: started.elapsed().as_millis(),
                        mark: Some(crate::model::SessionMark::Purple),
                        comment: format!("Rule closed connection: {names}"),
                        ..Default::default()
                    });
                    return rule_close_response().into();
                }
                if let Some(rr) = plan.response {
                    // Answer locally, never contact upstream.
                    self.pending = None;
                    let (resp, rh) = rule_local_response(&rr, &names);
                    self.push(SessionRecord {
                        id: 0,
                        method: parts.method.to_string(),
                        url: url.clone(),
                        status: rr.status,
                        http_version: http_version.clone(),
                        response_http_version: "HTTP/1.1".into(),
                        request_headers: hdrs(&parts.headers),
                        response_headers: rh,
                        request_body: crate::model::body_text(&bytes),
                        request_body_b64: crate::model::body_b64(&bytes),
                        response_body: crate::model::body_text(&rr.body),
                        response_body_b64: crate::model::body_b64(&rr.body),
                        response_raw_size: Some(rr.body.len()),
                        response_decoded_size: Some(rr.body.len()),
                        response_trailers: Vec::new(),
                        start_time: start_time.clone(),
                        duration_ms: started.elapsed().as_millis(),
                        mark: Some(crate::model::SessionMark::Purple),
                        comment: format!("AutoResponder: {names}"),
                        ..Default::default()
                    });
                    return resp.into();
                }
                rules_applied = plan.applied.clone();
                // Pass-through rules: rewrite the request, remember the response edits.
                if let Some(nu) = &plan.new_url {
                    if let Ok(uri) = nu.parse::<hudsucker::hyper::Uri>() {
                        if let Some(a) = uri.authority() {
                            if let Ok(hv) = hudsucker::hyper::header::HeaderValue::from_str(a.as_str()) {
                                parts.headers.insert("host", hv);
                            }
                        }
                        // The new upstream may not speak HTTP/2; let the client negotiate.
                        if parts.version == Version::HTTP_2 {
                            parts.version = Version::HTTP_11;
                        }
                        parts.uri = uri;
                        rules_applied.push(format!("Map Remote from {url}"));
                        url = nu.clone();
                    }
                }
                if let Some(q) = &plan.query_url {
                    // Query rewrite on the same host: only the path and query change.
                    if let Ok(uri) = q.parse::<hudsucker::hyper::Uri>() {
                        parts.uri = uri;
                        url = q.clone();
                    }
                }
                resp_status = plan.resp_status;
                kbps = plan.kbps;
                loss_pct = plan.loss_pct;
                if !plan.req_headers.is_empty() {
                    let mut hl2 = bp_headers(&parts.headers);
                    crate::rules_ext::apply_header_edits(&mut hl2, &plan.req_headers);
                    bp_set_headers(&mut parts.headers, &hl2);
                }
                if let Some(nb) = crate::rules_ext::apply_body_replace(&bytes, &plan.req_body) {
                    bytes = Bytes::from(nb);
                    bp_fix_len(&mut parts.headers, bytes.len());
                }
                resp_edits_h = plan.resp_headers;
                resp_edits_b = plan.resp_body;
            }
        }
        // Scripts (on_request), after the rules and before breakpoints.
        if crate::scripting::has_hook(|h| h.on_request) {
            let rp = crate::scripting::ReqParts { method: parts.method.to_string(), url: url.clone(), headers: bp_headers(&parts.headers), body: bytes.to_vec() };
            let before_url = url.clone();
            let before_body = bytes.clone();
            if let Ok((rp, names)) = tokio::task::spawn_blocking(move || {
                let mut rp = rp;
                let names = crate::scripting::run_request(&mut rp);
                (rp, names)
            })
            .await
            {
                if !names.is_empty() {
                    if let Ok(m) = hudsucker::hyper::Method::from_bytes(rp.method.trim().to_ascii_uppercase().as_bytes()) {
                        parts.method = m;
                    }
                    bp_set_headers(&mut parts.headers, &rp.headers);
                    if rp.url != before_url {
                        if let Ok(uri) = rp.url.parse::<hudsucker::hyper::Uri>() {
                            if let Some(a) = uri.authority() {
                                if let Ok(hv) = hudsucker::hyper::header::HeaderValue::from_str(a.as_str()) {
                                    parts.headers.insert("host", hv);
                                }
                            }
                            parts.uri = uri;
                            url = rp.url.clone();
                        }
                    }
                    if rp.body != before_body.as_ref() {
                        bytes = Bytes::from(rp.body);
                        bp_fix_len(&mut parts.headers, bytes.len());
                    }
                    rules_applied.push(format!("Script: {}", names.join(", ")));
                }
            }
        }
        // Breakpoints (request stage): park the request until the UI resumes/aborts it.
        if let Some(rule) = crate::model::breakpoint_match(crate::model::BpStage::Request, parts.method.as_str(), &url, 0, &bp_headers(&parts.headers), &crate::model::body_text(&bytes)) {
            let info = crate::model::PausedInfo {
                id: 0,
                stage: crate::model::BpStage::Request,
                rule: rule.clone(),
                method: parts.method.to_string(),
                url: url.clone(),
                status: 0,
                headers: bp_headers(&parts.headers),
                body: bytes.to_vec(),
                since_ms: 0,
            };
            match crate::model::breakpoint_hub().pause(info, BP_TIMEOUT).await {
                crate::model::PauseOutcome::Resumed(e) => {
                    if let Some(m) = e.method {
                        if let Ok(m) = hudsucker::hyper::Method::from_bytes(m.trim().to_ascii_uppercase().as_bytes()) {
                            parts.method = m;
                        }
                    }
                    if let Some(u) = e.url {
                        if let Ok(uri) = u.trim().parse::<hudsucker::hyper::Uri>() {
                            parts.uri = uri;
                            url = u.trim().to_string();
                        }
                    }
                    if let Some(h) = e.headers {
                        bp_set_headers(&mut parts.headers, &h);
                    }
                    if let Some(b) = e.body {
                        bytes = Bytes::from(b);
                        bp_fix_len(&mut parts.headers, bytes.len());
                    }
                }
                crate::model::PauseOutcome::Aborted => {
                    self.pending = None;
                    self.push(SessionRecord {
                        id: 0,
                        method: parts.method.to_string(),
                        url: url.clone(),
                        status: 502,
                        http_version: http_version.clone(),
                        response_http_version: "HTTP/1.1".into(),
                        request_headers: hdrs(&parts.headers),
                        request_body: crate::model::body_text(&bytes),
                        request_body_b64: crate::model::body_b64(&bytes),
                        response_body: "Aborted at breakpoint by Ferrotap".into(),
                        start_time: start_time.clone(),
                        duration_ms: started.elapsed().as_millis(),
                        comment: format!("Breakpoint aborted: {rule}"),
                        ..Default::default()
                    });
                    return bp_abort_response().into();
                }
                crate::model::PauseOutcome::TimedOut => {}
            }
        }
        if ws::is_upgrade_request(&parts.headers) {
            // WebSocket upgrade: Hudsucker answers the handshake itself (handle_response is not called),
            // so record the session now and link this connection's frames to it (x-ferrotap-ws-conn).
            self.pending = None;
            let conn = ws::register_upgrade(&ctx.client_addr.to_string());
            self.push(SessionRecord {
                id: 0,
                method: parts.method.to_string(),
                url: url.clone(),
                status: 101,
                http_version: http_version.clone(),
                response_http_version: "HTTP/1.1".into(),
                request_headers: hdrs(&parts.headers),
                response_headers: ws::handshake_response_headers(&parts.headers, &conn),
                start_time: start_time.clone(),
                duration_ms: started.elapsed().as_millis(),
                ..Default::default()
            });
            return Request::from_parts(parts, Body::from(Full::new(bytes))).into();
        }
        // G11 hook (llm_analysis.rs): when the LLM response cache is on, serve an identical LLM call from the cache.
        if let Some((resp, rec)) = crate::llm_analysis::proxy_cache_response(parts.method.as_str(), &url, &hdrs(&parts.headers), &bytes, &http_version, &start_time) {
            self.pending = None;
            self.push(rec);
            return resp.into();
        }
        self.pending = Some(Arc::new(Pending {
            http_version,
            method: parts.method.to_string(),
            url,
            headers: hdrs(&parts.headers),
            body: bytes.clone(),
            start_time,
            started,
            sent: Instant::now(),
            rules_applied,
            resp_headers: resp_edits_h,
            resp_body: resp_edits_b,
            process,
            resp_status,
            kbps,
            loss_pct,
        }));
        Request::from_parts(parts, Body::from(Full::new(bytes))).into()
    }

    /// Upstream could not be reached (DNS, refused, reset, TLS, timeout): record the failure so the
    /// error dashboard sees it, and answer the client with a 502 that says why.
    async fn handle_error(&mut self, _ctx: &HttpContext, err: hudsucker::hyper_util::client::legacy::Error) -> Response<Body> {
        let reason = error_chain(&err);
        if let Some(p) = self.pending.take() {
            self.push(SessionRecord {
                id: 0,
                method: p.method.clone(),
                url: p.url.clone(),
                status: 502,
                http_version: p.http_version.clone(),
                response_http_version: "HTTP/1.1".into(),
                request_headers: p.headers.clone(),
                response_headers: BTreeMap::from([("x-ferrotap-error".to_string(), reason.clone())]),
                request_body: crate::model::body_text(&p.body),
                request_body_b64: crate::model::body_b64(&p.body),
                response_body: format!("Ferrotap could not reach the server: {reason}"),
                start_time: p.start_time.clone(),
                duration_ms: p.started.elapsed().as_millis(),
                mark: Some(crate::model::SessionMark::Red),
                comment: format!("Upstream failed: {reason}"),
                ..Default::default()
            });
        }
        upstream_error_response(&reason)
    }

    async fn handle_response(&mut self, _ctx: &HttpContext, res: Response<Body>) -> Response<Body> {
        let Some(p) = self.pending.take() else {
            return res;
        };
        let (mut parts, body) = res.into_parts();
        // HTTP/3 offers would take the client off the proxy: remove them (Connections setting).
        let mut h3_removed = false;
        if crate::conn::current().strip_h3 {
            if let Some(v) = parts.headers.get("alt-svc").and_then(|v| v.to_str().ok()).map(str::to_string) {
                if crate::http3::offers_h3(&v) {
                    h3_removed = true;
                    match crate::http3::strip_h3(&v).and_then(|s| hudsucker::hyper::header::HeaderValue::from_str(&s).ok()) {
                        Some(nv) => {
                            parts.headers.insert("alt-svc", nv);
                        }
                        None => {
                            parts.headers.remove("alt-svc");
                        }
                    }
                }
            }
        }
        let ttfb_total = p.sent.elapsed().as_millis() as u64;
        let conn_info = crate::timing::take_conn(&p.url);
        let rules = crate::model::global_rules();
        let mut body = body;
        let mut overflow = false;
        if parts.headers.get("content-length").is_none() && !wants_streaming(&parts.headers, &p, &rules) && !body_needed(&p, &rules, &bp_headers(&parts.headers)) {
            (body, overflow) = read_ahead(body).await;
        }
        if overflow || wants_streaming(&parts.headers, &p, &rules) {
            let large = large_body(&parts.headers).or(overflow.then_some(0));
            let mut hl = bp_headers(&parts.headers);
            if !p.resp_headers.is_empty() {
                crate::rules_ext::apply_header_edits(&mut hl, &p.resp_headers);
                bp_set_headers(&mut parts.headers, &hl);
            }
            let mut rec = SessionRecord {
                id: 0,
                method: p.method.clone(),
                url: p.url.clone(),
                status: parts.status.as_u16(),
                http_version: p.http_version.clone(),
                response_http_version: ver(parts.version).to_string(),
                request_headers: p.headers.clone(),
                response_headers: hdrs(&parts.headers),
                request_body: crate::model::body_text(&p.body),
                request_body_b64: crate::model::body_b64(&p.body),
                start_time: p.start_time.clone(),
                duration_ms: p.started.elapsed().as_millis(),
                dns_ms: conn_info.dns_ms,
                connect_ms: conn_info.connect_ms,
                tls_ms: conn_info.tls_ms,
                ttfb_ms: Some(ttfb_total.saturating_sub(conn_info.dns_ms.unwrap_or(0) + conn_info.connect_ms.unwrap_or(0) + conn_info.tls_ms.unwrap_or(0))),
            send_ms: Some(p.sent.duration_since(p.started).as_millis() as u64),
                tls_version: conn_info.tls_version.clone(),
                comment: "streaming...".into(),
                ..Default::default()
            };
            let id = self.push_streaming(rec.clone());
            let store = self.store.clone();
            let mut applied = p.rules_applied.clone();
            if h3_removed {
                applied.push("HTTP/3 offer removed (Alt-Svc)".into());
            }
            let started = p.started;
            let (tx, rx) = tokio::sync::mpsc::channel(32);
            tokio::spawn(async move {
                let mut body = body;
                let mut acc: Vec<u8> = Vec::new();
                let mut total = 0usize;
                let mut trailers = Vec::new();
                let mut last = Instant::now();
                let mut client_gone = false;
                while let Some(fr) = body.frame().await {
                    match fr {
                        Ok(f) => {
                            if let Some(d) = f.data_ref() {
                                total += d.len();
                                if large.is_none() || acc.len() < LARGE_BODY_KEEP {
                                    let room = if large.is_some() { LARGE_BODY_KEEP - acc.len() } else { d.len() };
                                    acc.extend_from_slice(&d[..d.len().min(room)]);
                                }
                            }
                            if let Some(t) = f.trailers_ref() {
                                trailers = trailer_list(t);
                            }
                            if tx.send(Ok(f)).await.is_err() {
                                client_gone = true;
                                break;
                            }
                            if let (Some(id), None) = (id, large) {
                                if last.elapsed() >= std::time::Duration::from_millis(250) {
                                    rec.id = id;
                                    rec.set_response_body(&acc);
                                    rec.duration_ms = started.elapsed().as_millis();
                                    store.update(rec.clone());
                                    last = Instant::now();
                                }
                            }
                        }
                        Err(e) => {
                            let _ = tx.send(Err(e)).await;
                            break;
                        }
                    }
                }
                if let Some(id) = id {
                    rec.id = id;
                    rec.set_response_body(&acc);
                    rec.response_raw_size = Some(total);
                    rec.response_trailers = trailers;
                    rec.duration_ms = started.elapsed().as_millis();
                    let mut notes = vec![if client_gone { "streamed (client closed early)".to_string() } else { "streamed".to_string() }];
                    if large.is_some() && total > acc.len() {
                        notes.push(format!("large body: kept the first {} of {} bytes", acc.len(), total));
                    }
                    if !applied.is_empty() {
                        notes.push(applied.join(", "));
                    }
                    rec.comment = notes.join("; ");
                    store.update(rec);
                }
            });
            // The length may be unknown until the end: let hyper frame it. A large body passes
            // through unchanged, so its length stays (download progress keeps working).
            if large.is_none() {
                parts.headers.remove("content-length");
            }
            return Response::from_parts(parts, Body::from(http_body_util::combinators::BoxBody::new(ChannelBody { rx })));
        }
        let collected = body.collect().await.ok();
        let trailers = collected.as_ref().and_then(|c| c.trailers().cloned());
        let mut bytes: Bytes = collected.map(|c| c.to_bytes()).unwrap_or_default();
        // Rules on the response: request-stage edits plus response-stage rules (can match status).
        let mut applied = p.rules_applied.clone();
        if h3_removed {
            applied.push("HTTP/3 offer removed (Alt-Svc)".into());
        }
        let rules = crate::model::global_rules();
        let mut h_edits = p.resp_headers.clone();
        let mut b_edits = p.resp_body.clone();
        let (mut status_override, mut kbps, mut loss) = (p.resp_status, p.kbps, p.loss_pct);
        if rules.iter().any(crate::rules_ext::is_response_rule) {
            let req_h: Vec<(String, String)> = p.headers.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            let resp_h = bp_headers(&parts.headers);
            let ce = parts.headers.get("content-encoding").and_then(|v| v.to_str().ok()).map(|v| v.to_string());
            let decoded = crate::rules_ext::decode_for_edit(&bytes, ce.as_deref());
            let plan = crate::rules_ext::plan_response(
                &rules,
                &crate::rules_ext::ReqCtx { method: &p.method, url: &p.url, headers: &req_h, body: &p.body, process: p.process.as_deref() },
                &crate::rules_ext::RespCtx { status: parts.status.as_u16(), headers: &resp_h, body: decoded.as_deref().unwrap_or(&bytes) },
            );
            if !plan.applied.is_empty() {
                let names = plan.applied.join(", ");
                applied.extend(plan.applied.iter().cloned());
                if plan.delay_ms > 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(plan.delay_ms)).await;
                }
                if plan.close || plan.response.is_some() {
                    let (resp, rh, status, rb) = match &plan.response {
                        Some(rr) => {
                            let (resp, rh) = rule_local_response(rr, &names);
                            (resp, rh, rr.status, rr.body.to_vec())
                        }
                        None => (rule_close_response(), BTreeMap::new(), 0, Vec::new()),
                    };
                    self.push(SessionRecord {
                        id: 0,
                        method: p.method.clone(),
                        url: p.url.clone(),
                        status,
                        http_version: p.http_version.clone(),
                        response_http_version: "HTTP/1.1".into(),
                        request_headers: p.headers.clone(),
                        response_headers: rh,
                        request_body: crate::model::body_text(&p.body),
                        request_body_b64: crate::model::body_b64(&p.body),
                        response_body: crate::model::body_text(&rb),
                        response_body_b64: crate::model::body_b64(&rb),
                        start_time: p.start_time.clone(),
                        duration_ms: p.started.elapsed().as_millis(),
                        mark: Some(crate::model::SessionMark::Purple),
                        comment: if plan.close { format!("Rule closed connection: {names}") } else { format!("AutoResponder (response): {names}") },
                        ..Default::default()
                    });
                    return resp;
                }
                h_edits.extend(plan.resp_headers);
                b_edits.extend(plan.resp_body);
                if plan.resp_status.is_some() {
                    status_override = plan.resp_status;
                }
                if plan.kbps > 0 && (kbps == 0 || plan.kbps < kbps) {
                    kbps = plan.kbps;
                }
                loss = loss.max(plan.loss_pct);
            }
        }
        if let Some(s) = status_override.and_then(|s| hudsucker::hyper::StatusCode::from_u16(s).ok()) {
            parts.status = s;
        }
        if crate::rules_more::should_drop(loss) {
            self.push(SessionRecord {
                id: 0,
                method: p.method.clone(),
                url: p.url.clone(),
                status: 0,
                http_version: p.http_version.clone(),
                request_headers: p.headers.clone(),
                request_body: crate::model::body_text(&p.body),
                request_body_b64: crate::model::body_b64(&p.body),
                start_time: p.start_time.clone(),
                duration_ms: p.started.elapsed().as_millis(),
                mark: Some(crate::model::SessionMark::Purple),
                comment: format!("Throttle: connection dropped ({loss}% packet loss); rules: {}", applied.join(", ")),
                ..Default::default()
            });
            return rule_close_response();
        }
        if !b_edits.is_empty() {
            let ce = parts.headers.get("content-encoding").and_then(|v| v.to_str().ok()).map(|v| v.to_string());
            let decoded = crate::rules_ext::decode_for_edit(&bytes, ce.as_deref());
            if let Some(nb) = crate::rules_ext::apply_body_replace(decoded.as_deref().unwrap_or(&bytes), &b_edits) {
                bytes = Bytes::from(nb);
                if decoded.is_some() {
                    parts.headers.remove("content-encoding");
                }
                bp_fix_len(&mut parts.headers, bytes.len());
            }
        }
        if !h_edits.is_empty() {
            let mut hl = bp_headers(&parts.headers);
            crate::rules_ext::apply_header_edits(&mut hl, &h_edits);
            bp_set_headers(&mut parts.headers, &hl);
        }
        // Scripts (on_response) see the decoded body.
        if crate::scripting::has_hook(|h| h.on_response) {
            let req = crate::scripting::ReqParts {
                method: p.method.clone(),
                url: p.url.clone(),
                headers: p.headers.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                body: p.body.to_vec(),
            };
            let ce = parts.headers.get("content-encoding").and_then(|v| v.to_str().ok()).map(|v| v.to_string());
            let decoded = crate::rules_ext::decode_for_edit(&bytes, ce.as_deref());
            let was_decoded = decoded.is_some();
            let body_in = decoded.unwrap_or_else(|| bytes.to_vec());
            let resp = crate::scripting::RespParts { status: parts.status.as_u16(), headers: bp_headers(&parts.headers), body: body_in.clone() };
            if let Ok((resp, names)) = tokio::task::spawn_blocking(move || {
                let mut resp = resp;
                let names = crate::scripting::run_response(&req, &mut resp);
                (resp, names)
            })
            .await
            {
                if !names.is_empty() {
                    if let Ok(sc) = hudsucker::hyper::StatusCode::from_u16(resp.status) {
                        parts.status = sc;
                    }
                    bp_set_headers(&mut parts.headers, &resp.headers);
                    if resp.body != body_in {
                        bytes = Bytes::from(resp.body);
                        if was_decoded {
                            parts.headers.remove("content-encoding");
                        }
                        bp_fix_len(&mut parts.headers, bytes.len());
                    }
                    applied.push(format!("Script: {}", names.join(", ")));
                }
            }
        }
        // Breakpoints (response stage): match on status too. Decode the body for `~b` only when a
        // body-inspecting response breakpoint exists, to avoid decoding every response.
        let bp_body = if crate::model::bp_response_wants_body() {
            let ce = parts.headers.get("content-encoding").and_then(|v| v.to_str().ok());
            crate::model::body_text(&crate::rules_ext::decode_for_edit(&bytes, ce).unwrap_or_else(|| bytes.to_vec()))
        } else {
            String::new()
        };
        if let Some(rule) = crate::model::breakpoint_match(crate::model::BpStage::Response, &p.method, &p.url, parts.status.as_u16(), &bp_headers(&parts.headers), &bp_body) {
            let info = crate::model::PausedInfo {
                id: 0,
                stage: crate::model::BpStage::Response,
                rule: rule.clone(),
                method: p.method.clone(),
                url: p.url.clone(),
                status: parts.status.as_u16(),
                headers: bp_headers(&parts.headers),
                body: bytes.to_vec(),
                since_ms: 0,
            };
            match crate::model::breakpoint_hub().pause(info, BP_TIMEOUT).await {
                crate::model::PauseOutcome::Resumed(e) => {
                    if let Some(s) = e.status {
                        if let Ok(sc) = hudsucker::hyper::StatusCode::from_u16(s) {
                            parts.status = sc;
                        }
                    }
                    if let Some(h) = e.headers {
                        bp_set_headers(&mut parts.headers, &h);
                    }
                    if let Some(b) = e.body {
                        bytes = Bytes::from(b);
                        bp_fix_len(&mut parts.headers, bytes.len());
                    }
                }
                crate::model::PauseOutcome::Aborted => {
                    self.push(SessionRecord {
                        id: 0,
                        method: p.method.clone(),
                        url: p.url.clone(),
                        status: 502,
                        http_version: p.http_version.clone(),
                        response_http_version: "HTTP/1.1".into(),
                        request_headers: p.headers.clone(),
                        request_body: crate::model::body_text(&p.body),
                        request_body_b64: crate::model::body_b64(&p.body),
                        response_body: "Aborted at breakpoint by Ferrotap".into(),
                        start_time: p.start_time.clone(),
                        duration_ms: p.started.elapsed().as_millis(),
                        comment: format!("Breakpoint aborted: {rule}"),
                        ..Default::default()
                    });
                    return bp_abort_response();
                }
                crate::model::PauseOutcome::TimedOut => {}
            }
        }
        self.push(SessionRecord {
            id: 0,
            method: p.method.clone(),
            url: p.url.clone(),
            status: parts.status.as_u16(),
            http_version: p.http_version.clone(),
            response_http_version: ver(parts.version).to_string(),
            request_headers: p.headers.clone(),
            response_headers: hdrs(&parts.headers),
            request_body: crate::model::body_text(&p.body),
                        request_body_b64: crate::model::body_b64(&p.body),
            response_body: crate::model::body_text(&bytes),
            response_body_b64: crate::model::body_b64(&bytes),
            response_trailers: trailers.as_ref().map(trailer_list).unwrap_or_default(),
            start_time: p.start_time.clone(),
            duration_ms: p.started.elapsed().as_millis(),
            dns_ms: conn_info.dns_ms,
            connect_ms: conn_info.connect_ms,
            tls_ms: conn_info.tls_ms,
            ttfb_ms: Some(ttfb_total.saturating_sub(conn_info.dns_ms.unwrap_or(0) + conn_info.connect_ms.unwrap_or(0) + conn_info.tls_ms.unwrap_or(0))),
            send_ms: Some(p.sent.duration_since(p.started).as_millis() as u64),
            tls_version: conn_info.tls_version.clone(),
            comment: {
                let mut notes: Vec<String> = Vec::new();
                let (proto, rules): (Vec<&String>, Vec<&String>) = applied.iter().partition(|a| a.starts_with("HTTP/3 offer removed"));
                if !rules.is_empty() {
                    notes.push(format!("Rules: {}", rules.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
                }
                notes.extend(proto.into_iter().cloned());
                if let Some(e) = crate::conn::cert_error_for(&crate::rules_ext::url_host(&p.url)) {
                    notes.push(format!("Certificate error ignored: {e}"));
                }
                notes.join("; ")
            },
            ..Default::default()
        });
        let wait = crate::rules_more::throttle_delay(bytes.len(), kbps);
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
        let body = match trailers {
            // Forward trailers too (gRPC puts grpc-status there).
            Some(tr) => Body::from(
                Full::new(bytes)
                    .map_err(|never: std::convert::Infallible| match never {})
                    .with_trailers(async move { Some(Ok::<_, hudsucker::Error>(tr)) })
                    .boxed(),
            ),
            None => Body::from(Full::new(bytes)),
        };
        Response::from_parts(parts, body)
    }
}

fn load_or_create_ca(dir: &Path) -> Issuer<'static, KeyPair> {
    std::fs::create_dir_all(dir).expect("create ca dir");
    let cert_path = dir.join("ferrotap-ca.pem");
    let key_path = dir.join("ferrotap-ca.key");
    if !cert_path.exists() || !key_path.exists() {
        let kp = KeyPair::generate().expect("keygen");
        let mut p = CertificateParams::default();
        p.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, "Ferrotap Local Root CA");
        dn.push(DnType::OrganizationName, "Ferrotap");
        p.distinguished_name = dn;
        p.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
            KeyUsagePurpose::DigitalSignature,
        ];
        let cert = p.self_signed(&kp).expect("self sign");
        std::fs::write(&cert_path, cert.pem()).expect("write cert");
        std::fs::write(&key_path, kp.serialize_pem()).expect("write key");
        println!("Generated new CA: {}", cert_path.display());
    }
    let cert_pem = std::fs::read_to_string(&cert_path).expect("read cert");
    let key_pem = std::fs::read_to_string(&key_path).expect("read key");
    let kp = KeyPair::from_pem(&key_pem).expect("parse key");
    Issuer::from_ca_cert_pem(&cert_pem, kp).expect("parse ca cert")
}

/// Run the MITM proxy until Ctrl+C. Captured sessions are also sent on `tx` when given.
pub async fn run(port: u16, tx: Option<Sender<SessionEvent>>) -> Result<(), String> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let issuer = load_or_create_ca(&root.join("ca"));
    let ca = RcgenAuthority::new(issuer, 1_000, aws_lc_rs::default_provider());
    let store = Store::new(tx);
    if store.dump.is_some() {
        let s = store.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_millis(500));
            s.flush();
        });
    }
    let flush_store = store.clone();
    let handler = Recorder { store, pending: None, client: None, client_ip: None };

    let mut client = ClientBuilder::new(TokioExecutor::new());
    client.http1_preserve_header_case(true);
    client.pool_idle_timeout(std::time::Duration::from_secs(90));
    client.pool_max_idle_per_host(64);
    // real traffic (and debugging tools) can send more headers than hyper's default 100
    client.http1_max_headers(MAX_HEADERS);
    let mut server = ServerBuilder::new(TokioExecutor::new());
    server.http1().preserve_header_case(true).max_headers(MAX_HEADERS);

    crate::conn::ACTIVE_PORT.store(port, std::sync::atomic::Ordering::Relaxed);
    let proxy = Proxy::builder()
        .with_addr(if crate::conn::current().allow_remote { SocketAddr::from(([0, 0, 0, 0], port)) } else { SocketAddr::from(([127, 0, 0, 1], port)) })
        .with_ca(ca)
        .with_http_connector(crate::timing::build_connector())
        .with_client(client)
        .with_server(server)
        .with_http_handler(handler)
        .with_websocket_handler(ws::WsRecorder::new(root.join("ws_frames.json")))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .build()
        .map_err(|e| format!("build proxy: {e}"))?;
    println!(
        "MITM proxy listening on 127.0.0.1:{port}; CA: {}",
        root.join("ca").join("ferrotap-ca.pem").display()
    );
    let r = proxy.start().await.map_err(|e| format!("proxy error: {e}"));
    flush_store.flush();
    r
}

