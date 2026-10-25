//! G13 HTTP/3 fallback: Alt-Svc h3 offers are removed by the proxy; the Protocols report.
use std::sync::mpsc;
use std::time::{Duration, Instant};

use ferrotap::http3::*;
use ferrotap::model::{SessionEvent, SessionRecord};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[test]
fn alt_svc_rewrite() {
    assert_eq!(strip_h3("h3=\":443\"; ma=86400, h3-29=\":443\"; ma=86400"), None);
    assert_eq!(strip_h3("h3=\":443\"; ma=86400, h2=\"alt.example:443\"; ma=60").as_deref(), Some("h2=\"alt.example:443\"; ma=60"));
    assert_eq!(strip_h3("clear").as_deref(), Some("clear"));
    assert_eq!(strip_h3("quic=\":443\"; v=\"46\"").as_deref(), None);
    assert!(offers_h3("h3=\":443\""));
    assert!(!offers_h3("h2=\":443\""));
    assert!(!offers_h3("clear"));

    let mk = |host: &str, c: &str, s: &str| SessionRecord { method: "GET".into(), url: format!("https://{host}/"), http_version: c.into(), response_http_version: s.into(), ..Default::default() };
    let rows = [mk("a.test", "HTTP/2.0", "HTTP/1.1"), mk("a.test", "HTTP/2.0", "HTTP/1.1"), mk("b.test", "HTTP/2.0", "HTTP/2.0")];
    let refs: Vec<&SessionRecord> = rows.iter().collect();
    let r = protocol_report(&refs);
    assert_eq!(r.len(), 2);
    assert!(r[0].fallback && r[0].sessions == 2);
    assert!(!r[1].fallback);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn proxy_removes_h3_offers() {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else { return };
            tokio::spawn(async move {
                let mut b = [0u8; 2048];
                let _ = s.read(&mut b).await;
                let _ = s.write_all(b"HTTP/1.1 200 OK\r\nAlt-Svc: h3=\":443\"; ma=86400, h2=\":443\"\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").await;
            });
        }
    });
    let proxy_port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel::<SessionEvent>();
    let proxy = tokio::spawn(ferrotap::proxy::run(proxy_port, Some(tx)));
    let t0 = Instant::now();
    while TcpStream::connect(("127.0.0.1", proxy_port)).await.is_err() {
        assert!(t0.elapsed() < Duration::from_secs(20));
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let mut s = TcpStream::connect(("127.0.0.1", proxy_port)).await.unwrap();
    s.write_all(format!("GET http://127.0.0.1:{up}/x HTTP/1.1\r\nHost: 127.0.0.1:{up}\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), s.read_to_end(&mut out)).await;
    let text = String::from_utf8_lossy(&out).to_ascii_lowercase();
    assert!(text.contains("alt-svc: h2=\":443\"") && !text.contains("h3="), "{text}");
    let t = Instant::now();
    loop {
        if let Ok(SessionEvent::New(s)) = rx.recv_timeout(Duration::from_millis(200)) {
            if s.url.ends_with("/x") {
                assert!(s.comment.contains("HTTP/3 offer removed"), "{}", s.comment);
                assert!(!s.comment.contains("Rules:"), "{}", s.comment);
                break;
            }
        }
        assert!(t.elapsed() < Duration::from_secs(10));
    }
    proxy.abort();
}
