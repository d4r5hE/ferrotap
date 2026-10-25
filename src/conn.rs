//! G8 connection settings: listening port / remote devices, host remapping, upstream gateway,
//! "ignore server certificate errors". Saved in data/conn.json (env FERROTAP_CONN_SETTINGS).
//! The proxy reads `current()` for every new upstream connection, so most changes apply at once;
//! the port and remote access apply when the proxy starts.
use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::task::{Context, Poll};
use std::time::Duration;

use hudsucker::hyper::Uri;
use hudsucker::hyper_util::rt::TokioIo;
use hudsucker::rustls;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tower_service::Service;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct HostMapEntry {
    pub enabled: bool,
    /// Host name to remap (exact, case-insensitive; `*.example.com` matches subdomains).
    pub host: String,
    /// Where to connect instead: an IP address or another host name.
    pub target: String,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum UpstreamMode {
    /// Connect to servers directly.
    #[default]
    None,
    /// Use the proxy configured in Windows (ignored when it points at Ferrotap itself).
    System,
    /// Use `upstream_manual` (host:port, optionally user:pass@host:port).
    Manual,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct ConnSettings {
    pub port: u16,
    /// Listen on every network interface so phones / other PCs can use the proxy.
    pub allow_remote: bool,
    /// Accept invalid / self-signed server certificates (unsafe); each session notes the error.
    pub ignore_cert_errors: bool,
    pub host_map_enabled: bool,
    pub host_map: Vec<HostMapEntry>,
    pub upstream: UpstreamMode,
    pub upstream_manual: String,
    /// Hosts that skip the upstream gateway (`*.corp`, `localhost`, `<local>` = names without a dot).
    pub upstream_bypass: Vec<String>,
    /// Which sessions are recorded (traffic is always proxied).
    pub capture_mode: CaptureMode,
    /// Process image names (`chrome.exe` or `chrome`) and server ports for the capture filter.
    pub capture_processes: Vec<String>,
    pub capture_ports: Vec<u16>,
    /// Hosts that bypass the Windows system proxy while it points at Ferrotap (ProxyOverride).
    pub system_bypass: Vec<String>,
    /// Default browser (exe path; "" = first one found), its profile folder ("" = a fresh temp one)
    /// and the default terminal ("powershell", "cmd", "wt", "bash").
    pub browser: String,
    pub browser_data_dir: String,
    pub terminal: String,
    /// Streaming mode: forward every response body as it arrives (Server-Sent Events and gRPC
    /// always stream). Response rules, scripts and throttling need the whole body and turn it off.
    pub streaming: bool,
    /// Remove HTTP/3 offers (Alt-Svc) so clients do not switch to QUIC around the proxy.
    pub strip_h3: bool,
    /// Append TLS secrets (SSLKEYLOGFILE format) of the proxy's upstream connections and of the
    /// clean browser here, for decrypting packet captures ("" = off).
    pub keylog: String,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CaptureMode {
    #[default]
    All,
    /// Only sessions from the listed processes / to the listed ports.
    Only,
    /// Everything except the listed processes / ports.
    Except,
}

impl Default for ConnSettings {
    fn default() -> Self {
        ConnSettings {
            port: crate::proxy::PORT,
            allow_remote: false,
            ignore_cert_errors: false,
            host_map_enabled: true,
            host_map: Vec::new(),
            upstream: UpstreamMode::None,
            upstream_manual: String::new(),
            upstream_bypass: vec!["<local>".into(), "localhost".into(), "127.0.0.1".into()],
            capture_mode: CaptureMode::All,
            capture_processes: Vec::new(),
            capture_ports: Vec::new(),
            system_bypass: vec!["<local>".into()],
            browser: String::new(),
            browser_data_dir: String::new(),
            terminal: "powershell".into(),
            streaming: false,
            strip_h3: true,
            keylog: String::new(),
        }
    }
}

/// A proxy address with optional basic credentials.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProxyAddr {
    pub host: String,
    pub port: u16,
    pub auth: Option<String>,
}

/// Parse `host:port`, `http://host:port`, `user:pass@host:port` (port defaults to 8080).
pub fn parse_proxy_addr(s: &str) -> Option<ProxyAddr> {
    let s = s.trim().trim_start_matches("http://").trim_end_matches('/');
    if s.is_empty() {
        return None;
    }
    let (auth, hp) = match s.rsplit_once('@') {
        Some((a, h)) => (Some(a.to_string()), h),
        None => (None, s),
    };
    let (host, port) = match hp.rsplit_once(':') {
        Some((h, p)) if !h.contains(']') || h.ends_with(']') => (h, p.parse().ok()?),
        _ => (hp, 8080),
    };
    let host = host.trim_matches(['[', ']']).to_string();
    (!host.is_empty()).then_some(ProxyAddr { host, port, auth })
}

/// `pat` matches `host`: exact, `*.domain` (subdomains and the domain), `<local>` (no dot).
pub fn host_pattern_matches(pat: &str, host: &str) -> bool {
    let (p, h) = (pat.trim().to_ascii_lowercase(), host.trim().trim_matches(['[', ']']).to_ascii_lowercase());
    if p.is_empty() {
        return false;
    }
    if p == "<local>" {
        return !h.contains('.') && !h.contains(':');
    }
    if let Some(d) = p.strip_prefix("*.") {
        return h == d || h.ends_with(&format!(".{d}"));
    }
    if let Some(d) = p.strip_prefix('*') {
        return h.ends_with(d);
    }
    p == h
}

impl ConnSettings {
    pub fn default_path() -> PathBuf {
        std::env::var("FERROTAP_CONN_SETTINGS").map(PathBuf::from).unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data").join("conn.json")
        })
    }

    /// Missing or unreadable file -> defaults.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let j = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, j).map_err(|e| format!("write {}: {e}", path.display()))
    }

    /// Where to connect for `host` after host remapping (None = unchanged).
    pub fn map_host(&self, host: &str) -> Option<String> {
        if !self.host_map_enabled {
            return None;
        }
        self.host_map
            .iter()
            .find(|e| e.enabled && !e.target.trim().is_empty() && host_pattern_matches(&e.host, host))
            .map(|e| e.target.trim().to_string())
    }

    /// The upstream gateway to use for `host`, if any.
    pub fn upstream_for(&self, host: &str) -> Option<ProxyAddr> {
        let p = match self.upstream {
            UpstreamMode::None => return None,
            UpstreamMode::Manual => parse_proxy_addr(&self.upstream_manual)?,
            UpstreamMode::System => {
                let (server, bypass) = system_proxy_setting()?;
                let p = parse_system_proxy(&server)?;
                if is_ourselves(&p, self.port) || bypass.iter().any(|b| host_pattern_matches(b, host)) {
                    return None;
                }
                p
            }
        };
        if self.upstream_bypass.iter().any(|b| host_pattern_matches(b, host)) {
            return None;
        }
        Some(p)
    }

    /// The capture filter: does `s` match the listed processes or ports?
    fn filter_hit(&self, s: &crate::model::SessionRecord) -> bool {
        let norm = |n: &str| n.trim().trim_end_matches(".exe").trim_end_matches(".EXE").to_ascii_lowercase();
        let proc_hit = s.process_name.as_deref().is_some_and(|p| {
            let p = norm(p);
            self.capture_processes.iter().any(|x| !x.trim().is_empty() && norm(x) == p)
        });
        let port = s.url.parse::<Uri>().ok().and_then(|u| u.port_u16().or(match u.scheme_str() {
            Some("http") | Some("ws") => Some(80),
            Some("https") | Some("wss") => Some(443),
            _ => None,
        }));
        proc_hit || port.is_some_and(|p| self.capture_ports.contains(&p))
    }

    /// Whether a session is recorded under the capture filter.
    pub fn should_capture(&self, s: &crate::model::SessionRecord) -> bool {
        match self.capture_mode {
            CaptureMode::All => true,
            CaptureMode::Only => self.filter_hit(s),
            CaptureMode::Except => !self.filter_hit(s),
        }
    }

    /// The ProxyOverride value for the Windows system proxy.
    pub fn system_override(&self) -> String {
        let v: Vec<&str> = self.system_bypass.iter().map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
        if v.is_empty() { "<local>".into() } else { v.join(";") }
    }

    /// The listen address for the proxy.
    pub fn listen_addr(&self) -> std::net::SocketAddr {
        let ip = if self.allow_remote { [0, 0, 0, 0] } else { [127, 0, 0, 1] };
        std::net::SocketAddr::from((ip, self.port))
    }
}

