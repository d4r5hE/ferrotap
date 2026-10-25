//! Advanced replay: N times, bounded concurrency, pacing, stop, and the results summary.
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use ferrotap::model::SessionRecord;
use ferrotap::replay_adv::*;

/// Answers every request after 80 ms; tracks the most requests in flight at once.
/// Path /fail answers 503.
fn server() -> (String, Arc<AtomicUsize>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let (now, peak) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let p = peak.clone();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { continue };
            let (now, peak) = (now.clone(), p.clone());
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                let n = s.read(&mut buf).unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]).to_string();
                let cur = now.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(cur, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(80));
                now.fetch_sub(1, Ordering::SeqCst);
                let status = if head.contains("/fail") { "503 Busy" } else { "200 OK" };
                let _ = s.write_all(format!("HTTP/1.1 {status}\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello").as_bytes());
            });
        }
    });
    (addr, peak)
}

fn sessions(addr: &str) -> Vec<SessionRecord> {
    vec![
        SessionRecord { id: 1, method: "GET".into(), url: format!("http://{addr}/a"), ..Default::default() },
        SessionRecord { id: 2, method: "GET".into(), url: format!("http://{addr}/fail"), ..Default::default() },
        SessionRecord { id: 3, method: "CONNECT".into(), url: "tunnel://x:443".into(), ..Default::default() },
    ]
}

fn collect(rx: mpsc::Receiver<RunResult>) -> Vec<RunResult> {
    let mut v: Vec<RunResult> = rx.iter().collect();
    v.sort_by_key(|r| r.n);
    v
}

#[test]
fn concurrency_is_bounded_and_summarized() {
    let (addr, peak) = server();
    let jobs = jobs(&sessions(&addr), 5);
    assert_eq!(jobs.len(), 10, "CONNECT skipped, 2 sessions x 5");
    let (tx, rx) = mpsc::channel();
    let t = Instant::now();
    run(jobs, &addr, AdvancedOptions { count: 5, concurrency: 3, delay_ms: 0 }, tx, Arc::new(AtomicBool::new(false)));
    let took = t.elapsed();
    let res = collect(rx);
    assert_eq!(res.len(), 10);
    assert_eq!(peak.load(Ordering::SeqCst), 3, "at most 3 in flight");
    assert!(took < Duration::from_millis(10 * 80), "ran in parallel: {took:?}");
    let s = summarize(&res, took);
    assert_eq!((s.runs, s.ok, s.failed), (10, 5, 5));
    assert_eq!(s.by_status, vec![(200, 5), (503, 5)]);
    assert!(s.min_ms >= 70 && s.max_ms >= s.avg_ms && s.avg_ms >= s.min_ms, "{s:?}");
    assert_eq!(s.total_bytes, 50);
    assert_eq!(res.iter().filter(|r| r.session_id == 1).count(), 5);
}

#[test]
fn sequential_pacing_and_stop() {
    let (addr, peak) = server();
    let one = &sessions(&addr)[..1];
    let (tx, rx) = mpsc::channel();
    let t = Instant::now();
    run(jobs(one, 3), &addr, AdvancedOptions { count: 3, concurrency: 1, delay_ms: 150 }, tx, Arc::new(AtomicBool::new(false)));
    assert_eq!(collect(rx).len(), 3);
    assert_eq!(peak.load(Ordering::SeqCst), 1);
    assert!(t.elapsed() >= Duration::from_millis(300), "two pauses of 150 ms: {:?}", t.elapsed());

    // stop before the start: nothing runs
    let (tx, rx) = mpsc::channel();
    run(jobs(one, 50), &addr, AdvancedOptions { count: 50, concurrency: 2, delay_ms: 0 }, tx, Arc::new(AtomicBool::new(true)));
    assert!(collect(rx).is_empty());

    // background run through the UI state, cancelled midway
    let mut ui = ReplayUi::default();
    ui.sessions = one.to_vec();
    ui.opt = Some(AdvancedOptions { count: 40, concurrency: 1, delay_ms: 50 });
    ui.start(addr.clone());
    assert!(ui.running());
    std::thread::sleep(Duration::from_millis(400));
    ui.cancel();
    let t = Instant::now();
    while ui.poll() {
        assert!(t.elapsed() < Duration::from_secs(5), "did not stop");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!ui.running());
    assert!(!ui.results.is_empty() && ui.results.len() < 40, "{}", ui.results.len());
    assert!(ui.finished.is_some());
}
