//! End to end through the real proxy: failed upstream connections are recorded (error dashboard),
//! repeated Set-Cookie headers are all kept, and a gzip body keeps its exact bytes.
use std::io::Write as _;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use ferrotap::model::{SessionEvent, SessionRecord};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn gzip(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// Upstream answering every request with two Set-Cookie headers and a gzip JSON body.
async fn start_upstream() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else { return };
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 2048];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut tmp).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&tmp[..n]),
                    }
                }
                if buf.windows(9).any(|w| w == b"/trailers") {
                    let resp = "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nTransfer-Encoding: chunked\r\n\
                                Trailer: grpc-status\r\nConnection: close\r\n\r\n5\r\nhello\r\n0\r\ngrpc-status: 0\r\n\r\n";
                    let _ = s.write_all(resp.as_bytes()).await;
                    return;
                }
                let body = gzip(b"{\"hello\":\"world\"}");
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Encoding: gzip\r\n\
                     Set-Cookie: a=1; Path=/; Expires=Wed, 21 Oct 2037 07:28:00 GMT\r\nSet-Cookie: b=2; HttpOnly\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = s.write_all(head.as_bytes()).await;
                let _ = s.write_all(&body).await;
            });
        }
    });
    port
}

async fn get(proxy: u16, url: &str) -> u16 {
    let mut s = TcpStream::connect(("127.0.0.1", proxy)).await.unwrap();
    let host = url.split("://").nth(1).unwrap().split('/').next().unwrap();
    s.write_all(format!("GET {url} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(15), s.read_to_end(&mut out)).await;
    String::from_utf8_lossy(&out).split_whitespace().nth(1).and_then(|x| x.parse().ok()).unwrap_or(0)
}

fn next_session(rx: &mpsc::Receiver<SessionEvent>, url_part: &str) -> SessionRecord {
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(15) {
        if let Ok(SessionEvent::New(s)) = rx.recv_timeout(Duration::from_millis(200)) {
            if s.url.contains(url_part) {
                return s;
            }
        }
    }
    panic!("no session for {url_part}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn proxy_records_failures_cookies_and_exact_bodies() {
    let up = start_upstream().await;
    let proxy_port = free_port();
    let (tx, rx) = mpsc::channel::<SessionEvent>();
    let proxy = tokio::spawn(ferrotap::proxy::run(proxy_port, Some(tx)));
    let t0 = Instant::now();
    while TcpStream::connect(("127.0.0.1", proxy_port)).await.is_err() {
        assert!(t0.elapsed() < Duration::from_secs(20), "proxy did not start");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // 1. Upstream refuses the connection.
    let dead = free_port();
    assert_eq!(get(proxy_port, &format!("http://127.0.0.1:{dead}/gone")).await, 502);
    let s = next_session(&rx, "/gone");
    assert_eq!(s.status, 502);
    assert!(s.comment.starts_with("Upstream failed"), "{}", s.comment);
    assert!(s.response_header("x-ferrotap-error").is_some());
    let rows = [&s];
    let report = ferrotap::insights::error_report(&rows);
    assert_eq!(report.failed_connections, vec![s.id], "{:?}", s.comment);

    // 2. Repeated Set-Cookie + gzip body.
    assert_eq!(get(proxy_port, &format!("http://127.0.0.1:{up}/ok")).await, 200);
    let s = next_session(&rx, "/ok");
    let cookies = ferrotap::inspect::response_set_cookies(&s.response_headers);
    let names: Vec<_> = cookies.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["a", "b"]);
    assert!(s.response_body_b64.is_some());
    match ferrotap::inspect::decode_body_bytes(&s.response_bytes(), s.response_header("content-encoding"), s.response_header("content-type")) {
        ferrotap::inspect::BodyDecode::Decoded { text, .. } => assert_eq!(text, "{\"hello\":\"world\"}"),
        other => panic!("unexpected {other:?}"),
    }

    // 3. Chunked response with a trailer.
    assert_eq!(get(proxy_port, &format!("http://127.0.0.1:{up}/trailers")).await, 200);
    let s = next_session(&rx, "/trailers");
    assert_eq!(s.response_body, "hello");
    assert_eq!(s.response_trailers, vec![("grpc-status".to_string(), "0".to_string())]);
    proxy.abort();
}
