//! Malformed and odd requests must not take the proxy down: after each one, a normal request still
//! goes through (and the odd ones that are valid HTTP are recorded).
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
                let mut buf = vec![0u8; 256 * 1024];
                let _ = tokio::time::timeout(Duration::from_secs(2), s.read(&mut buf)).await;
                let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nX-Odd: \xff\xfe\r\nSet-Cookie: a=\xe9\r\nConnection: close\r\n\r\nok").await;
            });
        }
    });
    port
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// Send raw bytes, read whatever comes back (bounded), then close.
async fn raw(proxy: u16, bytes: &[u8], close_early: bool) -> Vec<u8> {
    let Ok(mut s) = TcpStream::connect(("127.0.0.1", proxy)).await else { return Vec::new() };
    let _ = s.write_all(bytes).await;
    if close_early {
        return Vec::new();
    }
    let mut v = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(3), s.read_to_end(&mut v)).await;
    v
}

async fn normal_get_works(proxy: u16, up: u16) -> bool {
    let r = raw(proxy, format!("GET http://127.0.0.1:{up}/ok HTTP/1.1\r\nHost: 127.0.0.1:{up}\r\nConnection: close\r\n\r\n").as_bytes(), false).await;
    String::from_utf8_lossy(&r).starts_with("HTTP/1.1 200")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn proxy_survives_malformed_requests() {
    let up = origin().await;
    let port = free_port();
    let (tx, rx) = mpsc::channel::<SessionEvent>();
    let proxy = tokio::spawn(ferrotap::proxy::run(port, Some(tx)));
    let t0 = Instant::now();
    while TcpStream::connect(("127.0.0.1", port)).await.is_err() {
        assert!(t0.elapsed() < Duration::from_secs(20), "proxy did not start");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(normal_get_works(port, up).await);
    let h = format!("127.0.0.1:{up}");
    let long_url = format!("GET http://{h}/{} HTTP/1.1\r\nHost: {h}\r\n\r\n", "a".repeat(100_000));
    let cases: Vec<(&str, Vec<u8>, bool)> = vec![
        ("garbage", b"\x00\x01\x02\xff\xfe garbage\r\n\r\n".to_vec(), false),
        ("http/0.9", b"GET /\r\n".to_vec(), false),
        ("no host", b"GET / HTTP/1.1\r\n\r\n".to_vec(), false),
        ("obs-text header", format!("GET http://{h}/obs HTTP/1.1\r\nHost: {h}\r\nX-Bin: ").into_bytes().into_iter().chain(b"\xff\xfe\x80\r\nConnection: close\r\n\r\n".iter().copied()).collect(), false),
        ("CL and TE", format!("POST http://{h}/clte HTTP/1.1\r\nHost: {h}\r\nContent-Length: 5\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n0\r\n\r\n").into_bytes(), false),
        ("negative CL", format!("POST http://{h}/neg HTTP/1.1\r\nHost: {h}\r\nContent-Length: -1\r\n\r\n").into_bytes(), false),
        ("two CLs", format!("POST http://{h}/twocl HTTP/1.1\r\nHost: {h}\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\nab").into_bytes(), false),
        ("huge CL then close", format!("POST http://{h}/big HTTP/1.1\r\nHost: {h}\r\nContent-Length: 999999999\r\n\r\npartial").into_bytes(), true),
        ("bad chunk", format!("POST http://{h}/chunk HTTP/1.1\r\nHost: {h}\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\nxx\r\n").into_bytes(), false),
        ("long url", long_url.into_bytes(), false),
        ("many headers", format!("GET http://{h}/many HTTP/1.1\r\nHost: {h}\r\n{}Connection: close\r\n\r\n", (0..500).map(|i| format!("X-H{i}: {i}\r\n")).collect::<String>()).into_bytes(), false),
        ("userinfo", format!("GET http://user:pa%20ss@{h}/ui HTTP/1.1\r\nHost: {h}\r\nConnection: close\r\n\r\n").into_bytes(), false),
        ("ipv6 host", b"GET http://[::1]:9/v6 HTTP/1.1\r\nHost: [::1]:9\r\nConnection: close\r\n\r\n".to_vec(), false),
        ("bad host", b"GET http://exa mple.test/ HTTP/1.1\r\nHost: exa mple.test\r\n\r\n".to_vec(), false),
        ("connect garbage", b"CONNECT :::: HTTP/1.1\r\nHost: ::::\r\n\r\n".to_vec(), false),
        ("connect then junk", b"CONNECT 127.0.0.1:9 HTTP/1.1\r\nHost: 127.0.0.1:9\r\n\r\n\x16\x03\x01\x00\x05hello".to_vec(), false),
        ("connect then close", format!("CONNECT {h} HTTP/1.1\r\nHost: {h}\r\n\r\n").into_bytes(), true),
        ("pipelined", format!("GET http://{h}/p1 HTTP/1.1\r\nHost: {h}\r\n\r\nGET http://{h}/p2 HTTP/1.1\r\nHost: {h}\r\nConnection: close\r\n\r\n").into_bytes(), false),
        ("upgrade to nowhere", format!("GET http://{h}/ws HTTP/1.1\r\nHost: {h}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: x\r\nSec-WebSocket-Version: 13\r\n\r\n").into_bytes(), false),
        ("unknown method", format!("BREW http://{h}/pot HTTP/1.1\r\nHost: {h}\r\nConnection: close\r\n\r\n").into_bytes(), false),
        ("half request", format!("GET http://{h}/half HTTP/1.1\r\nHost: {h}\r\nX-A: ").into_bytes(), true),
    ];
    for (name, bytes, early) in cases {
        let _ = raw(port, &bytes, early).await;
        assert!(normal_get_works(port, up).await, "the proxy stopped working after: {name}");
    }
    // the odd-but-valid ones were recorded (odd bytes in the response headers too)
    let mut urls = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        if let SessionEvent::New(s) | SessionEvent::Update(s) = ev {
            urls.push(s.url);
        }
    }
    for want in ["/obs", "/many", "/ui", "/pot", "/p1"] {
        assert!(urls.iter().any(|u| u.ends_with(want)), "{want} not recorded: {urls:?}");
    }
    proxy.abort();
}
