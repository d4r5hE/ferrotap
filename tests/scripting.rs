//! G9 scripts (Rhai) and the plugin folder: hooks, sandbox limits, diagnostics, columns, inspectors.
//! One test function: the active script set is process-wide.
use ferrotap::model::SessionRecord;
use ferrotap::scripting::*;

#[test]
fn scripts_hooks_sandbox_and_plugins() {
    // validation
    let h = validate("fn on_request(req) { req }\nfn column(s) { \"x\" }").unwrap();
    assert!(h.on_request && h.column && !h.on_response && !h.inspect);
    let err = validate("fn on_request(req) { req. }").unwrap_err();
    assert!(err.contains("line"), "{err}");

    let scripts = vec![
        Script {
            name: "tag".into(),
            enabled: true,
            code: r#"
fn on_request(req) {
    req.headers["x-debug"] = "1";
    if req.url.contains("/old/") { req.url.replace("/old/", "/new/"); }
    req
}
fn on_response(req, resp) {
    if resp.status == 500 { log("server error at " + req.url); resp.status = 503; }
    resp.headers["x-scripted"] = "yes";
    resp.body.replace("secret", "*****");
    let oops = (); resp.method_typo = oops;
    resp
}
fn column(s) { s.method + " " + s.status }
fn inspect(s) { "length " + s.response_body.len() }
"#
            .into(),
        },
        Script { name: "off".into(), enabled: false, code: "fn on_request(req) { req.method = \"DELETE\"; req }".into() },
        Script { name: "spin".into(), enabled: true, code: "fn on_request(req) { loop { } }".into() },
        Script { name: "broken".into(), enabled: true, code: "fn on_request(req) { ".into() },
    ];
    let errs = set_active(&scripts);
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].0, "broken");
    assert!(has_hook(|h| h.on_request));

    clear_diagnostics();
    let mut r = ReqParts { method: "GET".into(), url: "https://a.test/old/x".into(), headers: vec![("Accept".into(), "*/*".into())], body: vec![0xff, 0x01] };
    let t = std::time::Instant::now();
    let changed = run_request(&mut r);
    assert!(t.elapsed() < std::time::Duration::from_secs(5), "the infinite loop was stopped");
    assert_eq!(changed, vec!["tag"]);
    assert_eq!(r.method, "GET", "disabled script did not run");
    assert_eq!(r.url, "https://a.test/new/x");
    assert!(r.headers.contains(&("x-debug".to_string(), "1".to_string())));
    assert!(r.headers.contains(&("accept".to_string(), "*/*".to_string())));
    assert_eq!(r.body, vec![0xff, 0x01], "an unchanged binary body stays byte-exact");
    assert!(diagnostics().iter().any(|d| d.contains("spin") && d.contains("on_request failed")), "{:?}", diagnostics());

    let mut resp = RespParts { status: 500, headers: vec![], body: b"the secret value".to_vec() };
    assert_eq!(run_response(&r, &mut resp), vec!["tag"]);
    assert_eq!(resp.status, 503);
    assert_eq!(resp.body, b"the ***** value");
    assert!(resp.headers.contains(&("x-scripted".to_string(), "yes".to_string())));
    assert!(diagnostics().iter().any(|d| d.contains("log: server error at https://a.test/new/x")));

    let s = SessionRecord { id: 1, method: "POST".into(), url: "https://a.test/".into(), status: 201, response_body: "abcd".into(), ..Default::default() };
    assert_eq!(column_scripts(), vec!["tag"]);
    assert_eq!(call_session_fn("tag", "column", &s).unwrap(), "POST 201");
    assert_eq!(inspect_all(&s), vec![("tag".to_string(), "length 4".to_string())]);
    assert!(call_session_fn("nope", "column", &s).is_err());

    // files and the plugin folder
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("scripts.json");
    save_scripts(&p, &scripts).unwrap();
    assert_eq!(load_scripts(&p), scripts);
    let plug = dir.path().join("plugins");
    std::fs::create_dir(&plug).unwrap();
    std::fs::write(plug.join("b_size.rhai"), "fn column(s) { s.response_body.len().to_string() }").unwrap();
    std::fs::write(plug.join("a_note.rhai"), "fn inspect(s) { \"hi\" }").unwrap();
    std::fs::write(plug.join("readme.txt"), "ignored").unwrap();
    let plugins = load_plugins(&plug);
    assert_eq!(plugins.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["plugin: a_note", "plugin: b_size"]);
    let cli = load_cli(&plug.join("b_size.rhai")).unwrap();
    assert_eq!(cli[0].name, "b_size");
    assert!(load_cli(&p).unwrap().len() == 4);
    std::fs::write(dir.path().join("bad.rhai"), "fn (").unwrap();
    assert!(load_cli(&dir.path().join("bad.rhai")).is_err());

    set_active(&[]);
    assert!(!has_hook(|h| h.on_request));
}
