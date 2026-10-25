//! The sessions.json debugging dump is opt-in (FERROTAP_SESSIONS_DUMP); ids stay sequential either way.
use std::sync::mpsc;
use std::time::{Duration, Instant};

use ferrotap::model::SessionEvent;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

async fn origin() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else { return };
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf).await;
                let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").await;
            });
        }
    });
    port
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

async fn get(proxy: u16, up: u16, path: &str) {
    let mut s = TcpStream::connect(("127.0.0.1", proxy)).await.unwrap();
    s.write_all(format!("GET http://127.0.0.1:{up}{path} HTTP/1.1\r\nHost: 127.0.0.1:{up}\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
    let mut v = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), s.read_to_end(&mut v)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dump_only_when_asked() {
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("dump.json");
    assert_eq!(ferrotap::proxy::sessions_dump_path(), None, "off by default");
    std::env::set_var("FERROTAP_SESSIONS_DUMP", &dump);
    assert_eq!(ferrotap::proxy::sessions_dump_path(), Some(dump.clone()));
    let up = origin().await;
    let port = free_port();
    let (tx, rx) = mpsc::channel::<SessionEvent>();
    let proxy = tokio::spawn(ferrotap::proxy::run(port, Some(tx)));
    let t0 = Instant::now();
    while TcpStream::connect(("127.0.0.1", port)).await.is_err() {
        assert!(t0.elapsed() < Duration::from_secs(20), "proxy did not start");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    for p in ["/a", "/b", "/c"] {
        get(port, up, p).await;
    }
    let mut ids = Vec::new();
    while ids.len() < 3 {
        match rx.recv_timeout(Duration::from_secs(10)).expect("session event") {
            SessionEvent::New(s) => ids.push(s.id),
            _ => {}
        }
    }
    assert_eq!(ids, vec![1, 2, 3]);
    let t = Instant::now();
    let list: Vec<serde_json::Value> = loop {
        if let Some(v) = std::fs::read_to_string(&dump).ok().and_then(|s| serde_json::from_str::<Vec<serde_json::Value>>(&s).ok()).filter(|v| v.len() == 3) {
            break v;
        }
        assert!(t.elapsed() < Duration::from_secs(10), "dump not written");
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    assert!(list.iter().any(|s| s["url"].as_str().unwrap_or("").ends_with("/b")));
    proxy.abort();
    std::env::remove_var("FERROTAP_SESSIONS_DUMP");
}
