//! Large downloads pass straight through: the client gets every byte (and the Content-Length,
//! when there is one), while the session keeps only the first `LARGE_BODY_KEEP` bytes. A chunked
//! response is read ahead until it passes `LARGE_BODY`, then streamed.
use std::sync::mpsc;
use std::time::{Duration, Instant};

use ferrotap::model::{SessionEvent, SessionModel};
use ferrotap::proxy::{LARGE_BODY, LARGE_BODY_KEEP};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const SIZE: usize = LARGE_BODY as usize + 4 * 1024 * 1024;

fn byte(i: usize) -> u8 {
    (i % 251) as u8
}

/// Serves SIZE pattern bytes; paths containing "/chunked" use chunked framing.
async fn start_server() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else { return };
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let n = s.read(&mut buf).await.unwrap_or(0);
                let chunked = String::from_utf8_lossy(&buf[..n]).contains("/chunked");
                let framing = if chunked { "Transfer-Encoding: chunked".to_string() } else { format!("Content-Length: {SIZE}") };
                let head = format!("HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n{framing}\r\nConnection: close\r\n\r\n");
                let _ = s.write_all(head.as_bytes()).await;
                if buf.starts_with(b"HEAD ") {
                    let _ = s.flush().await;
                    return;
                }
                let mut sent = 0;
                while sent < SIZE {
                    let n = (64 * 1024).min(SIZE - sent);
                    let mut part: Vec<u8> = (sent..sent + n).map(byte).collect();
                    if chunked {
                        let mut c = format!("{n:x}\r\n").into_bytes();
                        c.append(&mut part);
                        c.extend_from_slice(b"\r\n");
                        part = c;
                    }
                    if s.write_all(&part).await.is_err() {
                        return;
                    }
                    sent += n;
                }
                if chunked {
                    let _ = s.write_all(b"0\r\n\r\n").await;
                }
                let _ = s.flush().await;
            });
        }
    });
    port
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// Undo chunked framing.
fn dechunk(mut b: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let eol = b.windows(2).position(|w| w == b"\r\n").unwrap();
        let n = usize::from_str_radix(std::str::from_utf8(&b[..eol]).unwrap().trim(), 16).unwrap();
        b = &b[eol + 2..];
        if n == 0 {
            return out;
        }
        out.extend_from_slice(&b[..n]);
        b = &b[n + 2..];
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn large_download_streams_and_session_keeps_a_prefix() {
    check("/big.bin", true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn large_chunked_download_switches_to_streaming() {
    check("/chunked.bin", false).await;
}

async fn check(path: &str, with_length: bool) {
    let up = start_server().await;
    let proxy_port = free_port();
    let (tx, rx) = mpsc::channel::<SessionEvent>();
    let proxy = tokio::spawn(ferrotap::proxy::run(proxy_port, Some(tx)));
    let t0 = Instant::now();
    while TcpStream::connect(("127.0.0.1", proxy_port)).await.is_err() {
        assert!(t0.elapsed() < Duration::from_secs(20), "proxy did not start");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let mut s = TcpStream::connect(("127.0.0.1", proxy_port)).await.unwrap();
    s.write_all(format!("GET http://127.0.0.1:{up}{path} HTTP/1.1\r\nHost: 127.0.0.1:{up}\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
    let mut got = Vec::new();
    tokio::time::timeout(Duration::from_secs(60), s.read_to_end(&mut got)).await.expect("download in time").unwrap();
    let split = got.windows(4).position(|w| w == b"\r\n\r\n").expect("headers") + 4;
    let head = String::from_utf8_lossy(&got[..split]).to_ascii_lowercase();
    let body = if with_length {
        assert!(head.contains(&format!("content-length: {SIZE}")), "{head}");
        got[split..].to_vec()
    } else {
        assert!(head.contains("transfer-encoding: chunked"), "{head}");
        dechunk(&got[split..])
    };
    assert_eq!(body.len(), SIZE);
    assert!(body.iter().enumerate().all(|(i, b)| *b == byte(i)), "body changed on the way");

    let mut m = SessionModel::default();
    let t = Instant::now();
    loop {
        match rx.recv_timeout(Duration::from_millis(300)) {
            Ok(ev) => m.apply(ev),
            Err(_) if m.rows.iter().any(|r| r.comment.starts_with("streamed")) => break,
            Err(_) => {}
        }
        assert!(t.elapsed() < Duration::from_secs(20), "no final update");
    }
    let r = m.rows.iter().find(|r| r.url.ends_with(path)).unwrap();
    assert_eq!(r.response_raw_size, Some(SIZE));
    let kept = r.response_bytes();
    assert_eq!(kept.len(), LARGE_BODY_KEEP);
    assert!(kept.iter().enumerate().all(|(i, b)| *b == byte(i)));
    assert!(r.comment.contains("large body: kept the first"), "{}", r.comment);
    proxy.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn head_of_a_large_file_has_no_body() {
    let up = start_server().await;
    let proxy_port = free_port();
    let (tx, rx) = mpsc::channel::<SessionEvent>();
    let proxy = tokio::spawn(ferrotap::proxy::run(proxy_port, Some(tx)));
    let t0 = Instant::now();
    while TcpStream::connect(("127.0.0.1", proxy_port)).await.is_err() {
        assert!(t0.elapsed() < Duration::from_secs(20), "proxy did not start");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let mut s = TcpStream::connect(("127.0.0.1", proxy_port)).await.unwrap();
    s.write_all(format!("HEAD http://127.0.0.1:{up}/big.bin HTTP/1.1\r\nHost: 127.0.0.1:{up}\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
    let mut got = Vec::new();
    tokio::time::timeout(Duration::from_secs(20), s.read_to_end(&mut got)).await.expect("HEAD answered in time").unwrap();
    let text = String::from_utf8_lossy(&got).to_ascii_lowercase();
    assert!(text.starts_with("http/1.1 200"), "{text}");
    assert!(text.contains(&format!("content-length: {SIZE}")), "{text}");
    assert!(text.ends_with("\r\n\r\n"), "no body after the headers: {} bytes", got.len());
    let t = Instant::now();
    let r = loop {
        if let Ok(SessionEvent::New(r) | SessionEvent::Update(r)) = rx.recv_timeout(Duration::from_millis(300)) {
            if r.method == "HEAD" && !r.comment.starts_with("streaming") {
                break r;
            }
        }
        assert!(t.elapsed() < Duration::from_secs(20), "HEAD session not recorded");
    };
    assert_eq!(r.status, 200);
    assert!(r.response_body.is_empty());
    proxy.abort();
}
