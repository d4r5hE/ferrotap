//! Live verification of system proxy toggle/restore and launchers against the Rust proxy.
//!   proxy_verify  -> runs all checks, exit code 0 if all pass
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use ferrotap::model::SessionEvent;
use ferrotap::proxy;
use ferrotap::system_proxy::*;

fn drain(rx: &mpsc::Receiver<SessionEvent>, secs: u64, needle: &str) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if let Ok(SessionEvent::New(s)) = rx.recv_timeout(Duration::from_millis(200)) {
            println!("  [CAPTURED] #{} {} {} -> {}", s.id, s.method, s.url, s.status);
            if s.url.contains(needle) {
                return true;
            }
        }
    }
    false
}

fn main() {
    let mut fails = 0;
    let mut check = |name: &str, ok: bool| {
        println!("{} {}", if ok { "PASS" } else { "FAIL" }, name);
        if !ok {
            fails += 1;
        }
    };

    let (tx, rx) = mpsc::channel::<SessionEvent>();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _ = rt.block_on(proxy::run(proxy::PORT, Some(tx)));
    });
    std::thread::sleep(Duration::from_secs(2));

    let mgr = Arc::new(SystemProxyManager::new());
    let before = mgr.get_state().unwrap();
    println!("BEFORE: {:?}", before);

    {
        let _guard = SystemProxyGuard::new(mgr.clone());
        mgr.enable(DEFAULT_PROXY_SERVER, DEFAULT_OVERRIDE).unwrap();
        let on = mgr.get_state().unwrap();
        println!("ON:     {:?}", on);
        check("registry enabled with 127.0.0.1:8897", on.enabled && on.server == DEFAULT_PROXY_SERVER);
        let out = Command::new("powershell.exe")
            .args(["-NoProfile", "-Command", "[System.Net.WebRequest]::GetSystemWebProxy().GetProxy('http://example.com').ToString()"])
            .output()
            .unwrap();
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        println!("  system proxy resolves to: {s}");
        check("WinINet resolves proxy 127.0.0.1:8897", s.contains("8897"));

        let st = Command::new("curl.exe")
            .args(["-s", "-o", "NUL", "-m", "15", "http://example.com/term_probe"])
            .envs(build_terminal_env(DEFAULT_PROXY_SERVER, None))
            .stdout(Stdio::null())
            .status();
        println!("  curl status: {:?}", st);
        check("terminal env traffic captured", drain(&rx, 8, "term_probe"));

        match launch_terminal(DEFAULT_PROXY_SERVER, TerminalType::PowerShell) {
            Ok(mut c) => {
                check("launch_terminal spawned", true);
                std::thread::sleep(Duration::from_secs(2));
                let _ = c.kill();
            }
            Err(e) => check(&format!("launch_terminal spawned ({e})"), false),
        }

        match launch_clean_browser(DEFAULT_PROXY_SERVER, Some("http://example.com/browser_probe")) {
            Ok(mut c) => {
                check("launch_clean_browser spawned", true);
                check("browser traffic captured", drain(&rx, 25, "browser_probe"));
                let _ = c.kill();
            }
            Err(e) => check(&format!("launch_clean_browser spawned ({e})"), false),
        }
    }

    let after = mgr.get_state().unwrap();
    println!("AFTER:  {:?}", after);
    check("registry restored after guard drop", after == before);
    println!("RESULT: {} failure(s)", fails);
    std::process::exit(if fails == 0 { 0 } else { 1 });
}