fn is_ourselves(p: &ProxyAddr, our_port: u16) -> bool {
    p.port == our_port && matches!(p.host.to_ascii_lowercase().as_str(), "127.0.0.1" | "localhost" | "::1" | "0.0.0.0")
}

/// WinINET ProxyServer value forms: `host:port` or `http=h:p;https=h:p;...` (the https entry wins).
pub fn parse_system_proxy(server: &str) -> Option<ProxyAddr> {
    if !server.contains('=') {
        return parse_proxy_addr(server);
    }
    let parts: HashMap<String, String> = server
        .split(';')
        .filter_map(|x| x.split_once('=').map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())))
        .collect();
    parts.get("https").or_else(|| parts.get("http")).and_then(|v| parse_proxy_addr(v))
}

/// The enabled Windows proxy (server, bypass list), if any.
pub fn system_proxy_setting() -> Option<(String, Vec<String>)> {
    #[cfg(windows)]
    {
        let key = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
            .open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings")
            .ok()?;
        let enabled: u32 = key.get_value("ProxyEnable").unwrap_or(0);
        let server: String = key.get_value("ProxyServer").unwrap_or_default();
        if enabled == 0 || server.trim().is_empty() {
            return None;
        }
        let bypass: String = key.get_value("ProxyOverride").unwrap_or_default();
        Some((server, bypass.split(';').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()))
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Entries of a hosts file (`ip name1 name2 # comment`), as host -> ip mappings.
pub fn parse_hosts_file(text: &str) -> Vec<HostMapEntry> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let mut it = line.split_whitespace();
        let Some(ip) = it.next() else { continue };
        if ip.parse::<std::net::IpAddr>().is_err() {
            continue;
        }
        for name in it {
            out.push(HostMapEntry { enabled: true, host: name.to_string(), target: ip.to_string() });
        }
    }
    out
}

