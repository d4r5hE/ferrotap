//! G12 rule extras: pure helpers, then end to end through the real proxy.
use std::sync::mpsc;
use std::time::{Duration, Instant};

use ferrotap::model::{set_global_rules, Rule, SessionEvent};
use ferrotap::rules_ext::HeaderEdit;
use ferrotap::rules_more::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn he(op: &str, name: &str, value: &str) -> HeaderEdit {
    HeaderEdit { op: op.into(), name: name.into(), value: value.into() }
}

#[test]
fn query_and_cookie_edits() {
    let u = "https://a.test/p?x=1&y=2&x=3";
    assert_eq!(edit_query(u, &[he("set", "x", "9")]), "https://a.test/p?x=9&y=2");
    assert_eq!(edit_query(u, &[he("remove", "x", "")]), "https://a.test/p?y=2");
    assert_eq!(edit_query("https://a.test/p", &[he("add", "q", "a b")]), "https://a.test/p?q=a%20b");
    assert_eq!(edit_query("https://a.test/p?y=2", &[he("remove", "y", "")]), "https://a.test/p");
    assert_eq!(edit_cookie_header(Some("a=1; b=2"), &[he("set", "b", "9"), he("set", "c", "3")]).as_deref(), Some("a=1; b=9; c=3"));
    assert_eq!(edit_cookie_header(Some("a=1"), &[he("remove", "a", "")]), None);
    assert_eq!(kv_edits_from_text("set a=1\nremove b\nbogus x\nadd c="), vec![he("set", "a", "1"), he("remove", "b", ""), he("add", "c", "")]);
    assert_eq!(kv_edits_to_text(&[he("set", "a", "1"), he("remove", "b", "")]), "set a=1\nremove b");
}

#[test]
fn throttle_math_and_folder_paths() {
    assert_eq!(throttle_delay(1000, 8), Duration::from_millis(1000));
    assert_eq!(throttle_delay(10, 0), Duration::ZERO);
    assert_eq!(throttle_delay(usize::MAX / 16, 1), Duration::from_secs(120));
    assert!(!should_drop(0) && should_drop(100));
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir(d.path().join("sub")).unwrap();
    let dir = d.path().display().to_string();
    assert_eq!(folder_file(&dir, "", "https://x/a/b.js?v=1").unwrap(), d.path().join("a").join("b.js"));
    assert_eq!(folder_file(&dir, "https://x/static/", "https://x/static/app.css").unwrap(), d.path().join("app.css"));
    assert_eq!(folder_file(&dir, "", "https://x/").unwrap(), d.path().join("index.html"));
    assert_eq!(folder_file(&dir, "", "https://x/sub").unwrap(), d.path().join("sub").join("index.html"));
    assert!(folder_file(&dir, "", "https://x/../secret").is_none());
    assert!(folder_file(&dir, "", "https://x/%2e%2e/secret").is_none());
    assert!(folder_file(&dir, "", "https://x/C:/Windows").is_none());
}

#[test]
fn extras_survive_rule_files() {
    let mut r = Rule { name: "x".into(), ..Default::default() };
    assert!(!serde_json::to_string(&r).unwrap().contains("\"more\""), "empty extras are not written");
    r.more.kbps = 500;
    r.more.no_cookies = true;
    let back: Rule = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
    assert_eq!(back.more, r.more);
}

// ------------------------------------------------------------------------------------------ end to end

/// Echo server: body = request head; Cache headers + a cookie so the edits are visible.
async fn start_echo() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else { return };
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut tmp).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&tmp[..n]),
                    }
                }
                let head = String::from_utf8_lossy(&buf).to_string();
                let body = format!("{head}{}", "x".repeat(1000));
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nETag: \"abc\"\r\nCache-Control: max-age=3600\r\nSet-Cookie: s=1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(resp.as_bytes()).await;
            });
        }
    });
    port
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

async fn send(proxy: u16, url: &str, extra: &str) -> (u16, String, Duration) {
    let t0 = Instant::now();
    let mut s = TcpStream::connect(("127.0.0.1", proxy)).await.unwrap();
    let host = url.split("://").nth(1).unwrap().split('/').next().unwrap();
    s.write_all(format!("GET {url} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n{extra}\r\n").as_bytes()).await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(20), s.read_to_end(&mut out)).await;
    let t = String::from_utf8_lossy(&out).into_owned();
    (t.split_whitespace().nth(1).and_then(|x| x.parse().ok()).unwrap_or(0), t, t0.elapsed())
}

