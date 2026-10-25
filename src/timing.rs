//! Real connection timing + TLS capture for the proxy's upstream connections.
//!
//! The proxy's upstream connector is wrapped twice:
//!  * `TimedTcp` measures the TCP connect (DNS + connect) to the upstream server.
//!  * `TimedTls` measures the whole HTTPS connect, derives the TLS handshake time from it and
//!    records the negotiated TLS version and ALPN protocol (h2 / http/1.1).
//! Results are stored per `host:port`; `take_conn` hands them to the session that used the
//! connection (a reused keep-alive connection reports no connect/TLS time, only the TLS version).
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use base64::Engine as _;
use hudsucker::hyper::Uri;
use hudsucker::hyper_util::client::legacy::connect::Connect;
use hudsucker::rustls::{self, crypto::aws_lc_rs, pki_types::CertificateDer};
use hyper_rustls::{HttpsConnectorBuilder, MaybeHttpsStream};
use tower_service::Service;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConnInfo {
    /// Name lookup for the new connection (measured by conn.rs).
    pub dns_ms: Option<u64>,
    pub connect_ms: Option<u64>,
    pub tls_ms: Option<u64>,
    pub tls_version: Option<String>,
    /// Negotiated application protocol: "h2" or "http/1.1" (None for plain HTTP).
    pub alpn: Option<String>,
}

fn table() -> &'static Mutex<HashMap<String, ConnInfo>> {
    static T: OnceLock<Mutex<HashMap<String, ConnInfo>>> = OnceLock::new();
    T.get_or_init(|| Mutex::new(HashMap::new()))
}

fn update(key: &str, f: impl FnOnce(&mut ConnInfo)) {
    if let Ok(mut t) = table().lock() {
        f(t.entry(key.to_string()).or_default());
    }
}

/// Record the DNS lookup time of a new connection to `key` (host:port).
pub fn record_dns(key: &str, d: Duration) {
    let v = ms(d);
    update(key, |c| c.dns_ms = Some(v));
}

/// Milliseconds, rounded up so a measured non-zero duration never shows as 0.
fn ms(d: Duration) -> u64 {
    (d.as_micros() as u64).div_ceil(1000)
}

pub fn key_of_uri(uri: &Uri) -> String {
    let host = uri.host().unwrap_or("").to_ascii_lowercase();
    let port = uri
        .port_u16()
        .unwrap_or(if uri.scheme_str() == Some("http") { 80 } else { 443 });
    format!("{host}:{port}")
}

pub fn key_of_url(url: &str) -> String {
    url.parse::<Uri>().map(|u| key_of_uri(&u)).unwrap_or_default()
}

/// Fetch the timing/TLS info for the connection that served `url`. Connect/TLS times are
/// handed out once (a later request on the same keep-alive connection gets None for them).
pub fn take_conn(url: &str) -> ConnInfo {
    let key = key_of_url(url);
    let Ok(mut t) = table().lock() else {
        return ConnInfo::default();
    };
    match t.get_mut(&key) {
        Some(c) => {
            let out = c.clone();
            c.dns_ms = None;
            c.connect_ms = None;
            c.tls_ms = None;
            out
        }
        None => ConnInfo::default(),
    }
}

/// Map a negotiated rustls version to the text shown in the grid.
pub fn tls_version_name(v: rustls::ProtocolVersion) -> String {
    match v {
        rustls::ProtocolVersion::TLSv1_3 => "TLS 1.3".into(),
        rustls::ProtocolVersion::TLSv1_2 => "TLS 1.2".into(),
        other => format!("{other:?}"),
    }
}

#[derive(Clone)]
pub struct TimedTcp<S>(pub S);

impl<S> Service<Uri> for TimedTcp<S>
where
    S: Service<Uri>,
    S::Future: Send + 'static,
    S::Response: Send + 'static,
    S::Error: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<S::Response, S::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.0.poll_ready(cx)
    }

    fn call(&mut self, uri: Uri) -> Self::Future {
        let key = key_of_uri(&uri);
        update(&key, |c| c.dns_ms = None);
        let t = Instant::now();
        let fut = self.0.call(uri);
        Box::pin(async move {
            let r = fut.await;
            if r.is_ok() {
                let d = ms(t.elapsed());
                // The connector measured DNS separately: TCP connect is the rest.
                update(&key, |c| c.connect_ms = Some(d.saturating_sub(c.dns_ms.unwrap_or(0)).max(1)));
            }
            r
        })
    }
}