pub fn os_hosts_path() -> PathBuf {
    if cfg!(windows) {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
        PathBuf::from(root).join("System32").join("drivers").join("etc").join("hosts")
    } else {
        PathBuf::from("/etc/hosts")
    }
}

// ------------------------------------------------------------------------------------------ globals

fn lock() -> &'static RwLock<ConnSettings> {
    static S: OnceLock<RwLock<ConnSettings>> = OnceLock::new();
    S.get_or_init(|| RwLock::new(ConnSettings::load(&ConnSettings::default_path())))
}

/// The settings the proxy uses now.
pub fn current() -> ConnSettings {
    lock().read().map(|g| g.clone()).unwrap_or_default()
}

/// Replace the live settings (does not save).
pub fn set_current(s: ConnSettings) {
    if let Ok(mut g) = lock().write() {
        *g = s;
    }
}

/// Port the running proxy listens on (set by `proxy::run`; the saved setting until then).
pub fn active_port() -> u16 {
    let p = ACTIVE_PORT.load(std::sync::atomic::Ordering::Relaxed);
    if p == 0 { current().port } else { p }
}

pub(crate) static ACTIVE_PORT: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);

/// The PC's IPv4 addresses on local networks (for setting up a phone), best effort.
pub fn local_ipv4s() -> Vec<String> {
    let mut v = Vec::new();
    // The OS picks the outgoing interface for a UDP "connect" without sending anything.
    for probe in ["8.8.8.8:80", "10.255.255.255:1", "192.168.255.255:1"] {
        if let Ok(s) = std::net::UdpSocket::bind("0.0.0.0:0") {
            if s.connect(probe).is_ok() {
                if let Ok(a) = s.local_addr() {
                    let ip = a.ip().to_string();
                    if !a.ip().is_loopback() && !a.ip().is_unspecified() && !v.contains(&ip) {
                        v.push(ip);
                    }
                }
            }
        }
    }
    v
}

// ------------------------------------------------------------------------------------------ cert errors

fn cert_errors() -> &'static Mutex<HashMap<String, String>> {
    static M: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    M.get_or_init(Default::default)
}

/// The certificate error that was ignored for `host`, if any.
pub fn cert_error_for(host: &str) -> Option<String> {
    cert_errors().lock().ok()?.get(&host.to_ascii_lowercase()).cloned()
}

/// Verifies like the web PKI verifier; when "ignore certificate errors" is on, a failure is recorded
/// for the host and the connection continues.
#[derive(Debug)]
pub struct LenientVerifier {
    pub inner: Arc<rustls::client::WebPkiServerVerifier>,
}

