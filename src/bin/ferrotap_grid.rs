//! Native session grid. `ferrotap_grid --demo` feeds synthetic sessions (UI smoke test);
//! live proxy connection is wired in the next step.
use std::sync::mpsc;
use std::time::Duration;

use ferrotap::model::{SessionEvent, SessionRecord};
use ferrotap::ui::GridApp;

fn main() -> eframe::Result<()> {
    ferrotap::system_proxy::install_console_handler();
    let demo = std::env::args().any(|a| a == "--demo");
    let (tx, rx) = mpsc::channel::<SessionEvent>();
    let label = if demo { "demo" } else { "none (use --demo)" };
    if demo {
        std::thread::spawn(move || {
            let urls = [
                ("GET", "https://example.com/", 200u16, "text/html"),
                ("POST", "http://httpbin.org/post", 201, "application/json"),
                ("GET", "https://api.test.local/missing", 404, "application/json"),
                ("GET", "https://cdn.test.local/app.js", 500, "text/javascript"),
            ];
            for i in 0..40usize {
                let (m, u, st, ct) = urls[i % urls.len()];
                let mut r = SessionRecord {
                    id: i + 1,
                    method: m.into(),
                    url: u.into(),
                    status: st,
                    http_version: "HTTP/1.1".into(),
                    response_body: format!("demo body {i}"),
                    start_time: chrono::Utc::now().to_rfc3339(),
                    duration_ms: 10 + i as u128,
                    ..Default::default()
                };
                r.response_headers.insert("content-type".into(), ct.into());
                r.request_headers.insert("host".into(), "demo".into());
                if tx.send(SessionEvent::New(r)).is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(400));
            }
        });
    }
    GridApp::new(rx, label).run("Ferrotap - Live Traffic")
}
