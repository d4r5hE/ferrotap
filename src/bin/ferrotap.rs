//! Live grid: runs the proxy core (port 8897) on a background tokio thread and streams every
//! captured session over an mpsc channel into the egui event loop.
//!   ferrotap             -> open the native window
//!   ferrotap --selftest  -> headless: print each session as it arrives on the channel
//!                               (exits after --secs N, default 30)
use std::sync::mpsc;
use std::time::{Duration, Instant};

use ferrotap::model::SessionEvent;
use ferrotap::proxy;
use ferrotap::ui::GridApp;

fn main() {
    ferrotap::system_proxy::install_console_handler();
    let _ = ferrotap::system_proxy::global_manager().recover_stale();
    let args: Vec<String> = std::env::args().collect();
    let selftest = args.iter().any(|a| a == "--selftest");
    let secs: u64 = args
        .iter()
        .position(|a| a == "--secs")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);

    let (tx, rx) = mpsc::channel::<SessionEvent>();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        if let Err(e) = rt.block_on(proxy::run(ferrotap::conn::current().port, Some(tx))) {
            eprintln!("proxy stopped: {e}");
        }
    });

    if selftest {
        let end = Instant::now() + Duration::from_secs(secs);
        let mut n = 0;
        while Instant::now() < end {
            if let Ok(SessionEvent::New(s)) = rx.recv_timeout(Duration::from_millis(100)) {
                n += 1;
                println!(
                    "[CHANNEL] #{} {} {} -> {} {} {}ms",
                    s.id, s.method, s.url, s.status, s.http_version, s.duration_ms
                );
            }
        }
        println!("[CHANNEL] total received: {n}");
        return;
    }

    let label = format!("live proxy 127.0.0.1:{}", ferrotap::conn::current().port);
    if let Err(e) = GridApp::new(rx, label).run("Ferrotap - Live Traffic") {
        eprintln!("ui error: {e}");
    }
    ferrotap::system_proxy::shutdown_restore();
}
