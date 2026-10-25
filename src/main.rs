//! ferrotap_proxy: the headless proxy core (no UI). The app with the window is `ferrotap`.
//! Options (see `--help`): --port, --out, --every, --rules, --duration, --quiet, --api-port, --api-token.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use ferrotap::auto_api::{random_token, ApiCmd, ApiServer, Shared};
use ferrotap::auto_cli::{self, CliOptions};
use ferrotap::model::SessionEvent;
use ferrotap::proxy;

fn main() {
    let opts = match CliOptions::parse(std::env::args().skip(1)) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("error: {e}\n\n{}", auto_cli::USAGE);
            std::process::exit(2);
        }
    };
    if opts.help {
        println!("{}", auto_cli::USAGE);
        return;
    }
    let mut rules = Vec::new();
    if let Some(p) = &opts.rules {
        match auto_cli::load_rules_file(p) {
            Ok(r) => {
                eprintln!("loaded {} rule(s) from {}", r.len(), p.display());
                rules = r;
            }
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(2);
            }
        }
    }
    ferrotap::model::set_global_rules(rules.clone());
    if !opts.scripts.is_empty() {
        let mut all = Vec::new();
        for p in &opts.scripts {
            match ferrotap::scripting::load_cli(p) {
                Ok(s) => all.extend(s),
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(2);
                }
            }
        }
        let errs = ferrotap::scripting::set_active(&all);
        for (n, e) in &errs {
            eprintln!("error: script {n}: {e}");
        }
        if !errs.is_empty() {
            std::process::exit(2);
        }
        eprintln!("loaded {} script(s)", ferrotap::scripting::active_scripts().len());
    }

    let shared = Arc::new(Mutex::new(Shared {
        capturing: true,
        rules,
        proxy_port: opts.port,
        mode: "headless".into(),
        ..Default::default()
    }));
    let (cmd_tx, cmd_rx) = mpsc::channel::<ApiCmd>();
    let _api = match opts.api_port {
        Some(p) => {
            let token = opts.api_token.clone().unwrap_or_else(random_token);
            match ApiServer::start(p, token, shared.clone(), cmd_tx) {
                Ok(s) => {
                    eprintln!("REST API on http://127.0.0.1:{}  token: {}", s.port, s.token);
                    Some(s)
                }
                Err(e) => {
                    eprintln!("error: API: {e}");
                    std::process::exit(2);
                }
            }
        }
        None => None,
    };

    let (tx, rx) = mpsc::channel::<SessionEvent>();
    let done = Arc::new(AtomicBool::new(false));
    let failed = Arc::new(Mutex::new(None::<String>));
    {
        let (done, failed, port) = (done.clone(), failed.clone(), opts.port);
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
            if let Err(e) = rt.block_on(proxy::run(port, Some(tx))) {
                *failed.lock().unwrap() = Some(e);
            }
            done.store(true, Ordering::Release);
        });
    }

    let start = Instant::now();
    let end = opts.duration.map(|d| start + Duration::from_secs(d));
    let mut next_write = opts.every.map(|n| start + Duration::from_secs(n));
    let save = |shared: &Arc<Mutex<Shared>>| {
        if let Some(out) = &opts.out {
            let list = shared.lock().unwrap().sessions.clone();
            match auto_cli::write_capture(out, &list) {
                Ok(n) => {
                    if !opts.quiet {
                        eprintln!("wrote {n} session(s) to {}", out.display());
                    }
                }
                Err(e) => eprintln!("error: {e}"),
            }
        }
    };
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(SessionEvent::New(s)) => {
                let mut sh = shared.lock().unwrap();
                if sh.capturing {
                    if !opts.quiet {
                        println!("{}", auto_cli::summary_line(&s));
                    }
                    sh.sessions.push(s);
                }
            }
            // a streamed session grew or finished
            Ok(SessionEvent::Update(s)) => {
                let mut sh = shared.lock().unwrap();
                if let Some(r) = sh.sessions.iter_mut().find(|r| r.id == s.id) {
                    *r = s;
                }
            }
            _ => {}
        }
        while let Ok(c) = cmd_rx.try_recv() {
            // The API already updated `shared`; only the proxy's live rule set lives elsewhere.
            if let ApiCmd::SetRules(r) = c {
                ferrotap::model::set_global_rules(r);
            }
        }
        if let Some(t) = next_write {
            if Instant::now() >= t {
                save(&shared);
                next_write = opts.every.map(|n| Instant::now() + Duration::from_secs(n));
            }
        }
        if done.load(Ordering::Acquire) || end.is_some_and(|e| Instant::now() >= e) {
            break;
        }
    }
    // Pick up anything still queued.
    while let Ok(ev) = rx.try_recv() {
        let mut sh = shared.lock().unwrap();
        match ev {
            SessionEvent::New(s) if sh.capturing => sh.sessions.push(s),
            SessionEvent::Update(s) => {
                if let Some(r) = sh.sessions.iter_mut().find(|r| r.id == s.id) {
                    *r = s;
                }
            }
            _ => {}
        }
    }
    save(&shared);
    let n = shared.lock().unwrap().sessions.len();
    eprintln!("captured {n} session(s) in {:.1}s", start.elapsed().as_secs_f64());
    if let Some(e) = failed.lock().unwrap().take() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
    std::process::exit(0);
}
