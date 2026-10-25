//! G8 end to end through the real proxy: host remapping, upstream gateway (CONNECT), the
//! http://ferrotap.ca/ device page, and "ignore server certificate errors".
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use ferrotap::conn::{self, ConnSettings, HostMapEntry, UpstreamMode};
use ferrotap::model::{SessionEvent, SessionRecord};
use hudsucker::rustls::{
    self,
    crypto::aws_lc_rs,
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

async fn read_head(s: &mut TcpStream) -> String {
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        let n = tokio::time::timeout(Duration::from_secs(10), s.read(&mut b)).await.expect("head timeout").unwrap_or(0);
        if n == 0 {
            break;
        }
        head.push(b[0]);
    }
    String::from_utf8_lossy(&head).into_owned()
}

/// Plain HTTP upstream answering "UP <path>".
async fn start_upstream() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else { return };
            tokio::spawn(async move {
                let head = read_head(&mut s).await;
                let path = head.split_whitespace().nth(1).unwrap_or("/").to_string();
                let body = format!("UP {path}");
                let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await;
            });
        }
    });
    port
}

/// Minimal CONNECT-only gateway; records each CONNECT target.
async fn start_gateway(seen: Arc<Mutex<Vec<String>>>) -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut c, _)) = l.accept().await else { return };
            let seen = seen.clone();
            tokio::spawn(async move {
                let head = read_head(&mut c).await;
                let target = head.split_whitespace().nth(1).unwrap_or("").to_string();
                seen.lock().unwrap().push(target.clone());
                let Ok(mut up) = TcpStream::connect(&target).await else {
                    let _ = c.write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n").await;
                    return;
                };
                let _ = c.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n").await;
                let _ = tokio::io::copy_bidirectional(&mut c, &mut up).await;
            });
        }
    });
    port
}

async fn start_tls_server() -> u16 {
    let ck = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let key = PrivateKeyDer::from(PrivatePkcs8KeyDer::from(ck.signing_key.serialize_der()));
    let mut cfg = rustls::ServerConfig::builder_with_provider(Arc::new(aws_lc_rs::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![ck.cert.der().clone()], key)
        .unwrap();
    cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(cfg));
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((s, _)) = l.accept().await else { return };
            let acc = acceptor.clone();
            tokio::spawn(async move {
                let Ok(mut t) = acc.accept(s).await else { return };
                let mut got = Vec::new();
                let mut buf = [0u8; 2048];
                while !got.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = t.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    got.extend_from_slice(&buf[..n]);
                }
                let _ = t.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\nsecret").await;
                let _ = t.shutdown().await;
            });
        }
    });
    port
}

