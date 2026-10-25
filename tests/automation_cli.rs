//! G16 headless mode: option parsing, summary lines, capture files, rules files, and an end-to-end run of
//! the `ferrotap_proxy` binary on random localhost ports (local origin server, no internet, no system proxy).
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use ferrotap::auto_cli::{load_rules_file, summary_line, write_capture, CliOptions};
use ferrotap::model::SessionRecord;

#[test]
fn parse_defaults_and_all_options() {
    let o = CliOptions::parse(Vec::<String>::new()).unwrap();
    assert_eq!(o.port, ferrotap::proxy::PORT);
    assert!(o.out.is_none() && o.rules.is_none() && o.duration.is_none() && !o.quiet && o.api_port.is_none());

    let o = CliOptions::parse([
        "--port", "9000", "--out", "cap.har", "--every", "5", "--rules", "r.json", "--duration", "30", "--quiet", "--api-port=0", "--api-token", "abc",
    ])
    .unwrap();
    assert_eq!(o.port, 9000);
    assert_eq!(o.out, Some(PathBuf::from("cap.har")));
    assert_eq!(o.every, Some(5));
    assert_eq!(o.rules, Some(PathBuf::from("r.json")));
    assert_eq!(o.duration, Some(30));
    assert!(o.quiet);
    assert_eq!(o.api_port, Some(0));
    assert_eq!(o.api_token.as_deref(), Some("abc"));
    assert!(CliOptions::parse(["-h"]).unwrap().help);
}

#[test]
fn parse_errors() {
    assert!(CliOptions::parse(["--port"]).is_err());
    assert!(CliOptions::parse(["--port", "abc"]).is_err());
    assert!(CliOptions::parse(["--port", "70000"]).is_err());
    assert!(CliOptions::parse(["--port", "0"]).is_err());
    assert!(CliOptions::parse(["--every", "5"]).is_err(), "--every without --out");
    assert!(CliOptions::parse(["--bogus"]).is_err());
}

fn rec(id: usize, url: &str, status: u16, body: &str) -> SessionRecord {
    let mut r = SessionRecord { id, method: "GET".into(), url: url.into(), status, response_body: body.into(), duration_ms: 42, ..Default::default() };
    r.response_headers.insert("Content-Type".into(), "text/html; charset=utf-8".into());
    r
}

#[test]
fn summary_line_format() {
    assert_eq!(summary_line(&rec(3, "https://a.test/x", 200, "hello")), "#3 GET 200 https://a.test/x 42ms 5B text/html");
    assert!(summary_line(&rec(4, "https://a.test/y", 404, &"z".repeat(2048))).contains(" 2.0KB "));
}

#[test]
fn write_capture_formats() {
    let d = tempfile::tempdir().unwrap();
    let v = vec![rec(1, "https://a.test/1", 200, "a"), rec(2, "https://b.test/2", 500, "b")];
    let har = d.path().join("sub").join("c.har");
    assert_eq!(write_capture(&har, &v).unwrap(), 2);
    let back = ferrotap::har::import_har(&har).unwrap();
    assert_eq!(back.len(), 2);
    assert_eq!(back[1].status, 500);
    let ftap = d.path().join("c.ftap");
    write_capture(&ftap, &v).unwrap();
    assert_eq!(ferrotap::data_io::import_ftap(&ftap).unwrap().len(), 2);
    let js = d.path().join("c.json");
    write_capture(&js, &v).unwrap();
    let parsed: Vec<SessionRecord> = serde_json::from_str(&std::fs::read_to_string(&js).unwrap()).unwrap();
    assert_eq!(parsed[0].url, "https://a.test/1");
}

#[test]
fn rules_file_array_or_single() {
    let d = tempfile::tempdir().unwrap();
    let a = d.path().join("a.json");
    std::fs::write(&a, r#"[{"name":"one","pattern":"/x","body":"X"},{"name":"two","pattern":"/y","status":404}]"#).unwrap();
    let r = load_rules_file(&a).unwrap();
    assert_eq!(r.len(), 2);
    assert_eq!(r[1].status, 404);
    assert!(r[0].enabled && r[0].matches("GET", "http://h.test/x"));
    let s = d.path().join("s.json");
    std::fs::write(&s, r#"{"pattern":"/z"}"#).unwrap();
    assert_eq!(load_rules_file(&s).unwrap().len(), 1);
    std::fs::write(&s, "not json").unwrap();
    assert!(load_rules_file(&s).is_err());
    assert!(load_rules_file(&d.path().join("missing.json")).is_err());
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn via_proxy(proxy: u16, url: &str, host: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", proxy)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(15))).unwrap();
    write!(s, "GET {url} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").unwrap();
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    out
}

#[test]
fn headless_binary_end_to_end() {
    // Local origin server.
    let origin = TcpListener::bind("127.0.0.1:0").unwrap();
    let oport = origin.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for c in origin.incoming().take(10) {
            let Ok(mut c) = c else { continue };
            let mut buf = [0u8; 4096];
            let _ = c.read(&mut buf);
            let _ = c.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello");
        }
    });
    let d = tempfile::tempdir().unwrap();
    let rules = d.path().join("rules.json");
    std::fs::write(&rules, r#"[{"name":"mock","pattern":"/mocked","status":201,"body":"MOCK"}]"#).unwrap();
    let out = d.path().join("cap.har");
    let (pport, aport) = (free_port(), free_port());
    let child = Command::new(env!("CARGO_BIN_EXE_ferrotap_proxy"))
        .args(["--port", &pport.to_string(), "--out", out.to_str().unwrap(), "--rules", rules.to_str().unwrap()])
        .args(["--duration", "12", "--api-port", &aport.to_string(), "--api-token", "t0ken"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let t0 = Instant::now();
    while TcpStream::connect(("127.0.0.1", pport)).is_err() {
        assert!(t0.elapsed() < Duration::from_secs(10), "proxy did not start");
        std::thread::sleep(Duration::from_millis(100));
    }
    let host = format!("127.0.0.1:{oport}");
    let r1 = via_proxy(pport, &format!("http://{host}/hello"), &host);
    assert!(r1.contains("200") && r1.ends_with("hello"), "{r1}");
    let r2 = via_proxy(pport, &format!("http://{host}/mocked"), &host);
    assert!(r2.contains("201") && r2.ends_with("MOCK"), "rule from --rules applies: {r2}");

    // REST API on the headless run.
    std::thread::sleep(Duration::from_millis(500));
    let (st, body) = ferrotap::auto_api::http_call(aport, "t0ken", "GET", "/status", None).unwrap();
    assert_eq!(st, 200, "{body}");
    assert!(body.contains("\"headless\""));
    let (st, body) = ferrotap::auto_api::http_call(aport, "t0ken", "GET", "/sessions?host=127.0.0.1", None).unwrap();
    assert_eq!(st, 200);
    assert!(body.contains("/hello"), "{body}");
    assert_eq!(ferrotap::auto_api::http_call(aport, "wrong", "GET", "/status", None).unwrap().0, 401);

    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(stdout.lines().any(|l| l.starts_with('#') && l.contains("GET 200") && l.contains("/hello")), "{stdout}");
    let har = ferrotap::har::import_har(&out).unwrap();
    assert!(har.iter().any(|s| s.url.ends_with("/hello") && s.status == 200));
    assert!(har.iter().any(|s| s.url.ends_with("/mocked") && s.status == 201));
}