impl rustls::client::danger::ServerCertVerifier for LenientVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        intermediates: &[rustls::pki_types::CertificateDer<'_>],
        server_name: &rustls::pki_types::ServerName<'_>,
        ocsp: &[u8],
        now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        let host = server_name.to_str().to_ascii_lowercase();
        match self.inner.verify_server_cert(end_entity, intermediates, server_name, ocsp, now) {
            Ok(v) => {
                if let Ok(mut m) = cert_errors().lock() {
                    m.remove(&host);
                }
                Ok(v)
            }
            Err(e) if current().ignore_cert_errors => {
                if let Ok(mut m) = cert_errors().lock() {
                    m.insert(host, e.to_string());
                }
                Ok(rustls::client::danger::ServerCertVerified::assertion())
            }
            Err(e) => Err(e),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

// ------------------------------------------------------------------------------------------ connector

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

fn io_err(msg: impl Into<String>) -> std::io::Error {
    std::io::Error::other(msg.into())
}

/// Open a TCP connection for `uri`, applying host remapping and the upstream gateway.
pub async fn connect_uri(uri: &Uri) -> std::io::Result<TcpStream> {
    let host = uri.host().ok_or_else(|| io_err("URI has no host"))?.trim_matches(['[', ']']).to_string();
    let port = uri.port_u16().unwrap_or(if uri.scheme_str() == Some("https") { 443 } else { 80 });
    let s = current();
    let key = crate::timing::key_of_uri(uri);
    let target = s.map_host(&host).unwrap_or_else(|| host.clone());
    if let Some(up) = s.upstream_for(&host) {
        let mut tcp = connect_host(&up.host, up.port, &key).await.map_err(|e| io_err(format!("upstream proxy {}:{}: {e}", up.host, up.port)))?;
        let t = if target.contains(':') { format!("[{target}]:{port}") } else { format!("{target}:{port}") };
        let mut req = format!("CONNECT {t} HTTP/1.1\r\nHost: {t}\r\n");
        if let Some(a) = &up.auth {
            use base64::Engine;
            req.push_str(&format!("Proxy-Authorization: Basic {}\r\n", base64::engine::general_purpose::STANDARD.encode(a)));
        }
        req.push_str("\r\n");
        tcp.write_all(req.as_bytes()).await?;
        let mut head = Vec::new();
        let mut b = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            if head.len() > 16 * 1024 {
                return Err(io_err("upstream proxy sent an oversized CONNECT answer"));
            }
            let n = tokio::time::timeout(CONNECT_TIMEOUT, tcp.read(&mut b)).await.map_err(|_| io_err("upstream proxy timed out"))??;
            if n == 0 {
                return Err(io_err("upstream proxy closed the connection"));
            }
            head.push(b[0]);
        }
        let line = String::from_utf8_lossy(&head).lines().next().unwrap_or("").to_string();
        if line.split_whitespace().nth(1) != Some("200") {
            return Err(io_err(format!("upstream proxy refused CONNECT {t}: {line}")));
        }
        return Ok(tcp);
    }
    connect_host(&target, port, &key).await
}

async fn connect_host(host: &str, port: u16, timing_key: &str) -> std::io::Result<TcpStream> {
    let t = std::time::Instant::now();
    let addrs: Vec<_> = tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::lookup_host((host, port)))
        .await
        .map_err(|_| io_err(format!("dns lookup for {host} timed out")))?
        .map_err(|e| io_err(format!("dns error for {host}: {e}")))?
        .collect();
    crate::timing::record_dns(timing_key, t.elapsed());
    let mut last = io_err(format!("no addresses for {host}"));
    for a in addrs {
        match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(a)).await {
            Ok(Ok(s)) => {
                let _ = s.set_nodelay(true);
                return Ok(s);
            }
            Ok(Err(e)) => last = io_err(format!("tcp connect error: {e} ({a})")),
            Err(_) => last = io_err(format!("tcp connect to {a} timed out")),
        }
    }
    Err(last)
}

/// The proxy's TCP connector (wrapped by TLS + timing in `timing::build_connector`).
#[derive(Clone, Default)]
pub struct FerroConnector;

impl Service<Uri> for FerroConnector {
    type Response = TokioIo<TcpStream>;
    type Error = std::io::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, uri: Uri) -> Self::Future {
        Box::pin(async move { connect_uri(&uri).await.map(TokioIo::new) })
    }
}

// ------------------------------------------------------------------------------------------ device page

/// Magic host names answered by the proxy itself (setup page + CA download for phones).
pub const SETUP_HOSTS: [&str; 2] = ["ferrotap.ca", "ferrotap"];

pub fn is_setup_host(host: &str) -> bool {
    let h = host.split(':').next().unwrap_or("").to_ascii_lowercase();
    SETUP_HOSTS.contains(&h.as_str())
}

