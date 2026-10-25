//! Streaming: Server-Sent Events reach the client while the server is still sending, and the
//! session's body fills in (Update events) until it completes.
use std::sync::mpsc;
use std::time::{Duration, Instant};

use ferrotap::model::{SessionEvent, SessionModel};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// SSE server: three events 400 ms apart (chunked), then the end.
async fn start_sse() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else { return };
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf).await;
                let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").await;
                for i in 1..=3 {
                    let ev = format!("data: tick {i}\n\n");
                    let _ = s.write_all(format!("{:x}\r\n{ev}\r\n", ev.len()).as_bytes()).await;
                    let _ = s.flush().await;
                    tokio::time::sleep(Duration::from_millis(400)).await;
                }
                let _ = s.write_all(b"0\r\n\r\n").await;
            });
        }
    });
    port
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sse_streams_through_the_proxy() {
    let up = start_sse().await;
    let proxy_port = free_port();
    let (tx, rx) = mpsc::channel::<SessionEvent>();
    let proxy = tokio::spawn(ferrotap::proxy::run(proxy_port, Some(tx)));
    let t0 = Instant::now();
    while TcpStream::connect(("127.0.0.1", proxy_port)).await.is_err() {
        assert!(t0.elapsed() < Duration::from_secs(20), "proxy did not start");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let mut s = TcpStream::connect(("127.0.0.1", proxy_port)).await.unwrap();
    s.write_all(format!("GET http://127.0.0.1:{up}/events HTTP/1.1\r\nHost: 127.0.0.1:{up}\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
    // The first event arrives long before the stream ends (~1.2 s).
    let start = Instant::now();
    let mut got = Vec::new();
    let mut tmp = [0u8; 4096];
    while !String::from_utf8_lossy(&got).contains("tick 1") {
        let n = tokio::time::timeout(Duration::from_secs(5), s.read(&mut tmp)).await.expect("first event in time").unwrap();
        assert!(n > 0, "closed early");
        got.extend_from_slice(&tmp[..n]);
    }
    let first = start.elapsed();
    assert!(first < Duration::from_millis(900), "first event took {first:?}: the body was buffered");
    let _ = tokio::time::timeout(Duration::from_secs(10), s.read_to_end(&mut got)).await;
    let text = String::from_utf8_lossy(&got);
    assert!(text.contains("tick 3"), "{text}");

    // Grid view: New (empty, streaming...) then Updates until complete.
    let mut m = SessionModel::default();
    let t = Instant::now();
    let mut updates = 0;
    loop {
        match rx.recv_timeout(Duration::from_millis(300)) {
            Ok(ev) => {
                if matches!(ev, SessionEvent::Update(_)) {
                    updates += 1;
                }
                m.apply(ev);
            }
            Err(_) if m.rows.iter().any(|r| r.comment.starts_with("streamed")) => break,
            Err(_) => {}
        }
        assert!(t.elapsed() < Duration::from_secs(10), "no final update: {:?}", m.rows.iter().map(|r| &r.comment).collect::<Vec<_>>());
    }
    let r = m.rows.iter().find(|r| r.url.ends_with("/events")).unwrap();
    assert_eq!(m.rows.len(), 1, "updates replace the row");
    assert!(updates >= 2, "partial updates while streaming: {updates}");
    assert_eq!(r.response_body, "data: tick 1\n\ndata: tick 2\n\ndata: tick 3\n\n");
    assert!(r.duration_ms >= 1000);
    proxy.abort();
}
