//! Rules: matching conditions, plans, edits, templates, duplicate, counters, groups, old files.
use ferrotap::model::{load_rules, save_rules, KvRow, Rule, SessionRecord};
use ferrotap::rules_ext::*;

fn hdrs(v: &[(&str, &str)]) -> Vec<(String, String)> {
    v.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn cond(kind: &str, name: &str, op: &str, value: &str) -> RuleCond {
    RuleCond { enabled: true, kind: kind.into(), name: name.into(), op: op.into(), value: value.into() }
}

fn req<'a>(url: &'a str, h: &'a [(String, String)], body: &'a [u8], process: Option<&'a str>) -> ReqCtx<'a> {
    ReqCtx { method: "GET", url, headers: h, body, process }
}

#[test]
fn host_matching() {
    assert_eq!(url_host("https://Api.Example.com:8443/x?y"), "api.example.com");
    assert_eq!(url_host("http://[::1]:80/"), "::1");
    assert!(host_matches("*.example.com", "api.example.com"));
    assert!(host_matches("*.example.com", "example.com"));
    assert!(!host_matches("*.example.com", "badexample.com"));
    assert!(host_matches("=api.example.com", "API.example.com"));
    assert!(!host_matches("=example.com", "api.example.com"));
    assert!(host_matches("example", "api.example.com"));
    let r = Rule { host: "*.example.com".into(), ..Default::default() };
    let h = vec![];
    assert!(rule_matches(&r, &req("https://a.example.com/p", &h, b"", None), None));
    assert!(!rule_matches(&r, &req("https://a.example.org/p", &h, b"", None), None));
}

#[test]
fn header_cookie_body_process_conditions() {
    let h = hdrs(&[("User-Agent", "Mozilla/5.0 Test"), ("Cookie", "a=1; sid=abc123"), ("X-Num", "42")]);
    let body = b"{\"user\":\"alice\"}";
    let cx = req("http://h/api", &h, body, Some("chrome.exe"));
    let t = |c: RuleCond| cond_matches(&c, &cx, None);
    assert!(t(cond("header", "user-agent", "contains", "mozilla")));
    assert!(t(cond("header", "X-Num", "equals", "42")));
    assert!(t(cond("header", "X-Num", "regex", r"^\d+$")));
    assert!(t(cond("header", "X-Missing", "not_contains", "x")));
    assert!(!t(cond("header", "X-Missing", "exists", "")));
    assert!(t(cond("header", "X-Num", "exists", "")));
    assert!(t(cond("cookie", "sid", "equals", "abc123")));
    assert!(!t(cond("cookie", "sid", "equals", "zzz")));
    assert!(t(cond("cookie", "a", "exists", "")));
    assert!(t(cond("body", "", "contains", "alice")));
    assert!(t(cond("body", "", "regex", r#""user":"\w+""#)));
    assert!(!t(cond("body", "", "contains", "bob")));
    assert!(t(cond("process", "", "equals", "Chrome.exe")));
    assert!(!t(cond("process", "", "contains", "firefox")));
    // Disabled conditions are ignored.
    let mut off = cond("body", "", "contains", "bob");
    off.enabled = false;
    assert!(t(off));
    // No process known -> a process condition does not hold.
    let cx2 = req("http://h/api", &h, body, None);
    assert!(!cond_matches(&cond("process", "", "contains", "chrome"), &cx2, None));
    // A rule with only conditions (no URL pattern) still matches.
    let r = Rule { conds: vec![cond("cookie", "sid", "exists", "")], ..Default::default() };
    assert!(rule_matches(&r, &cx, None));
    assert!(needs_process(&[Rule { conds: vec![cond("process", "", "contains", "x")], ..Default::default() }]));
    assert!(!needs_process(&[r]));
}

#[test]
fn status_and_response_conditions() {
    assert!(status_matches("404", 404));
    assert!(status_matches("4xx", 418));
    assert!(!status_matches("4xx", 500));
    assert!(status_matches("500-599", 503));
    assert!(status_matches("301, 302", 302));
    assert!(status_matches("", 200));
    let h = vec![];
    let cx = req("http://h/a", &h, b"", None);
    let rh = hdrs(&[("Content-Type", "application/json")]);
    let resp = RespCtx { status: 503, headers: &rh, body: b"maintenance" };
    assert!(cond_matches(&cond("status", "", "", "5xx"), &cx, Some(&resp)));
    assert!(!cond_matches(&cond("status", "", "", "5xx"), &cx, None), "status needs a response");
    assert!(cond_matches(&cond("resp_header", "content-type", "contains", "json"), &cx, Some(&resp)));
    assert!(cond_matches(&cond("resp_body", "", "contains", "maint"), &cx, Some(&resp)));
    // Response rules are only evaluated by plan_response.
    let r = Rule { name: "r".into(), id: 7001, stage: "response".into(), conds: vec![cond("status", "", "", "503")], status: 200, body: "ok".into(), ..Default::default() };
    assert!(plan_request(&[r.clone()], &cx).applied.is_empty());
    let p = plan_response(&[r], &cx, &resp);
    assert_eq!(p.response.unwrap().body, b"ok");
}

#[test]
fn plans_actions_and_order() {
    let h = vec![];
    let cx = req("http://h/api/v1/users", &h, b"", None);
    // Pass-through rules combine; the first answering rule ends the list.
    let m = Rule { name: "m".into(), id: 7101, pattern: "/api/".into(), action: "modify".into(), delay_ms: 50, req_headers: vec![HeaderEdit { op: "set".into(), name: "A".into(), value: "1".into() }], ..Default::default() };
    let mr = Rule { name: "mr".into(), id: 7102, pattern: "/api/".into(), action: "map_remote".into(), target: "http://localhost:3000".into(), delay_ms: 25, ..Default::default() };
    let c = Rule { name: "c".into(), id: 7103, pattern: "/api/".into(), status: 201, body: "C".into(), ..Default::default() };
    let never = Rule { name: "never".into(), id: 7104, pattern: "/api/".into(), action: "block".into(), ..Default::default() };
    let p = plan_request(&[m.clone(), mr.clone(), c, never], &cx);
    assert_eq!(p.applied, vec!["m", "mr", "c"]);
    assert_eq!(p.delay_ms, 75);
    assert_eq!(p.new_url.as_deref(), Some("http://localhost:3000/api/v1/users"));
    assert_eq!(p.req_headers.len(), 1);
    assert_eq!(p.response.unwrap().status, 201);
    // Stop.
    let mut m2 = m.clone();
    m2.stop = true;
    let p = plan_request(&[m2, mr], &cx);
    assert_eq!(p.applied, vec!["m"]);
    assert!(p.new_url.is_none());
    // Block, close, redirect.
    let b = Rule { name: "b".into(), id: 7105, pattern: "/api/".into(), action: "block".into(), ..Default::default() };
    assert_eq!(plan_request(&[b], &cx).response.unwrap().status, 403);
    let cl = Rule { name: "cl".into(), id: 7106, pattern: "/api/".into(), action: "close".into(), ..Default::default() };
    let p = plan_request(&[cl], &cx);
    assert!(p.close && p.response.is_none());
    let rd = Rule { name: "rd".into(), id: 7107, pattern: "/api/".into(), action: "redirect".into(), status: 200, target: "https://new/".into(), ..Default::default() };
    let resp = plan_request(&[rd], &cx).response.unwrap();
    assert_eq!(resp.status, 302, "non-3xx status falls back to 302");
    assert!(resp.headers.contains(&("Location".to_string(), "https://new/".to_string())));
}

#[test]
fn map_remote_urls() {
    let r = |kind: &str, pat: &str, target: &str| Rule { match_kind: kind.into(), pattern: pat.into(), target: target.into(), action: "map_remote".into(), ..Default::default() };
    assert_eq!(map_remote_url(&r("contains", "x", "http://127.0.0.1:9000"), "https://prod.com/a/b?c=1"), "http://127.0.0.1:9000/a/b?c=1");
    assert_eq!(map_remote_url(&r("contains", "x", "https://staging.com/"), "https://prod.com/a"), "https://staging.com/a");
    assert_eq!(map_remote_url(&r("contains", "x", "https://staging.com/v2/x"), "https://prod.com/a"), "https://staging.com/v2/x");
    assert_eq!(map_remote_url(&r("regex", r"^https://prod\.com/api/(.*)$", "http://localhost:8080/$1"), "https://prod.com/api/users/1"), "http://localhost:8080/users/1");
}

#[test]
fn header_and_body_edits() {
    let mut h = hdrs(&[("Accept", "a"), ("X-Dup", "1"), ("x-dup", "2"), ("Cookie", "c")]);
    apply_header_edits(
        &mut h,
        &[
            HeaderEdit { op: "set".into(), name: "X-Dup".into(), value: "only".into() },
            HeaderEdit { op: "remove".into(), name: "cookie".into(), value: String::new() },
            HeaderEdit { op: "add".into(), name: "X-New".into(), value: "n".into() },
            HeaderEdit { op: "set".into(), name: "X-Set".into(), value: "s".into() },
        ],
    );
    assert_eq!(h, hdrs(&[("Accept", "a"), ("X-Dup", "only"), ("X-New", "n"), ("X-Set", "s")]));
    let out = apply_body_replace(b"hello world 123", &[TextReplace { find: "world".into(), replace: "there".into(), regex: false }, TextReplace { find: r"\d+".into(), replace: "N".into(), regex: true }]);
    assert_eq!(out.unwrap(), b"hello there N");
    assert!(apply_body_replace(b"abc", &[TextReplace { find: "zzz".into(), replace: "y".into(), regex: false }]).is_none());
    // Editor text formats round trip.
    let edits = header_edits_from_text("set X-A: 1\nadd X-B: two words\nremove X-C\nbogus line");
    assert_eq!(edits.len(), 3);
    assert_eq!(header_edits_from_text(&header_edits_to_text(&edits)), edits);
    let reps = replaces_from_text("foo => bar\nre:\\d+ => N\nnot a rule");
    assert_eq!(reps.len(), 2);
    assert!(reps[1].regex);
    assert_eq!(replaces_from_text(&replaces_to_text(&reps)), reps);
    // Encoded bodies are decoded for editing.
    use std::io::Write;
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(b"compressed text").unwrap();
    let enc = gz.finish().unwrap();
    assert_eq!(decode_for_edit(&enc, Some("gzip")).unwrap(), b"compressed text");
    assert!(decode_for_edit(b"x", None).is_none());
}

#[test]
fn map_local_content_type_from_extension() {
    let d = tempfile::tempdir().unwrap();
    for (name, ct) in [("a.json", "application/json"), ("b.html", "text/html; charset=utf-8"), ("c.png", "image/png"), ("d.svg", "image/svg+xml"), ("e.bin", "application/octet-stream")] {
        let p = d.path().join(name);
        std::fs::write(&p, "x").unwrap();
        let r = Rule { action: "file".into(), file: p.to_string_lossy().to_string(), ..Default::default() };
        let resp = terminal_response(&r);
        assert!(resp.headers.contains(&("Content-Type".to_string(), ct.to_string())), "{name}: {:?}", resp.headers);
    }
    let missing = Rule { action: "file".into(), file: d.path().join("nope.txt").to_string_lossy().to_string(), ..Default::default() };
    assert_eq!(terminal_response(&missing).status, 404);
}

#[test]
fn templates_fill_the_rule() {
    let names: Vec<&str> = templates().iter().map(|t| t.0).collect();
    for want in ["200 OK (JSON)", "204 No Content", "301 Moved Permanently", "302 Found", "401 Unauthorized", "403 Forbidden", "404 Not Found", "500 Internal Server Error", "502 Bad Gateway", "503 Service Unavailable", "CORS preflight OK"] {
        assert!(names.contains(&want), "missing template {want}");
    }
    let mut r = Rule { action: "file".into(), ..Default::default() };
    assert!(apply_template(&mut r, "404 Not Found"));
    assert_eq!((r.action.as_str(), r.status, r.body.as_str()), ("canned", 404, "Not Found"));
    assert!(apply_template(&mut r, "200 OK (JSON)"));
    assert_eq!(terminal_response(&r).headers, vec![("Content-Type".to_string(), "application/json".to_string())]);
    let mut c = Rule::default();
    assert!(apply_template(&mut c, "CORS preflight OK"));
    assert_eq!(c.status, 204);
    assert_eq!(c.method, "OPTIONS");
    assert!(c.headers.iter().any(|h| h.key == "Access-Control-Allow-Origin"));
    assert!(!apply_template(&mut c, "no such template"));
}

#[test]
fn duplicate_counters_and_groups() {
    let a = Rule { name: "a".into(), id: 7201, pattern: "/x".into(), group: "g1".into(), ..Default::default() };
    let d = duplicate_rule(&a);
    assert_ne!(d.id, a.id);
    assert_eq!(d.name, "a (copy)");
    assert_eq!(d.pattern, a.pattern);
    assert_eq!(d.group, "g1");

    let h = vec![];
    let cx = req("http://h/x", &h, b"", None);
    assert_eq!(rule_hits(7201), 0);
    plan_request(&[a.clone()], &cx);
    plan_request(&[a.clone()], &cx);
    assert_eq!(rule_hits(7201), 2);
    reset_hits(7201);
    assert_eq!(rule_hits(7201), 0);

    let b = Rule { name: "b".into(), id: 7202, pattern: "/x".into(), status: 202, ..Default::default() };
    let mut rules = vec![a, d, b];
    assert_eq!(group_names(&rules), vec![String::new(), "g1".to_string()]); // ungrouped first
    set_group_enabled(&mut rules, "g1", false);
    assert!(!group_enabled(&rules, "g1"));
    assert!(rules[0].enabled, "rule's own flag is kept");
    assert_eq!(plan_request(&rules, &cx).applied, vec!["b"]);
    set_group_enabled(&mut rules, "g1", true);
    assert_eq!(plan_request(&rules, &cx).applied, vec!["a"]);

    // ensure_ids fills zero and duplicated ids.
    let mut v = vec![Rule::default(), Rule { id: 5, ..Default::default() }, Rule { id: 5, ..Default::default() }];
    ensure_ids(&mut v);
    assert!(v[0].id != 0 && v[1].id == 5 && v[2].id != 5 && v[0].id != v[2].id);
}

#[test]
fn old_rules_file_still_loads_and_new_fields_round_trip() {
    let d = tempfile::tempdir().unwrap();
    let old = r#"[{"name":"legacy","enabled":true,"match_kind":"regex","pattern":"/api/\\d+","method":"GET","action":"canned","status":201,"headers":[{"enabled":true,"key":"X-Old","value":"1"}],"body":"old","file":""},{"pattern":"/min"}]"#;
    std::fs::write(d.path().join("rules.json"), old).unwrap();
    let rs = load_rules(d.path());
    assert_eq!(rs.len(), 2);
    assert_eq!(rs[0].name, "legacy");
    assert_eq!(rs[0].status, 201);
    assert!(rs[0].conds.is_empty() && rs[0].group.is_empty() && rs[0].delay_ms == 0 && !rs[0].group_off);
    assert!(rs[0].id != 0 && rs[1].id != 0 && rs[0].id != rs[1].id);
    assert!(rs[1].enabled);
    assert_eq!(rs[1].status, 200);
    let h = vec![];
    assert_eq!(plan_request(&rs, &req("http://h/api/12", &h, b"", None)).response.unwrap().body, b"old");

    let mut n = rs.clone();
    n[0].conds = vec![cond("cookie", "sid", "exists", "")];
    n[0].resp_body = vec![TextReplace { find: "a".into(), replace: "b".into(), regex: true }];
    n[0].req_headers = vec![HeaderEdit { op: "add".into(), name: "X".into(), value: "1".into() }];
    n[0].group = "G".into();
    n[0].stage = "response".into();
    n[0].target = "http://t".into();
    n[0].delay_ms = 10;
    n[0].stop = true;
    let d2 = tempfile::tempdir().unwrap();
    save_rules(d2.path(), &n).unwrap();
    assert_eq!(load_rules(d2.path()), n);
}

#[test]
fn rule_from_selected_session() {
    let s = SessionRecord {
        id: 3,
        method: "POST".into(),
        url: "https://api.example.com/v1/items?id=1".into(),
        status: 404,
        response_headers: [("content-type".to_string(), "application/json".to_string()), ("content-length".to_string(), "9".to_string())].into_iter().collect(),
        response_body: "{\"e\": 1}".into(),
        ..Default::default()
    };
    let r = rule_from_session(&s);
    assert_eq!(r.method, "POST");
    assert_eq!(r.status, 404);
    assert_eq!(r.body, "{\"e\": 1}");
    assert!(r.headers.iter().any(|h: &KvRow| h.key == "content-type"));
    assert!(!r.headers.iter().any(|h| h.key == "content-length"));
    let h = vec![];
    let hit = ReqCtx { method: "POST", url: &s.url, headers: &h, body: b"", process: None };
    assert!(rule_matches(&r, &hit, None));
    let miss = ReqCtx { method: "POST", url: "https://api.example.com/v1/items?id=12", headers: &h, body: b"", process: None };
    assert!(!rule_matches(&r, &miss, None));
    assert!(rule_summary(&r).contains("POST"));
}

#[test]
fn glob_url_matching() {
    // glob_match: whole-string, case-insensitive, * and ?.
    assert!(glob_match("*/api/*", "https://h/api/users"));
    assert!(glob_match("https://*.example.com/*", "https://cdn.EXAMPLE.com/a.js"));
    assert!(glob_match("*.png", "https://h/img/logo.PNG"));
    assert!(glob_match("a?c", "abc"));
    assert!(!glob_match("a?c", "ac"));
    assert!(!glob_match("*/api/*", "https://h/web/users"));
    assert!(glob_match("*", "anything"));
    assert!(glob_match("https://h/x", "https://h/x")); // literal, no wildcard

    // rule_matches honours match_kind == "glob".
    let h = hdrs(&[]);
    let mut r = Rule { match_kind: "glob".into(), pattern: "*/api/*".into(), ..Default::default() };
    r.enabled = true;
    assert!(rule_matches(&r, &req("https://h/api/v1", &h, b"", None), None));
    assert!(!rule_matches(&r, &req("https://h/static/app.js", &h, b"", None), None));
}
