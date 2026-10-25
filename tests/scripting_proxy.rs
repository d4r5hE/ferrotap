//! G9 scripts end to end through the proxy, the --scripts option, and the Scripts window.
use std::io::Write as _;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use eframe::egui;
use ferrotap::model::SessionEvent;
use ferrotap::scripting::{set_active, Script};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn gzip(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

/// Upstream: echoes the request head in an X-Seen header (base64-free: just the header names), gzip body.
async fn start_upstream() -> u16 {
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
                let head = String::from_utf8_lossy(&buf).to_ascii_lowercase();
                let tagged = if head.contains("x-from-script: yes") { "tagged" } else { "plain" };
                let line = head.lines().next().unwrap_or("").to_string();
                let body = gzip(format!("{tagged} {line} password=hunter2").as_bytes());
                let resp = format!("HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = s.write_all(resp.as_bytes()).await;
                let _ = s.write_all(&body).await;
            });
        }
    });
    port
}

/// The active script set is process-wide: these tests take turns.
static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scripts_change_traffic_through_the_proxy() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let up = start_upstream().await;
    let proxy_port = free_port();
    let (tx, rx) = mpsc::channel::<SessionEvent>();
    let proxy = tokio::spawn(ferrotap::proxy::run(proxy_port, Some(tx)));
    let t0 = Instant::now();
    while TcpStream::connect(("127.0.0.1", proxy_port)).await.is_err() {
        assert!(t0.elapsed() < Duration::from_secs(20), "proxy did not start");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let errs = set_active(&[Script {
        name: "edit".into(),
        enabled: true,
        code: r#"
fn on_request(req) {
    req.headers["x-from-script"] = "yes";
    req.url.replace("/v1/", "/v2/");
    req
}
fn on_response(req, resp) {
    resp.body.replace("hunter2", "******");
    resp.headers["x-script"] = "edit";
    resp
}
"#
        .into(),
    }]);
    assert!(errs.is_empty(), "{errs:?}");

    let mut s = TcpStream::connect(("127.0.0.1", proxy_port)).await.unwrap();
    s.write_all(format!("GET http://127.0.0.1:{up}/v1/thing HTTP/1.1\r\nHost: 127.0.0.1:{up}\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(15), s.read_to_end(&mut out)).await;
    let text = String::from_utf8_lossy(&out).to_string();
    let lower = text.to_ascii_lowercase();
    assert!(text.ends_with("tagged get /v2/thing http/1.1 password=******"), "{text}");
    assert!(lower.contains("x-script: edit") && !lower.contains("content-encoding"), "{text}");

    let t = Instant::now();
    let sess = loop {
        if let Ok(SessionEvent::New(s)) = rx.recv_timeout(Duration::from_millis(200)) {
            if s.url.contains("/v2/thing") {
                break s;
            }
        }
        assert!(t.elapsed() < Duration::from_secs(10), "no session");
    };
    assert!(sess.comment.contains("Script: edit"), "{}", sess.comment);
    set_active(&[]);
    proxy.abort();
}

#[test]
fn cli_option_and_window() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let o = ferrotap::auto_cli::CliOptions::parse(["--scripts", "a.rhai", "--scripts=b.json"]).unwrap();
    assert_eq!(o.scripts, vec![std::path::PathBuf::from("a.rhai"), std::path::PathBuf::from("b.json")]);
    assert!(ferrotap::auto_cli::USAGE.contains("--scripts"));

    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("FERROTAP_SCRIPTS", dir.path().join("scripts.json"));
    std::env::set_var("FERROTAP_PLUGINS", dir.path().join("plugins"));
    let (_tx, rx) = mpsc::channel();
    if std::env::var_os("FERROTAP_UI_STATE").is_none() { std::env::set_var("FERROTAP_UI_STATE", std::env::temp_dir().join(format!("ferrotap_ui_test_{}.json", std::process::id()))); }
    let mut app = ferrotap::ui::GridApp::new(rx, "test");
    app.ext.scripts.show = true;
    app.ext.scripts.scripts = Some(vec![Script { name: "mine".into(), enabled: true, code: "fn column(s) { s.url }".into() }]);
    app.ext.scripts.sel = Some(0);
    let ctx = egui::Context::default();
    let mut text = String::new();
    let tall = || egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1600.0, 1400.0))), ..Default::default() };
    for _ in 0..3 {
        let mut out = ctx.run_ui(tall(), |ui| app.scripts_windows(ui.ctx()));
        out.textures_delta.clear();
        text.clear();
        for s in &out.shapes {
            fn walk(s: &egui::Shape, out: &mut String) {
                match s {
                    egui::Shape::Text(t) => {
                        out.push_str(t.galley.text());
                        out.push('\n');
                    }
                    egui::Shape::Vec(v) => v.iter().for_each(|x| walk(x, out)),
                    _ => {}
                }
            }
            walk(&s.shape, &mut text);
        }
    }
    for want in ["Scripts", "Sandbox", "Save and apply", "New script", "mine", "OK - hooks: column", "Plugins", "Diagnostics", "Insert:"] {
        assert!(text.contains(want), "missing {want} in\n{text}");
    }
    set_active(&[]);
}