/// (status, content type, body) for a request to the setup host; `ca_pem` is our root CA.
pub fn setup_page(path: &str, ca_pem: Option<&str>) -> (u16, &'static str, Vec<u8>) {
    let p = path.split('?').next().unwrap_or("/");
    match (p, ca_pem) {
        ("/ferrotap-ca.crt" | "/ca" | "/ca.crt", Some(pem)) => (200, "application/x-x509-ca-cert", pem.as_bytes().to_vec()),
        ("/ferrotap-ca.crt" | "/ca" | "/ca.crt", None) => (404, "text/plain; charset=utf-8", b"The root CA is not available.".to_vec()),
        ("/" | "/index.html", _) => {
            let html = "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\">\
<title>Ferrotap</title></head><body style=\"font-family:sans-serif;margin:24px;background:#fff;color:#222\">\
<h1>Ferrotap is reachable</h1><p>This device's traffic goes through Ferrotap.</p>\
<p><a href=\"/ferrotap-ca.crt\"><b>Download the Ferrotap root certificate</b></a> to see HTTPS traffic, then trust it:</p>\
<ul><li><b>Android</b>: Settings &gt; Security &gt; Encryption &amp; credentials &gt; Install a certificate &gt; CA certificate.</li>\
<li><b>iOS</b>: open the downloaded profile in Settings &gt; Profile Downloaded &gt; Install, then Settings &gt; General &gt; About &gt;\
 Certificate Trust Settings and turn on full trust for Ferrotap Local Root CA.</li></ul>\
<p>Remove the certificate when you are done.</p></body></html>";
            (200, "text/html; charset=utf-8", html.as_bytes().to_vec())
        }
        _ => (404, "text/plain; charset=utf-8", b"Not found. Open http://ferrotap.ca/".to_vec()),
    }
}


// ------------------------------------------------------------------------------------------ VPN notice

/// A network adapter that looks like a VPN.
#[derive(Debug, Clone, PartialEq)]
pub struct VpnAdapter {
    pub name: String,
    pub description: String,
    /// Has a routable IPv4 address (not just 169.254 autoconfiguration) and media is connected.
    pub active: bool,
    /// Its DNS suffix (often the company domain): a candidate for the bypass list.
    pub dns_suffix: String,
}

const VPN_WORDS: &[&str] = &[
    "vpn", "wireguard", "openvpn", "tap-windows", "wintun", "anyconnect", "fortinet", "fortissl", "globalprotect", "pangp", "juniper", "pulse secure",
    "zscaler", "nordlynx", "tailscale", "zerotier", "proton", "f5 ", "checkpoint", "sonicwall", "ppp adapter", "wan miniport (ikev2)",
];

/// VPN-looking adapters in `ipconfig /all` output.
pub fn parse_ipconfig(text: &str) -> Vec<VpnAdapter> {
    let mut out = Vec::new();
    let mut cur: Option<(String, Vec<(String, String)>)> = None;
    let flush = |c: Option<(String, Vec<(String, String)>)>, out: &mut Vec<VpnAdapter>| {
        let Some((name, kv)) = c else { return };
        let get = |k: &str| kv.iter().find(|(a, _)| a.starts_with(k)).map(|(_, v)| v.clone()).unwrap_or_default();
        let description = get("Description");
        let hay = format!("{} {name} {description}", get("Header")).to_ascii_lowercase();
        if !VPN_WORDS.iter().any(|w| hay.contains(w)) {
            return;
        }
        let disconnected = get("Media State").to_ascii_lowercase().contains("disconnected");
        let ipv4 = get("IPv4 Address");
        out.push(VpnAdapter { name, description, active: !disconnected && !ipv4.is_empty() && !ipv4.starts_with("169.254"), dns_suffix: get("Connection-specific DNS Suffix") });
    };
    for line in text.lines() {
        if !line.starts_with(' ') && line.trim_end().ends_with(':') && line.to_ascii_lowercase().contains("adapter") {
            flush(cur.take(), &mut out);
            let name = line.trim_end().trim_end_matches(':');
            let name = name.split_once(" adapter ").map(|(_, n)| n).unwrap_or(name).trim().to_string();
            cur = Some((name, vec![("Header".to_string(), line.trim().to_string())]));
        } else if let Some((_, kv)) = cur.as_mut() {
            if let Some((k, v)) = line.split_once(" : ") {
                let k = k.trim().trim_end_matches(['.', ' ']).trim().to_string();
                kv.push((k, v.trim().trim_end_matches("(Preferred)").trim().to_string()));
            }
        }
    }
    flush(cur.take(), &mut out);
    out
}

/// Run `ipconfig /all` (hidden window) and return the VPN adapters.
pub fn detect_vpns() -> Vec<VpnAdapter> {
    let mut c = std::process::Command::new("ipconfig");
    c.arg("/all");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    c.output().map(|o| parse_ipconfig(&String::from_utf8_lossy(&o.stdout))).unwrap_or_default()
}

