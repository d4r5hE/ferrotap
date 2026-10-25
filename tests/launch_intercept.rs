//! G13 interception launchers: browser command lines, Firefox profile, runtime environment,
//! starting a program with capture on and tracking it; the capture filter.
use ferrotap::conn::{CaptureMode, ConnSettings};
use ferrotap::launch::*;
use ferrotap::model::SessionRecord;

#[test]
fn browser_command_lines() {
    let dir = tempfile::tempdir().unwrap();
    let (args, prof) = browser_cmd(std::path::Path::new("C:/x/firefox.exe"), BrowserKind::Firefox, "127.0.0.1:9000", Some(dir.path()), Some("https://e.test/")).unwrap();
    assert_eq!(prof, dir.path());
    assert!(args.contains(&"-profile".to_string()) && args.contains(&"-no-remote".to_string()));
    assert_eq!(args.last().unwrap(), "https://e.test/");
    let js = std::fs::read_to_string(dir.path().join("user.js")).unwrap();
    assert!(js.contains("user_pref(\"network.proxy.http\", \"127.0.0.1\");"));
    assert!(js.contains("user_pref(\"network.proxy.ssl_port\", 9000);"));
    assert!(js.contains("security.enterprise_roots.enabled"));
    let (args, _) = browser_cmd(std::path::Path::new("C:/x/chrome.exe"), BrowserKind::Chromium, "127.0.0.1:9000", Some(dir.path()), None).unwrap();
    assert!(args.iter().any(|a| a == "--proxy-server=http://127.0.0.1:9000"));
    assert_eq!(kind_of(std::path::Path::new("C:/Mozilla Firefox/firefox.exe")), BrowserKind::Firefox);
    assert_eq!(kind_of(std::path::Path::new("msedge.exe")), BrowserKind::Chromium);
    for b in detect_browsers() {
        assert!(b.exe.is_file());
    }
    assert!(detect_terminals().iter().any(|(id, _)| *id == "powershell"));
}

#[test]
fn runtime_environment_and_args() {
    let e = app_env("127.0.0.1:9000", true, std::path::Path::new("C:/ca.pem"));
    assert_eq!(e["HTTPS_PROXY"], "http://127.0.0.1:9000");
    assert_eq!(e["NODE_USE_ENV_PROXY"], "1");
    assert_eq!(e["NODE_EXTRA_CA_CERTS"], "C:/ca.pem");
    assert_eq!(e["REQUESTS_CA_BUNDLE"], "C:/ca.pem");
    assert!(e["JAVA_TOOL_OPTIONS"].contains("-Dhttps.proxyPort=9000"));
    assert_eq!(e["NO_PROXY"], "");
    let e2 = app_env("127.0.0.1:9000", false, std::path::Path::new("C:/ca.pem"));
    assert!(!e2.contains_key("NODE_EXTRA_CA_CERTS"));
    assert_eq!(split_args(r#"-a "two words" b"#), vec!["-a", "two words", "b"]);
    assert_eq!(split_args(r#""" x"#), vec!["", "x"]);
    assert!(split_args("   ").is_empty());
}

#[cfg(windows)]
#[test]
fn program_runs_with_capture_env() {
    let dir = tempfile::tempdir().unwrap();
    let mut inst = launch_program("cmd.exe", "/c echo %HTTPS_PROXY%^|%NODE_EXTRA_CA_CERTS%>env.txt& exit 3", &dir.path().display().to_string(), "127.0.0.1:9123", true).unwrap();
    assert!(inst.pid > 0);
    let t = std::time::Instant::now();
    while inst.alive() {
        assert!(t.elapsed() < std::time::Duration::from_secs(15), "program did not finish");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert_eq!(inst.exit.as_deref(), Some("exited (3)"));
    let out = std::fs::read_to_string(dir.path().join("env.txt")).unwrap();
    assert!(out.starts_with("http://127.0.0.1:9123|"), "{out}");
    assert!(out.trim_end().ends_with("ferrotap-ca.pem"), "{out}");
    assert!(launch_program("  ", "", "", "x:1", false).is_err());
}

#[test]
fn capture_filter() {
    let rec = |proc_: Option<&str>, url: &str| SessionRecord { process_name: proc_.map(str::to_string), url: url.into(), ..Default::default() };
    let mut s = ConnSettings::default();
    assert!(s.should_capture(&rec(None, "http://a/")));
    s.capture_mode = CaptureMode::Only;
    s.capture_processes = vec!["chrome".into()];
    s.capture_ports = vec![8080];
    assert!(s.should_capture(&rec(Some("chrome.exe"), "https://a/")));
    assert!(s.should_capture(&rec(Some("CHROME.EXE"), "https://a/")));
    assert!(!s.should_capture(&rec(Some("node.exe"), "https://a/")));
    assert!(s.should_capture(&rec(None, "http://a:8080/x")));
    assert!(!s.should_capture(&rec(None, "http://a/x")), "port 80 is not listed");
    s.capture_mode = CaptureMode::Except;
    assert!(!s.should_capture(&rec(Some("chrome.exe"), "https://a/")));
    assert!(s.should_capture(&rec(Some("node.exe"), "https://a/")));
    s.system_bypass = vec!["<local>".into(), "*.corp".into(), " ".into()];
    assert_eq!(s.system_override(), "<local>;*.corp");
    s.system_bypass.clear();
    assert_eq!(s.system_override(), "<local>");
}

#[test]
fn wsl_and_docker_commands() {
    let s = wsl_script(8897, r"C:\Projects\ca\ferrotap-ca.pem");
    assert!(s.contains("/dev/tcp/127.0.0.1/8897"));
    assert!(s.contains("export HTTP_PROXY=$P HTTPS_PROXY=$P"));
    assert!(s.contains(r"wslpath 'C:\Projects\ca\ferrotap-ca.pem'"));
    assert!(s.ends_with("exec bash -i"));
    std::fs::write(std::env::temp_dir().join("ferrotap_wsl_script.sh"), &s).unwrap();
    let d = docker_command(9000, r"C:\ca\ferrotap-ca.pem", "node:22");
    assert!(d.starts_with("docker run --rm -it -e HTTP_PROXY=http://host.docker.internal:9000"));
    assert!(d.contains(r#"-v "C:\ca\ferrotap-ca.pem:/usr/local/share/ca-certificates/ferrotap.crt:ro""#));
    assert!(d.ends_with(" node:22"));
    assert!(docker_command(1, "x", " ").ends_with(" IMAGE"));
}