fn rule(name: &str, pattern: &str, action: &str) -> Rule {
    Rule { name: name.into(), pattern: pattern.into(), action: action.into(), ..Default::default() }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn extras_through_the_proxy() {
    let up = start_echo().await;
    let base = format!("http://127.0.0.1:{up}");
    let proxy_port = free_port();
    let (tx, _rx) = mpsc::channel::<SessionEvent>();
    let proxy = tokio::spawn(ferrotap::proxy::run(proxy_port, Some(tx)));
    let t0 = Instant::now();
    while TcpStream::connect(("127.0.0.1", proxy_port)).await.is_err() {
        assert!(t0.elapsed() < Duration::from_secs(20), "proxy did not start");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // 1. query / cookie / user-agent / no-cache / status override
    let mut m = rule("rewrite", "/edit", "modify");
    m.more = RuleExtras {
        query: vec![he("set", "debug", "1"), he("remove", "secret", "")],
        cookies: vec![he("set", "flag", "on")],
        user_agent: "FerroBot/1".into(),
        no_cache: true,
        status: 202,
        ..Default::default()
    };
    set_global_rules(vec![m]);
    let (st, body, _) = send(proxy_port, &format!("{base}/edit?secret=x&keep=2"), "Cookie: a=1\r\nIf-None-Match: \"abc\"\r\nUser-Agent: real\r\n").await;
    assert_eq!(st, 202, "{body}");
    assert!(body.contains("GET /edit?keep=2&debug=1 HTTP/1.1"), "{body}");
    let lower = body.to_ascii_lowercase();
    assert!(lower.contains("cookie: a=1; flag=on"), "{body}");
    assert!(lower.contains("user-agent: ferrobot/1") && !lower.contains("user-agent: real"), "{body}");
    assert!(!lower.contains("if-none-match"), "{body}");
    assert!(lower.contains("cache-control: no-store, no-cache, must-revalidate") && !lower.contains("etag:"), "{body}");

    // 2. no cookies
    let mut nc = rule("nocookies", "/nc", "modify");
    nc.more.no_cookies = true;
    set_global_rules(vec![nc]);
    let (_, body, _) = send(proxy_port, &format!("{base}/nc"), "Cookie: a=1\r\n").await;
    let lower = body.to_ascii_lowercase();
    assert!(!lower.contains("cookie: a=1") && !lower.contains("set-cookie"), "{body}");

    // 3. Map Local folder
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), "<h1>home</h1>").unwrap();
    std::fs::write(dir.path().join("app.json"), "{\"ok\":true}").unwrap();
    let mut f = rule("site", "folder.test", "folder");
    f.file = dir.path().display().to_string();
    f.target = "http://folder.test/static/".into();
    set_global_rules(vec![f]);
    let (st, body, _) = send(proxy_port, "http://folder.test/static/app.json", "").await;
    assert_eq!(st, 200);
    assert!(body.contains("application/json") && body.ends_with("{\"ok\":true}"), "{body}");
    let (st, body, _) = send(proxy_port, "http://folder.test/static/", "").await;
    assert_eq!(st, 200);
    assert!(body.ends_with("<h1>home</h1>"));
    assert_eq!(send(proxy_port, "http://folder.test/static/missing.txt", "").await.0, 404);
    assert_eq!(send(proxy_port, "http://folder.test/static/%2e%2e/x", "").await.0, 403);

    // 4. allow list
    set_global_rules(vec![rule("only this", "/allowed", "allow_only")]);
    assert_eq!(send(proxy_port, &format!("{base}/allowed"), "").await.0, 200);
    let (st, body, _) = send(proxy_port, &format!("{base}/other"), "").await;
    assert_eq!(st, 403);
    assert!(body.contains("not on the allow list"));

    // 5. bandwidth: ~1.1 KB at 8 kbit/s takes about a second
    let mut slow = rule("slow", "/slow", "modify");
    slow.more.kbps = 8;
    set_global_rules(vec![slow]);
    let (st, _, took) = send(proxy_port, &format!("{base}/slow"), "").await;
    assert_eq!(st, 200);
    assert!(took >= Duration::from_millis(900), "{took:?}");

    // 6. 100% loss drops the connection
    let mut off = rule("offline", "/off", "modify");
    off.more.loss_pct = 100;
    set_global_rules(vec![off]);
    let (st, body, _) = send(proxy_port, &format!("{base}/off"), "").await;
    assert!(st == 0 || st == 502, "{st} {body}");
    assert!(!body.contains("xxxx"), "no body may reach the client: {body}");

    set_global_rules(Vec::new());
    proxy.abort();
}