// ------------------------------------------------------------------------------------------ window

/// UI state of the Connections window (GridApp.ext.conn).
#[derive(Default)]
pub struct ConnUi {
    pub show: bool,
    /// Settings being edited (loaded when the window opens).
    pub draft: Option<ConnSettings>,
    pub msg: String,
    pub bypass_text: String,
    pub procs_text: String,
    pub ports_text: String,
    pub sysbypass_text: String,
    pub ips: Vec<String>,
    /// VPN adapters found when the window opened (filled in the background).
    pub vpns: Option<Vec<VpnAdapter>>,
    pub vpn_rx: Option<std::sync::mpsc::Receiver<Vec<VpnAdapter>>>,
    /// HTTPS window: cached (trusted, CA info, expiry warning); None = read again.
    pub ca_facts: Option<(bool, Result<crate::ca_trust::CaInfo, String>, Option<String>)>,
    /// certutil work in progress (it can wait on a Windows confirmation dialog).
    pub ca_job: Option<std::sync::mpsc::Receiver<CaJob>>,
    pub confirm_regen: bool,
    pub custom_cert: String,
    pub custom_key: String,
}

/// Result of CA work done off the UI thread.
pub enum CaJob {
    /// (trusted, CA info, expiry warning)
    Facts(bool, Result<crate::ca_trust::CaInfo, String>, Option<String>),
    /// Trust / untrust finished: the message to show.
    Done(String),
}

/// Run `f` on a thread; its result arrives on the returned channel.
pub fn ca_job(ctx: &eframe::egui::Context, f: impl FnOnce() -> CaJob + Send + 'static) -> std::sync::mpsc::Receiver<CaJob> {
    let (tx, rx) = std::sync::mpsc::channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let _ = tx.send(f());
        ctx.request_repaint();
    });
    rx
}

impl crate::ui::GridApp {
    /// Toolbar button.
    pub fn conn_menu(&mut self, ui: &mut eframe::egui::Ui) {
        if ui.selectable_label(self.ext.conn.show, "Connections").on_hover_text("Port, remote devices, host remapping, upstream proxy, certificate errors").clicked() {
            self.ext.conn.show = !self.ext.conn.show;
        }
    }