async fn get(proxy: u16, url: &str) -> (u16, String) {
    let mut s = TcpStream::connect(("127.0.0.1", proxy)).await.unwrap();
    let host = url.split("://").nth(1).unwrap().split('/').next().unwrap();
    s.write_all(format!("GET {url} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(20), s.read_to_end(&mut out)).await;
    let t = String::from_utf8_lossy(&out).into_owned();
    (t.split_whitespace().nth(1).and_then(|x| x.parse().ok()).unwrap_or(0), t)
}

/// HTTPS GET through the proxy (CONNECT + TLS to the intercepting proxy, which we trust).
async fn https_get(proxy: u16, host: &str, port: u16, path: &str) -> (u16, String) {
    let ca_pem = std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ca").join("ferrotap-ca.pem")).unwrap();
    let mut roots = rustls::RootCertStore::empty();
    for der in ferrotap::timing::pem_certs(&ca_pem) {
        roots.add(CertificateDer::from(der)).unwrap();
    }
    let ccfg = rustls::ClientConfig::builder_with_provider(Arc::new(aws_lc_rs::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let mut s = TcpStream::connect(("127.0.0.1", proxy)).await.unwrap();
    s.write_all(format!("CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\n\r\n").as_bytes()).await.unwrap();
    let head = read_head(&mut s).await;
    assert!(head.contains(" 200"), "{head}");
    let mut tls = tokio_rustls::TlsConnector::from(Arc::new(ccfg)).connect(ServerName::try_from(host.to_string()).unwrap(), s).await.expect("TLS to proxy");
    tls.write_all(format!("GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(20), tls.read_to_end(&mut out)).await;
    let t = String::from_utf8_lossy(&out).into_owned();
    (t.split_whitespace().nth(1).and_then(|x| x.parse().ok()).unwrap_or(0), t)
}

fn session(rx: &mpsc::Receiver<SessionEvent>, part: &str) -> SessionRecord {
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(15) {
        if let Ok(SessionEvent::New(s)) = rx.recv_timeout(Duration::from_millis(200)) {
            if s.url.contains(part) {
                return s;
            }
        }
    }
    panic!("no session for {part}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn connections_through_the_proxy() {
    let tmp = tempfile::tempdir().unwrap();
    std::env::set_var("FERROTAP_CONN_SETTINGS", tmp.path().join("conn.json"));
    let up = start_upstream().await;
    let proxy_port = free_port();
    let (tx, rx) = mpsc::channel::<SessionEvent>();
    let proxy = tokio::spawn(ferrotap::proxy::run(proxy_port, Some(tx)));
    let t0 = Instant::now();
    while TcpStream::connect(("127.0.0.1", proxy_port)).await.is_err() {
        assert!(t0.elapsed() < Duration::from_secs(20), "proxy did not start");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(conn::active_port(), proxy_port);

    // 1. host remapping: a made-up name reaches the local upstream
    let mut s = ConnSettings::default();
    s.host_map = vec![HostMapEntry { enabled: true, host: "*.remap.test".into(), target: "127.0.0.1".into() }];
    conn::set_current(s.clone());
    let (st, body) = get(proxy_port, &format!("http://api.remap.test:{up}/one")).await;
    assert_eq!(st, 200, "{body}");
    assert!(body.ends_with("UP /one"), "{body}");

    // 2. upstream gateway: the connection goes through CONNECT to the (remapped) target
    let seen = Arc::new(Mutex::new(Vec::new()));
    let gw = start_gateway(seen.clone()).await;
    s.upstream = UpstreamMode::Manual;
    s.upstream_manual = format!("127.0.0.1:{gw}");
    s.upstream_bypass.clear();
    conn::set_current(s.clone());
    let (st, body) = get(proxy_port, &format!("http://gw.remap.test:{up}/two")).await;
    assert_eq!(st, 200, "{body}");
    assert!(body.ends_with("UP /two"));
    assert_eq!(seen.lock().unwrap().clone(), vec![format!("127.0.0.1:{up}")]);
    // a dead gateway is a recorded failure
    s.upstream_manual = format!("127.0.0.1:{}", free_port());
    conn::set_current(s.clone());
    let (st, _) = get(proxy_port, &format!("http://dead.remap.test:{up}/three")).await;
    assert_eq!(st, 502);
    let three = session(&rx, "/three");
    assert!(three.comment.contains("upstream proxy"));
    // every session knows its local client process (native lookup)
    assert!(three.process_name.as_deref().unwrap_or("").starts_with("conn_proxy"), "{:?}", three.process_name);
    assert_eq!(three.pid, Some(std::process::id()));
    assert_eq!(three.client_ip.as_deref(), Some("127.0.0.1"));
    s.upstream = UpstreamMode::None;
    conn::set_current(s.clone());

    // capture filter: our own process excluded -> proxied but not recorded; then back to everything
    s.capture_mode = conn::CaptureMode::Except;
    s.capture_processes = vec![std::env::current_exe().unwrap().file_name().unwrap().to_string_lossy().into_owned()];
    conn::set_current(s.clone());
    let (st, _) = get(proxy_port, &format!("http://api.remap.test:{up}/hidden")).await;
    assert_eq!(st, 200);
    s.capture_mode = conn::CaptureMode::All;
    conn::set_current(s.clone());
    let (st, _) = get(proxy_port, &format!("http://api.remap.test:{up}/shown")).await;
    assert_eq!(st, 200);
    loop {
        match rx.recv_timeout(Duration::from_secs(10)) {
            Ok(SessionEvent::New(x)) if x.url.contains("/hidden") => panic!("filtered session was recorded"),
            Ok(SessionEvent::New(x)) if x.url.contains("/shown") => break,
            Ok(_) => {}
            Err(e) => panic!("no /shown session: {e}"),
        }
    }

    // 3. device setup page
    let (st, body) = get(proxy_port, "http://ferrotap.ca/").await;
    assert_eq!(st, 200);
    assert!(body.contains("Ferrotap is reachable"));
    let (st, body) = get(proxy_port, "http://ferrotap.ca/ferrotap-ca.crt").await;
    assert_eq!(st, 200);
    assert!(body.contains("-----BEGIN CERTIFICATE-----"));
    assert_eq!(session(&rx, "ferrotap.ca/ferrotap-ca.crt").comment, "Ferrotap device setup page");

    // 4. untrusted server certificate: refused, then allowed and noted when ignoring errors
    let tls_port = start_tls_server().await;
    let (st, _) = https_get(proxy_port, "localhost", tls_port, "/strict").await;
    assert_eq!(st, 502);
    let failed = session(&rx, "/strict");
    assert!(failed.comment.to_lowercase().contains("certificate"), "{}", failed.comment);
    s.ignore_cert_errors = true;
    conn::set_current(s.clone());
    let (st, body) = https_get(proxy_port, "localhost", tls_port, "/lenient").await;
    assert_eq!(st, 200, "{body}");
    assert!(body.ends_with("secret"));
    let ok = session(&rx, "/lenient");
    assert!(ok.comment.contains("Certificate error ignored"), "{}", ok.comment);

    // Replay / Composer can send https:// URLs through the proxy too
    let r = tokio::task::spawn_blocking(move || {
        let req = ferrotap::model::ComposerRequest { method: "GET".into(), url: format!("https://localhost:{tls_port}/composed"), headers: vec![], body: String::new() };
        ferrotap::model::composer_send(&req, &format!("127.0.0.1:{proxy_port}"))
    }).await.unwrap();
    assert_eq!((r.status, r.body.as_str(), r.error.as_str()), (200, "secret", ""));
    assert!(session(&rx, "/composed").url.starts_with("https://"));
    proxy.abort();
}