#[derive(Clone)]
pub struct TimedTls<S>(pub S);

impl<S, T> Service<Uri> for TimedTls<S>
where
    S: Service<Uri, Response = MaybeHttpsStream<T>>,
    S::Future: Send + 'static,
    S::Error: Send + 'static,
    T: Send + 'static,
{
    type Response = MaybeHttpsStream<T>;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<MaybeHttpsStream<T>, S::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.0.poll_ready(cx)
    }

    fn call(&mut self, uri: Uri) -> Self::Future {
        let key = key_of_uri(&uri);
        let t = Instant::now();
        let fut = self.0.call(uri);
        Box::pin(async move {
            let r = fut.await;
            let total = ms(t.elapsed());
            match &r {
                Ok(MaybeHttpsStream::Https(tls)) => {
                    let (_, conn) = tls.inner().get_ref();
                    let ver = conn.protocol_version().map(tls_version_name);
                    let alpn = conn
                        .alpn_protocol()
                        .map(|p| String::from_utf8_lossy(p).to_string())
                        .or_else(|| Some("http/1.1".to_string()));
                    update(&key, |c| {
                        let tcp = c.connect_ms.unwrap_or(0) + c.dns_ms.unwrap_or(0);
                        c.tls_ms = Some(total.saturating_sub(tcp).max(1));
                        c.tls_version = ver;
                        c.alpn = alpn;
                    });
                }
                Ok(_) => update(&key, |c| {
                    c.tls_version = None;
                    c.alpn = None;
                }),
                Err(_) => {}
            }
            r
        })
    }
}

/// Extract the DER bytes of every CERTIFICATE block in a PEM text.
pub fn pem_certs(txt: &str) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut inside = false;
    for l in txt.lines() {
        let l = l.trim();
        if l.starts_with("-----BEGIN CERTIFICATE") {
            inside = true;
            cur.clear();
        } else if l.starts_with("-----END CERTIFICATE") {
            inside = false;
            if let Ok(d) = base64::engine::general_purpose::STANDARD.decode(&cur) {
                out.push(d);
            }
        } else if inside {
            cur.push_str(l);
        }
    }
    out
}

/// The proxy's upstream connector: web PKI roots (+ optional extra roots from the PEM file named
/// by `FERROTAP_EXTRA_CA`, for private CAs), HTTP/1.1 + h2 via ALPN, with timing capture.
pub fn build_connector() -> impl Connect + Clone + Send + Sync + 'static {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    if let Ok(p) = std::env::var("FERROTAP_EXTRA_CA") {
        if let Ok(txt) = std::fs::read_to_string(&p) {
            for der in pem_certs(&txt) {
                let _ = roots.add(CertificateDer::from(der));
            }
        }
    }
    let provider = Arc::new(aws_lc_rs::default_provider());
    // Web PKI verification; with "ignore certificate errors" on, failures are noted and allowed (conn.rs).
    let inner = rustls::client::WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider.clone())
        .build()
        .expect("certificate verifier");
    let mut cfg = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("tls protocol versions")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(crate::conn::LenientVerifier { inner }))
        .with_no_client_auth();
    // Writes secrets only when a key log file is configured (Connections > TLS key log file).
    cfg.key_log = Arc::new(crate::tlsdecrypt::ProxyKeyLog);
    // TCP via conn.rs: host remapping and the upstream gateway.
    let https = HttpsConnectorBuilder::new()
        .with_tls_config(cfg)
        .https_or_http()
        .enable_http1()
        .enable_http2()
        .wrap_connector(TimedTcp(crate::conn::FerroConnector));
    TimedTls(https)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_default_ports() {
        assert_eq!(key_of_url("https://Example.com/a"), "example.com:443");
        assert_eq!(key_of_url("http://example.com/a"), "example.com:80");
        assert_eq!(key_of_url("https://localhost:8443/x"), "localhost:8443");
    }

    #[test]
    fn take_conn_hands_out_times_once() {
        update("unit.test:443", |c| {
            c.connect_ms = Some(5);
            c.tls_ms = Some(7);
            c.tls_version = Some("TLS 1.3".into());
        });
        let a = take_conn("https://unit.test/");
        assert_eq!(a.connect_ms, Some(5));
        assert_eq!(a.tls_ms, Some(7));
        let b = take_conn("https://unit.test/again");
        assert_eq!(b.connect_ms, None);
        assert_eq!(b.tls_version.as_deref(), Some("TLS 1.3"));
    }
}