    pub fn conn_windows(&mut self, ctx: &eframe::egui::Context) {
        use eframe::egui::{self, Color32, RichText};
        let st = &mut self.ext.conn;
        if !st.show {
            st.draft = None;
            return;
        }
        if st.draft.is_none() {
            let s = current();
            st.bypass_text = s.upstream_bypass.join(", ");
            st.procs_text = s.capture_processes.join(", ");
            st.ports_text = s.capture_ports.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(", ");
            st.sysbypass_text = s.system_bypass.join("; ");
            st.draft = Some(s);
            st.ips = local_ipv4s();
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(detect_vpns());
            });
            st.vpns = None;
            st.vpn_rx = Some(rx);
        }
        if let Some(rx) = &st.vpn_rx {
            if let Ok(v) = rx.try_recv() {
                st.vpns = Some(v);
                st.vpn_rx = None;
            }
        }
        let mut open = true;
        let (mut save, mut import_hosts) = (false, false);
        let gray = Color32::GRAY;
        let ConnUi { draft, msg, bypass_text, procs_text, ports_text, sysbypass_text, ips, vpns, .. } = &mut self.ext.conn;
        let d = draft.as_mut().expect("draft");
        egui::Window::new("Connections").open(&mut open).default_width(560.0).vscroll(true).show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    save = true;
                }
                ui.label(msg.as_str());
            });
            ui.separator();
            ui.label(RichText::new("Listening").strong());
            ui.horizontal(|ui| {
                ui.label("Port");
                ui.add(egui::DragValue::new(&mut d.port).range(1024..=65535));
                ui.checkbox(&mut d.allow_remote, "Allow remote devices to connect");
            });
            ui.label(
                RichText::new(format!(
                    "Now listening on {}:{} (port and remote access apply when Ferrotap restarts)",
                    if current().allow_remote { "0.0.0.0" } else { "127.0.0.1" },
                    active_port()
                ))
                .small()
                .color(gray),
            );
            if d.allow_remote {
                ui.colored_label(Color32::from_rgb(230, 160, 40), "Anyone on your network can then send traffic through this PC while Ferrotap runs.");
            }
            ui.collapsing("Set up a phone or another PC", |ui| {
                if ips.is_empty() {
                    ui.label("No local network address found.");
                }
                for ip in ips.iter() {
                    ui.monospace(format!("Proxy: {ip}  port {}", d.port));
                }
                ui.label("1. Turn on \"Allow remote devices\", save and restart Ferrotap (Windows may ask to allow it through the firewall).");
                ui.label("2. On the device, set the Wi-Fi HTTP proxy to the address above (manual proxy).");
                ui.label("3. Open http://ferrotap.ca/ on the device: if the page loads the connection works; it also offers the root certificate.");
                if ui.small_button("Refresh addresses").clicked() {
                    *ips = local_ipv4s();
                }
            });
            ui.separator();
            ui.label(RichText::new("What is recorded").strong());
            ui.horizontal(|ui| {
                ui.radio_value(&mut d.capture_mode, CaptureMode::All, "Everything");
                ui.radio_value(&mut d.capture_mode, CaptureMode::Only, "Only these");
                ui.radio_value(&mut d.capture_mode, CaptureMode::Except, "All except these");
            });
            if d.capture_mode != CaptureMode::All {
                ui.horizontal(|ui| {
                    ui.label("Processes");
                    ui.add(egui::TextEdit::singleline(procs_text).hint_text("chrome.exe, node, python").desired_width(300.0));
                });
                ui.horizontal(|ui| {
                    ui.label("Server ports");
                    ui.add(egui::TextEdit::singleline(ports_text).hint_text("443, 8080").desired_width(200.0));
                });
                ui.label(RichText::new("A session matches by its client process (local programs) or its server port. Traffic still flows; only recording is filtered.").small().color(gray));
            }
            ui.horizontal(|ui| {
                ui.label("System proxy bypass");
                ui.add(egui::TextEdit::singleline(sysbypass_text).desired_width(320.0));
            });
            if let Some(list) = vpns.as_ref().filter(|l| l.iter().any(|v| v.active)) {
                ui.colored_label(Color32::from_rgb(230, 160, 40), format!(
                    "VPN active: {}. If internal sites stop working while the system proxy is on, add their domains here.",
                    list.iter().filter(|v| v.active).map(|v| v.name.as_str()).collect::<Vec<_>>().join(", ")
                ));
                for v in list.iter().filter(|v| v.active && !v.dns_suffix.is_empty()) {
                    let entry = format!("*.{}", v.dns_suffix);
                    if !sysbypass_text.contains(&entry) && ui.small_button(format!("Add {entry}")).clicked() {
                        if !sysbypass_text.trim().is_empty() {
                            sysbypass_text.push_str("; ");
                        }
                        sysbypass_text.push_str(&entry);
                    }
                }
            }
            ui.label(RichText::new("Hosts Windows sends directly while the system proxy points at Ferrotap (; separated, <local> = names without a dot, *.corp.com). Applies the next time the system proxy is turned on. Add your VPN's internal hosts here if they stop working.").small().color(gray));
            ui.horizontal(|ui| {
                ui.label("Browser profile folder");
                ui.add(egui::TextEdit::singleline(&mut d.browser_data_dir).hint_text("empty = a fresh temporary profile each time").desired_width(300.0));
            });
            ui.horizontal(|ui| {
                ui.label("TLS key log file");
                ui.add(egui::TextEdit::singleline(&mut d.keylog).hint_text("empty = off; e.g. C:\\captures\\keys.log").desired_width(280.0));
            })
            .response
            .on_hover_text("Ferrotap's upstream TLS secrets and the clean browser's are appended here, so packet captures (Packets window, Wireshark) can be decrypted. Anyone with this file can read those connections.");
            ui.checkbox(&mut d.strip_h3, "Keep clients on the proxy: remove HTTP/3 offers (Alt-Svc)")
                .on_hover_text("Browsers that see an HTTP/3 offer may switch to QUIC (UDP), which goes around the proxy.");
            ui.checkbox(&mut d.streaming, "Streaming mode: pass response bodies on as they arrive")
                .on_hover_text("Server-Sent Events and gRPC always stream. A streamed session's body fills in live; response rules, scripts and throttling need the whole body, so those responses are still buffered.");
            ui.separator();
            ui.label(RichText::new("Server certificates").strong());
            ui.checkbox(&mut d.ignore_cert_errors, "Ignore server certificate errors (unsafe)");
            ui.label(
                RichText::new("Invalid, expired or self-signed server certificates are accepted; each affected session gets a \"Certificate error ignored\" note.")
                    .small()
                    .color(gray),
            );
            ui.separator();
            ui.label(RichText::new("Host remapping").strong());
            ui.horizontal(|ui| {
                ui.checkbox(&mut d.host_map_enabled, "Enabled");
                if ui.small_button("Add").clicked() {
                    d.host_map.push(HostMapEntry { enabled: true, ..Default::default() });
                }
                if ui.small_button("Import from the hosts file").on_hover_text(os_hosts_path().display().to_string()).clicked() {
                    import_hosts = true;
                }
            });
            ui.label(
                RichText::new("Connect to another IP or host for a host name (TLS still uses the original name). *.example.com matches subdomains.")
                    .small()
                    .color(gray),
            );
            let mut del = None;
            egui::Grid::new("conn_hostmap").striped(true).show(ui, |ui| {
                for (i, e) in d.host_map.iter_mut().enumerate() {
                    ui.checkbox(&mut e.enabled, "");
                    ui.add(egui::TextEdit::singleline(&mut e.host).hint_text("host").desired_width(200.0));
                    ui.label("->");
                    ui.add(egui::TextEdit::singleline(&mut e.target).hint_text("IP or host").desired_width(160.0));
                    if ui.small_button("x").clicked() {
                        del = Some(i);
                    }
                    ui.end_row();
                }
            });
            if let Some(i) = del {
                d.host_map.remove(i);
            }
            ui.separator();
            ui.label(RichText::new("Upstream gateway").strong());
            ui.horizontal(|ui| {
                ui.radio_value(&mut d.upstream, UpstreamMode::None, "No proxy");
                ui.radio_value(&mut d.upstream, UpstreamMode::System, "Windows proxy");
                ui.radio_value(&mut d.upstream, UpstreamMode::Manual, "Manual");
            });
            match d.upstream {
                UpstreamMode::System => {
                    let t = match system_proxy_setting() {
                        Some((s, _)) => match parse_system_proxy(&s) {
                            Some(p) if is_ourselves(&p, d.port) => "Windows points at Ferrotap itself (system proxy on): connecting directly.".to_string(),
                            Some(p) => format!("Windows proxy: {}:{}", p.host, p.port),
                            None => format!("Windows proxy \"{s}\" could not be read: connecting directly."),
                        },
                        None => "No Windows proxy is set: connecting directly.".to_string(),
                    };
                    ui.label(RichText::new(t).small().color(gray));
                }
                UpstreamMode::Manual => {
                    ui.horizontal(|ui| {
                        ui.label("Proxy");
                        ui.add(egui::TextEdit::singleline(&mut d.upstream_manual).hint_text("host:port or user:pass@host:port").desired_width(260.0));
                    });
                    if !d.upstream_manual.trim().is_empty() && parse_proxy_addr(&d.upstream_manual).is_none() {
                        ui.colored_label(Color32::from_rgb(230, 90, 90), "Not a valid host:port");
                    }
                }
                UpstreamMode::None => {}
            }
            if d.upstream != UpstreamMode::None {
                ui.horizontal(|ui| {
                    ui.label("Bypass");
                    ui.add(egui::TextEdit::singleline(bypass_text).desired_width(360.0));
                });
                ui.label(
                    RichText::new("Comma separated; <local> = names without a dot. HTTP and HTTPS both go through the gateway with CONNECT.")
                        .small()
                        .color(gray),
                );
            }
        });
        if import_hosts {
            match std::fs::read_to_string(os_hosts_path()) {
                Ok(t) => {
                    let mut n = 0;
                    for e in parse_hosts_file(&t) {
                        if !d.host_map.iter().any(|x| x.host.eq_ignore_ascii_case(&e.host)) {
                            d.host_map.push(e);
                            n += 1;
                        }
                    }
                    *msg = format!("Imported {n} entries from the hosts file (not saved yet)");
                }
                Err(e) => *msg = format!("Cannot read the hosts file: {e}"),
            }
        }
        if save {
            d.upstream_bypass = bypass_text.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect();
            d.capture_processes = procs_text.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect();
            d.capture_ports = ports_text.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            d.system_bypass = sysbypass_text.split(';').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect();
            d.host_map.retain(|e| !e.host.trim().is_empty());
            let restart = d.port != active_port() || d.allow_remote != current().allow_remote;
            set_current(d.clone());
            *msg = match d.save(&ConnSettings::default_path()) {
                Ok(()) if restart => "Saved. Restart Ferrotap to listen on the new port / interfaces.".into(),
                Ok(()) => "Saved and applied to new connections.".into(),
                Err(e) => format!("Save failed: {e}"),
            };
        }
        self.ext.conn.show = open;
    }
}
