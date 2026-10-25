//! End to end: plain HTTP client -> Ferrotap proxy (with rules) -> local echo server.
//! Every rule action is checked through the real proxy. Rules are set in memory with
//! `set_global_rules` (the user's rules.json is never read or written).
use std::sync::mpsc;
use std::time::{Duration, Instant};

use ferrotap::model::{set_global_rules, Rule, SessionEvent};
use ferrotap::rules_ext::{rule_hits, HeaderEdit, RuleCond, TextReplace};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn find(h: &[u8], pat: &[u8]) -> Option<usize> {
    h.windows(pat.len()).position(|w| w == pat)
}

/// Echo server: status from `/status/NNN`, body = "ECHO\n" + request head + request body. Keep-alive.
async fn start_echo() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else { return };
            tokio::spawn(async move {
                let mut buf: Vec<u8> = Vec::new();
                let mut tmp = [0u8; 4096];
                loop {
                    // Read one request.
                    let head_end = loop {
                        if let Some(i) = find(&buf, b"\r\n\r\n") {
                            break i + 4;
                        }
                        let n = s.read(&mut tmp).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                    };
                    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                    let clen = head
                        .lines()
                        .find_map(|l| l.split_once(':').filter(|(k, _)| k.trim().eq_ignore_ascii_case("content-length")).map(|(_, v)| v.trim().parse::<usize>().unwrap_or(0)))
                        .unwrap_or(0);
                    while buf.len() < head_end + clen {
                        let n = s.read(&mut tmp).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                    }
                    let body = String::from_utf8_lossy(&buf[head_end..head_end + clen]).to_string();
                    buf.drain(..head_end + clen);
                    let path = head.split_whitespace().nth(1).unwrap_or("/").to_string();
                    let status: u16 = path.strip_prefix("/status/").and_then(|x| x.parse().ok()).unwrap_or(200);
                    let out = format!("ECHO\n{head}{body}");
                    let resp = format!(
                        "HTTP/1.1 {status} X\r\nContent-Type: text/plain\r\nX-Upstream: 1\r\nContent-Length: {}\r\n\r\n{out}",
                        out.len()
                    );
                    if s.write_all(resp.as_bytes()).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    port
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

struct Resp {
    raw: String,
    status: u16,
    elapsed: Duration,
}

impl Resp {
    fn header(&self, name: &str) -> Option<String> {
        let head = self.raw.split("\r\n\r\n").next().unwrap_or("");
        head.lines().skip(1).find_map(|l| {
            l.split_once(':').filter(|(k, _)| k.trim().eq_ignore_ascii_case(name)).map(|(_, v)| v.trim().to_string())
        })
    }
    fn body(&self) -> String {
        self.raw.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default()
    }
}

/// Send one request through the proxy (absolute-form URL) and read until the connection closes.
async fn send(proxy: u16, method: &str, url: &str, headers: &[(&str, &str)], body: &str) -> Resp {
    let t0 = Instant::now();
    let mut s = TcpStream::connect(("127.0.0.1", proxy)).await.unwrap();
    let host = url.split("://").nth(1).unwrap().split('/').next().unwrap();
    let mut req = format!("{method} {url} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    if !body.is_empty() || method == "POST" {
        req.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    req.push_str("\r\n");
    req.push_str(body);
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    let mut tmp = [0u8; 4096];
    loop {
        match tokio::time::timeout(Duration::from_secs(10), s.read(&mut tmp)).await {
            Ok(Ok(0)) | Ok(Err(_)) => break,
            Ok(Ok(n)) => out.extend_from_slice(&tmp[..n]),
            Err(_) => panic!("read timeout for {url}"),
        }
    }
    let raw = String::from_utf8_lossy(&out).to_string();
    let status = raw.split_whitespace().nth(1).and_then(|x| x.parse().ok()).unwrap_or(0);
    Resp { raw, status, elapsed: t0.elapsed() }
}

fn rule(name: &str, id: u64) -> Rule {
    Rule { name: name.into(), id, ..Default::default() }
}

fn cond(kind: &str, name: &str, op: &str, value: &str) -> RuleCond {
    RuleCond { enabled: true, kind: kind.into(), name: name.into(), op: op.into(), value: value.into() }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rule_actions_through_real_proxy() {
    let up = start_echo().await;
    let base = format!("http://127.0.0.1:{up}");
    let proxy_port = free_port();
    let (tx, rx) = mpsc::channel::<SessionEvent>();
    let proxy = tokio::spawn(ferrotap::proxy::run(proxy_port, Some(tx)));
    let t0 = Instant::now();
    while TcpStream::connect(("127.0.0.1", proxy_port)).await.is_err() {
        assert!(t0.elapsed() < Duration::from_secs(20), "proxy did not start");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // No rules: plain pass-through.
    set_global_rules(vec![]);
    let r = send(proxy_port, "GET", &format!("{base}/plain"), &[], "").await;
    assert_eq!(r.status, 200, "{}", r.raw);
    assert!(r.body().starts_with("ECHO"));

    // 1. Host + header condition -> canned response (rule 1); without the header -> upstream.
    let mut a = rule("canned-host-header", 9001);
    a.host = "=127.0.0.1".into();
    a.conds = vec![cond("header", "X-Test", "equals", "yes")];
    a.status = 418;
    a.body = "teapot".into();
    set_global_rules(vec![a]);
    let r = send(proxy_port, "GET", &format!("{base}/x"), &[("X-Test", "yes")], "").await;
    assert_eq!(r.status, 418, "{}", r.raw);
    assert_eq!(r.body(), "teapot");
    let r = send(proxy_port, "GET", &format!("{base}/x"), &[("X-Test", "no")], "").await;
    assert_eq!(r.status, 200);
    assert_eq!(rule_hits(9001), 1);

    // 2. Cookie condition -> block (403).
    let mut b = rule("block-cookie", 9002);
    b.conds = vec![cond("cookie", "session", "equals", "evil")];
    b.action = "block".into();
    set_global_rules(vec![b]);
    let r = send(proxy_port, "GET", &format!("{base}/c"), &[("Cookie", "a=1; session=evil")], "").await;
    assert_eq!(r.status, 403, "{}", r.raw);
    assert!(r.body().contains("Blocked"));
    let r = send(proxy_port, "GET", &format!("{base}/c"), &[("Cookie", "session=good")], "").await;
    assert_eq!(r.status, 200);

    // 3. Request body condition -> redirect 301.
    let mut c = rule("redirect-body", 9003);
    c.method = "POST".into();
    c.conds = vec![cond("body", "", "contains", "secret")];
    c.action = "redirect".into();
    c.status = 301;
    c.target = "http://example.invalid/new".into();
    set_global_rules(vec![c]);
    let r = send(proxy_port, "POST", &format!("{base}/form"), &[], "a=secret").await;
    assert_eq!(r.status, 301, "{}", r.raw);
    assert_eq!(r.header("location").as_deref(), Some("http://example.invalid/new"));
    let r = send(proxy_port, "POST", &format!("{base}/form"), &[], "a=public").await;
    assert_eq!(r.status, 200);

    // 4. Map Local: serve a file, content type from its extension.
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("data.json");
    std::fs::write(&f, "{\"local\": true}").unwrap();
    let mut d = rule("map-local", 9004);
    d.pattern = "/api/data".into();
    d.action = "file".into();
    d.file = f.to_string_lossy().to_string();
    set_global_rules(vec![d]);
    let r = send(proxy_port, "GET", &format!("{base}/api/data?x=1"), &[], "").await;
    assert_eq!(r.status, 200);
    assert_eq!(r.header("content-type").as_deref(), Some("application/json"));
    assert_eq!(r.body(), "{\"local\": true}");

    // 5. Map Remote: send to another URL and return its response.
    let mut e = rule("map-remote", 9005);
    e.host = "never-resolves.invalid".into();
    e.action = "map_remote".into();
    e.target = format!("{base}/mapped/target");
    set_global_rules(vec![e]);
    let r = send(proxy_port, "GET", "http://never-resolves.invalid/orig", &[], "").await;
    assert_eq!(r.status, 200, "{}", r.raw);
    assert!(r.body().contains("GET /mapped/target"), "{}", r.raw);
    // Host-only target keeps the path.
    let mut e2 = rule("map-remote-host", 9015);
    e2.host = "never-resolves.invalid".into();
    e2.action = "map_remote".into();
    e2.target = base.clone();
    set_global_rules(vec![e2]);
    let r = send(proxy_port, "GET", "http://never-resolves.invalid/keep/path?q=1", &[], "").await;
    assert!(r.body().contains("GET /keep/path?q=1"), "{}", r.raw);

    // 6. Modify request/response headers and bodies (two pass-through rules combine).
    let mut m = rule("modify-req", 9006);
    m.pattern = "/mod".into();
    m.action = "modify".into();
    m.req_headers = vec![
        HeaderEdit { op: "set".into(), name: "X-Added".into(), value: "1".into() },
        HeaderEdit { op: "remove".into(), name: "X-Drop".into(), value: String::new() },
        HeaderEdit { op: "set".into(), name: "X-Replace".into(), value: "new".into() },
    ];
    m.req_body = vec![TextReplace { find: "foo".into(), replace: "bar".into(), regex: false }];
    let mut m2 = rule("modify-resp", 9007);
    m2.pattern = "/mod".into();
    m2.action = "modify".into();
    m2.resp_headers = vec![
        HeaderEdit { op: "add".into(), name: "X-Resp".into(), value: "r".into() },
        HeaderEdit { op: "remove".into(), name: "X-Upstream".into(), value: String::new() },
    ];
    m2.resp_body = vec![TextReplace { find: r"^ECHO".into(), replace: "echo!".into(), regex: true }];
    set_global_rules(vec![m, m2]);
    let r = send(proxy_port, "POST", &format!("{base}/mod"), &[("X-Drop", "x"), ("X-Replace", "old")], "foo foo").await;
    assert_eq!(r.status, 200, "{}", r.raw);
    let body = r.body().to_ascii_lowercase();
    assert!(body.starts_with("echo!"), "{}", r.raw);
    assert!(body.contains("x-added: 1"), "{}", r.raw);
    assert!(!body.contains("x-drop"), "{}", r.raw);
    assert!(body.contains("x-replace: new") && !body.contains("x-replace: old"), "{}", r.raw);
    assert!(body.ends_with("bar bar"), "{}", r.raw);
    assert_eq!(r.header("x-resp").as_deref(), Some("r"));
    assert!(r.header("x-upstream").is_none());
    assert_eq!(r.header("content-length").and_then(|v| v.parse::<usize>().ok()), Some(r.body().len()));

    // "Stop processing more rules": the second rule is not applied.
    let mut s1 = rule("stopper", 9008);
    s1.pattern = "/stop".into();
    s1.action = "modify".into();
    s1.stop = true;
    let mut s2 = rule("after-stop", 9009);
    s2.pattern = "/stop".into();
    s2.status = 500;
    set_global_rules(vec![s1, s2]);
    let r = send(proxy_port, "GET", &format!("{base}/stop"), &[], "").await;
    assert_eq!(r.status, 200);
    assert_eq!(rule_hits(9009), 0);

    // 7. Delay.
    let mut dl = rule("delay", 9010);
    dl.pattern = "/slow".into();
    dl.action = "modify".into();
    dl.delay_ms = 400;
    set_global_rules(vec![dl]);
    let r = send(proxy_port, "GET", &format!("{base}/slow"), &[], "").await;
    assert_eq!(r.status, 200);
    assert!(r.elapsed >= Duration::from_millis(400), "elapsed {:?}", r.elapsed);

    // 8. Close the connection: no complete response reaches the client.
    let mut cl = rule("close", 9011);
    cl.pattern = "/drop".into();
    cl.action = "close".into();
    set_global_rules(vec![cl]);
    let r = send(proxy_port, "GET", &format!("{base}/drop"), &[], "").await;
    assert!(!r.raw.contains("ECHO"), "{}", r.raw);
    assert!(r.body().len() < 1, "connection should be cut: {:?}", r.raw);

    // 9. Response rules: match on status, response header, response body.
    let mut rs = rule("resp-404", 9012);
    rs.stage = "response".into();
    rs.conds = vec![cond("status", "", "", "4xx"), cond("resp_header", "X-Upstream", "equals", "1")];
    rs.status = 200;
    rs.body = "replaced 404".into();
    let mut rm = rule("resp-mod", 9013);
    rm.stage = "response".into();
    rm.conds = vec![cond("resp_body", "", "contains", "/status/201")];
    rm.action = "modify".into();
    rm.resp_headers = vec![HeaderEdit { op: "set".into(), name: "X-Seen".into(), value: "201".into() }];
    set_global_rules(vec![rs, rm]);
    let r = send(proxy_port, "GET", &format!("{base}/status/404"), &[], "").await;
    assert_eq!(r.status, 200, "{}", r.raw);
    assert_eq!(r.body(), "replaced 404");
    let r = send(proxy_port, "GET", &format!("{base}/status/201"), &[], "").await;
    assert_eq!(r.status, 201);
    assert_eq!(r.header("x-seen").as_deref(), Some("201"));
    let r = send(proxy_port, "GET", &format!("{base}/status/500"), &[], "").await;
    assert_eq!(r.status, 500);

    // 10. Groups: a switched-off group does not apply.
    let mut g = rule("grouped", 9014);
    g.pattern = "/grp".into();
    g.group = "mocks".into();
    g.status = 299;
    let mut rules = vec![g];
    ferrotap::rules_ext::set_group_enabled(&mut rules, "mocks", false);
    set_global_rules(rules.clone());
    assert_eq!(send(proxy_port, "GET", &format!("{base}/grp"), &[], "").await.status, 200);
    ferrotap::rules_ext::set_group_enabled(&mut rules, "mocks", true);
    set_global_rules(rules);
    assert_eq!(send(proxy_port, "GET", &format!("{base}/grp"), &[], "").await.status, 299);

    // 11. Process name of the client (this test binary).
    #[cfg(windows)]
    {
        let exe = std::env::current_exe().unwrap();
        let stem = exe.file_stem().unwrap().to_string_lossy().to_string();
        let mut p = rule("process", 9016);
        p.conds = vec![cond("process", "", "contains", &stem)];
        p.pattern = "/proc".into();
        p.status = 203;
        p.body = "by process".into();
        let mut q = rule("other-process", 9017);
        q.conds = vec![cond("process", "", "equals", "definitely-not-running.exe")];
        q.pattern = "/proc".into();
        q.status = 500;
        set_global_rules(vec![q, p]);
        let r = send(proxy_port, "GET", &format!("{base}/proc"), &[], "").await;
        assert_eq!(r.status, 203, "process rule did not match {stem}: {}", r.raw);
    }

    // Sessions record what the rules did.
    set_global_rules(vec![]);
    let mut comments = Vec::new();
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(3) {
        while let Ok(ev) = rx.try_recv() {
            if let SessionEvent::New(s) = ev {
                comments.push((s.url.clone(), s.comment.clone()));
            }
        }
        if comments.iter().any(|(u, _)| u.ends_with("/grp")) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    proxy.abort();
    assert!(comments.iter().any(|(_, c)| c.contains("AutoResponder: canned-host-header")), "{comments:?}");
    assert!(comments.iter().any(|(_, c)| c.contains("Rules: modify-req, modify-resp")), "{comments:?}");
    assert!(comments.iter().any(|(u, c)| u.ends_with("/mapped/target") && c.contains("Map Remote from http://never-resolves.invalid/orig")), "{comments:?}");
    assert!(comments.iter().any(|(_, c)| c.contains("Rule closed connection: close")), "{comments:?}");
}
