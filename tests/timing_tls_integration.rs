//! End to end: local TLS server <- Ferrotap proxy (MITM) <- HTTPS client that trusts the proxy CA.
//! Asserts the captured session carries connect/TLS/TTFB timings, TLS version and HTTP version.
use std::path::PathBuf;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use ferrotap::model::{SessionEvent, SessionRecord};
use hudsucker::rustls::{
    self,
    crypto::aws_lc_rs,
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const BODY: &str = "hello from tls server";

async fn start_tls_server() -> (u16, String) {
    let ck = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let pem = ck.cert.pem();
    let der = ck.cert.der().clone();
    let key = PrivateKeyDer::from(PrivatePkcs8KeyDer::from(ck.signing_key.serialize_der()));
    let mut cfg = rustls::ServerConfig::builder_with_provider(Arc::new(aws_lc_rs::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![der], key)
        .unwrap();
    cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(cfg));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((s, _)) = listener.accept().await else { return };
            let acc = acceptor.clone();
            tokio::spawn(async move {
                let Ok(mut t) = acc.accept(s).await else { return };
                let mut got = Vec::new();
                let mut buf = [0u8; 4096];
                loop {
                    let n = t.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    got.extend_from_slice(&buf[..n]);
                    if got.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                // Small delay so TTFB is a real, non-zero measurement.
                tokio::time::sleep(Duration::from_millis(30)).await;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{}",
                    BODY.len(),
                    BODY
                );
                let _ = t.write_all(resp.as_bytes()).await;
                let _ = t.flush().await;
                tokio::time::sleep(Duration::from_millis(500)).await;
                let _ = t.shutdown().await;
            });
        }
    });
    (port, pem)
}

fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn https_session_has_timings_tls_and_http_version() {
    let (server_port, server_pem) = start_tls_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let extra = tmp.path().join("upstream.pem");
    std::fs::write(&extra, server_pem).unwrap();
    std::env::set_var("FERROTAP_EXTRA_CA", &extra);
    let keylog = tmp.path().join("keys.log");
    ferrotap::tlsdecrypt::set_keylog_path(Some(keylog.clone()));

    let proxy_port = free_port();
    let (tx, rx) = mpsc::channel::<SessionEvent>();
    let proxy = tokio::spawn(ferrotap::proxy::run(proxy_port, Some(tx)));

    // Wait for the proxy to listen.
    let t0 = Instant::now();
    loop {
        if TcpStream::connect(("127.0.0.1", proxy_port)).await.is_ok() {
            break;
        }
        assert!(t0.elapsed() < Duration::from_secs(20), "proxy did not start");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Trust the proxy's own CA.
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
    let connector = tokio_rustls::TlsConnector::from(Arc::new(ccfg));

    // CONNECT tunnel through the proxy, then TLS to the (intercepting) proxy.
    let mut s = TcpStream::connect(("127.0.0.1", proxy_port)).await.unwrap();
    let req = format!("CONNECT localhost:{server_port} HTTP/1.1\r\nHost: localhost:{server_port}\r\n\r\n");
    s.write_all(req.as_bytes()).await.unwrap();
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        let n = tokio::time::timeout(Duration::from_secs(10), s.read(&mut b)).await.expect("CONNECT reply timeout").unwrap();
        assert!(n > 0, "proxy closed during CONNECT");
        head.push(b[0]);
    }
    assert!(String::from_utf8_lossy(&head).contains(" 200"), "CONNECT reply: {}", String::from_utf8_lossy(&head));
    let mut tls = connector.connect(ServerName::try_from("localhost").unwrap(), s).await.expect("TLS to proxy");
    tls.write_all(format!("GET /hello HTTP/1.1\r\nHost: localhost:{server_port}\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
    let mut resp = Vec::new();
    let t1 = Instant::now();
    while !String::from_utf8_lossy(&resp).contains(BODY) {
        assert!(t1.elapsed() < Duration::from_secs(15), "no response; got: {}", String::from_utf8_lossy(&resp));
        let n = tokio::time::timeout(Duration::from_secs(10), tls.read(&mut b)).await.expect("read timeout").unwrap_or(0);
        if n == 0 {
            break;
        }
        resp.push(b[0]);
    }
    assert!(String::from_utf8_lossy(&resp).contains(BODY), "response: {}", String::from_utf8_lossy(&resp));

    // Find the captured session.
    let mut found: Option<SessionRecord> = None;
    let t2 = Instant::now();
    while found.is_none() && t2.elapsed() < Duration::from_secs(10) {
        while let Ok(ev) = rx.try_recv() {
            if let SessionEvent::New(s) = ev {
                if s.url.ends_with("/hello") {
                    found = Some(s);
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // the proxy wrote its upstream TLS secrets for packet decryption
    let kl = std::fs::read_to_string(&keylog).unwrap_or_default();
    assert!(kl.contains("CLIENT_TRAFFIC_SECRET_0") || kl.contains("CLIENT_RANDOM"), "key log: {kl}");
    ferrotap::tlsdecrypt::set_keylog_path(None);
    proxy.abort();
    let s = found.expect("no session captured for /hello");
    println!("session: {} {} status={} http={} resp_http={} connect={:?} tls={:?} ttfb={:?} tls_version={:?}",
        s.method, s.url, s.status, s.http_version, s.response_http_version, s.connect_ms, s.tls_ms, s.ttfb_ms, s.tls_version);

    assert_eq!(s.status, 200);
    assert!(s.connect_ms.is_some(), "connect_ms not set");
    assert!(s.dns_ms.is_some(), "dns_ms not set (measured by the connector)");
    assert!(s.send_ms.is_some(), "request-sent phase recorded");
    assert_eq!(ferrotap::inspect::timing_phases(&s)[0].0, "Request sent");
    assert!(s.tls_ms.is_some(), "tls_ms not set");
    assert!(s.ttfb_ms.is_some(), "ttfb_ms not set");
    assert!(s.ttfb_ms.unwrap() >= 20, "ttfb should include the server's 30 ms delay, got {:?}", s.ttfb_ms);
    assert_eq!(s.tls_version.as_deref(), Some("TLS 1.3"));
    assert_eq!(s.response_http_version, "HTTP/1.1");
    assert!(!s.http_version.is_empty());
}
