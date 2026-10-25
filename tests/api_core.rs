//! G15: environments/variables, assertions, code generation, GraphQL detection, raw headers.
use ferrotap::api_tools::*;
use ferrotap::model::{ComposerRequest, ComposerResponse, KvRow, SavedRequest};

fn vars() -> Vec<(String, String)> {
    vec![("baseUrl".into(), "http://127.0.0.1:9000".into()), ("token".into(), "abc".into()), ("id".into(), "42".into()), ("nested".into(), "{{baseUrl}}/n".into())]
}

#[test]
fn substitution_in_url_params_headers_body() {
    let v = vars();
    assert_eq!(substitute("{{baseUrl}}/users/{{ id }}", &v), "http://127.0.0.1:9000/users/42");
    assert_eq!(substitute("{{unknown}} stays", &v), "{{unknown}} stays");
    assert_eq!(substitute("{{nested}}", &v), "http://127.0.0.1:9000/n");
    assert_eq!(substitute("open {{ only", &v), "open {{ only");
    assert_eq!(variables_in("{{a}} x {{ b }} {{a}}"), vec!["a".to_string(), "b".to_string()]);
    let r = SavedRequest {
        method: "POST".into(), url: "{{baseUrl}}/u/{{id}}".into(),
        params: vec![KvRow::new("t", "{{token}}")], headers: vec![KvRow::new("Authorization", "Bearer {{token}}")],
        body: "{\"id\": {{id}}}".into(), ..Default::default()
    };
    let c = prepare(&r, &v);
    assert_eq!(c.url, "http://127.0.0.1:9000/u/42?t=abc");
    assert_eq!(c.headers, vec![("Authorization".to_string(), "Bearer abc".to_string())]);
    assert_eq!(c.body, "{\"id\": 42}");
}

#[test]
fn environments_persist_and_active_vars() {
    let d = tempfile::tempdir().unwrap();
    let mut s = EnvStore::default();
    s.upsert(Environment { name: "dev".into(), vars: vec![KvRow::new("baseUrl", "http://dev"), KvRow { enabled: false, key: "off".into(), value: "x".into() }] });
    s.upsert(Environment { name: "prod".into(), vars: vec![KvRow::new("baseUrl", "http://prod")] });
    s.active = "prod".into();
    save_envs(d.path(), &s).unwrap();
    let l = load_envs(d.path());
    assert_eq!(l, s);
    assert_eq!(l.active_vars(), vec![("baseUrl".to_string(), "http://prod".to_string())]);
    let mut l2 = l.clone();
    l2.active = "dev".into();
    assert_eq!(l2.active_vars().len(), 1, "disabled variable is skipped");
    // the environments file does not show up as a collection
    assert!(ferrotap::model::load_collections(d.path()).is_empty());
    // settings persist too
    let st = ComposerSettings { follow_redirects: false, timeout_secs: 7, via_proxy: false, http10: false };
    save_settings(d.path(), &st).unwrap();
    assert_eq!(load_settings(d.path()), st);
}

#[test]
fn old_saved_requests_still_load() {
    let old = r#"{"name":"C","requests":[{"name":"r","method":"GET","url":"http://h/","params":[],"headers":[],"body":""}]}"#;
    let c: ferrotap::model::Collection = serde_json::from_str(old).unwrap();
    assert!(c.requests[0].tests.is_empty() && c.requests[0].example.is_none() && c.requests[0].body_mode.is_empty());
}

fn resp() -> ComposerResponse {
    ComposerResponse {
        status: 201, reason: "Created".into(),
        headers: vec![("Content-Type".into(), "application/json".into()), ("X-Req".into(), "7".into())],
        body: r#"{"data":{"id":5,"name":"Ann","tags":["a","b"],"ok":true}}"#.into(),
        elapsed_ms: 120, error: String::new(),
    }
}

#[test]
fn each_assertion_kind() {
    let r = resp();
    let ok = |a: Assertion| assert!(check(&a, &r).passed, "{:?} should pass: {:?}", a, check(&a, &r));
    let bad = |a: Assertion| assert!(!check(&a, &r).passed, "{:?} should fail", a);
    ok(Assertion::new(AssertKind::StatusEquals, "", "201"));
    ok(Assertion::new(AssertKind::StatusEquals, "", "2xx"));
    ok(Assertion::new(AssertKind::StatusEquals, "", "200-299"));
    ok(Assertion::new(AssertKind::StatusEquals, "", "201,204"));
    bad(Assertion::new(AssertKind::StatusEquals, "", "200"));
    bad(Assertion::new(AssertKind::StatusEquals, "", "")); // empty spec never passes
    ok(Assertion::new(AssertKind::HeaderExists, "content-type", ""));
    bad(Assertion::new(AssertKind::HeaderExists, "X-None", ""));
    ok(Assertion::new(AssertKind::HeaderEquals, "x-req", "7"));
    bad(Assertion::new(AssertKind::HeaderEquals, "x-req", "8"));
    ok(Assertion::new(AssertKind::JsonPathEquals, "$.data.id", "5"));
    ok(Assertion::new(AssertKind::JsonPathEquals, "data.name", "Ann"));
    ok(Assertion::new(AssertKind::JsonPathEquals, "$.data.tags[1]", "b"));
    ok(Assertion::new(AssertKind::JsonPathEquals, "$.data.ok", "true"));
    bad(Assertion::new(AssertKind::JsonPathEquals, "$.data.id", "6"));
    ok(Assertion::new(AssertKind::JsonPathExists, "$.data.tags.0", ""));
    bad(Assertion::new(AssertKind::JsonPathExists, "$.data.missing", ""));
    ok(Assertion::new(AssertKind::BodyContains, "", "\"Ann\""));
    bad(Assertion::new(AssertKind::BodyContains, "", "Bob"));
    ok(Assertion::new(AssertKind::BodyMatches, "", "\"name\"\\s*:\\s*\"An.\""));
    bad(Assertion::new(AssertKind::BodyMatches, "", "\"name\":\"Bob\""));
    bad(Assertion::new(AssertKind::BodyMatches, "", "(")); // bad regex -> fail, no panic
    ok(Assertion::new(AssertKind::TimeBelow, "", "500"));
    bad(Assertion::new(AssertKind::TimeBelow, "", "100"));
    let mut off = Assertion::new(AssertKind::StatusEquals, "", "999");
    off.enabled = false;
    let rs = run_tests(&[off, Assertion::new(AssertKind::StatusEquals, "", "201")], &r);
    assert_eq!(rs.len(), 1);
    let err = ComposerResponse { error: "boom".into(), ..Default::default() };
    assert!(!check(&Assertion::new(AssertKind::BodyContains, "", ""), &err).passed);
}

#[test]
fn tests_saved_with_request() {
    let d = tempfile::tempdir().unwrap();
    let mut c = ferrotap::model::Collection::new("T");
    c.upsert(SavedRequest { name: "r".into(), method: "GET".into(), url: "http://h/".into(), tests: vec![Assertion::new(AssertKind::JsonPathEquals, "$.a", "1")], ..Default::default() });
    ferrotap::model::save_collection(d.path(), &c).unwrap();
    let l = ferrotap::model::load_collections(d.path());
    assert_eq!(l[0].requests[0].tests, c.requests[0].tests);
}

#[test]
fn code_generators_include_method_url_headers_body() {
    let req = ComposerRequest { method: "post".into(), url: "http://api.local/items?x=1".into(), headers: vec![("Content-Type".into(), "application/json".into()), ("X-Token".into(), "t\"q".into())], body: "{\"a\":\"line\\n\"}".into() };
    let s = ferrotap::codegen::session_from_request(&req);
    for lang in ferrotap::codegen::LANGUAGES {
        let code = ferrotap::codegen::generate(lang, &s);
        assert!(code.contains("http://api.local/items?x=1"), "{lang}: url\n{code}");
        assert!(code.contains("X-Token"), "{lang}: header\n{code}");
        assert!(code.to_lowercase().contains("post"), "{lang}: method\n{code}");
        assert!(code.contains("line"), "{lang}: body\n{code}");
    }
    let cs = ferrotap::codegen::to_csharp(&s);
    assert!(cs.contains("new HttpMethod(\"POST\")") && cs.contains("StringContent(") && cs.contains("\"application/json\"") && cs.contains("t\\\"q"));
    let go = ferrotap::codegen::to_go(&s);
    assert!(go.contains("http.NewRequest(\"POST\", \"http://api.local/items?x=1\", strings.NewReader(") && go.contains("req.Header.Set(\"X-Token\""));
    let rs = ferrotap::codegen::to_rust(&s);
    assert!(rs.contains("reqwest::Method::from_bytes(\"POST\"") && rs.contains(".header(\"Content-Type\", \"application/json\")") && rs.contains(".body("));
    let get = ferrotap::codegen::session_from_request(&ComposerRequest { method: "GET".into(), url: "http://h/".into(), ..Default::default() });
    assert!(!ferrotap::codegen::to_go(&get).contains("strings"));
}

#[test]
fn curl_uses_safe_shell_quoting() {
    let req = ComposerRequest {
        method: "POST".into(),
        url: "http://h/p?a=$x&b=1".into(),
        headers: vec![("X-Q".into(), "it's $HOME".into())],
        body: "@file 'q' $x `id`\nline2".into(),
    };
    let s = ferrotap::codegen::session_from_request(&req);
    let c = ferrotap::codegen::to_curl(&s);
    // Body goes through --data-raw (not --data, which strips newlines and reads '@file' as a file),
    // single-quoted, with embedded ' escaped as the POSIX '\'' sequence.
    assert!(c.contains("--data-raw '@file '\\''q'\\'' $x `id`\nline2'"), "{c}");
    assert!(!c.contains(" --data '"), "must not use the newline-stripping --data form:\n{c}");
    // Header value's single quote escaped, $ left literal inside single quotes.
    assert!(c.contains("-H 'X-Q: it'\\''s $HOME'"), "{c}");
    // URL single-quoted so $ and & are literal, not shell-expanded or a background operator.
    assert!(c.contains(" 'http://h/p?a=$x&b=1'"), "{c}");
}

#[test]
fn go_and_rust_skip_auto_headers() {
    let req = ComposerRequest {
        method: "POST".into(),
        url: "http://api.local/x".into(),
        headers: vec![
            ("Host".into(), "api.local".into()),
            ("Content-Length".into(), "7".into()),
            ("Content-Type".into(), "application/json".into()),
            ("X-Token".into(), "abc".into()),
        ],
        body: "{\"a\":1}".into(),
    };
    let s = ferrotap::codegen::session_from_request(&req);
    for code in [ferrotap::codegen::to_go(&s), ferrotap::codegen::to_rust(&s)] {
        assert!(!code.to_lowercase().contains("host"), "Host should be auto-set:\n{code}");
        assert!(!code.to_lowercase().contains("content-length"), "Content-Length should be auto-set:\n{code}");
        // Content-Type and custom headers are kept.
        assert!(code.contains("Content-Type") && code.contains("application/json"), "{code}");
        assert!(code.contains("X-Token") && code.contains("abc"), "{code}");
    }
}

#[test]
fn python_and_fetch_escape_cr_and_tab() {
    let req = ComposerRequest {
        method: "POST".into(),
        url: "http://h/p".into(),
        headers: vec![("X-H".into(), "a\tb".into())],
        body: "line1\r\ntab\tend\\z".into(),
    };
    let s = ferrotap::codegen::session_from_request(&req);
    let py = ferrotap::codegen::to_python(&s);
    let js = ferrotap::codegen::to_fetch(&s);
    for code in [&py, &js] {
        // A raw CR or newline inside a "..." literal is a syntax error in both languages.
        assert!(!code.contains('\r'), "raw CR leaked:\n{code}");
        assert!(code.contains("line1\\r\\ntab\\tend\\\\z"), "body not escaped:\n{code}");
        assert!(code.contains("\"X-H\": \"a\\tb\""), "header tab not escaped:\n{code}");
    }
}

#[test]
fn powershell_content_type_and_escaping() {
    let req = ComposerRequest {
        method: "POST".into(),
        url: "http://h/p".into(),
        headers: vec![("Content-Type".into(), "application/json".into()), ("X-Token".into(), "a$b`c".into())],
        body: "{\"p\":\"$x\"}".into(),
    };
    let s = ferrotap::codegen::session_from_request(&req);
    let ps = ferrotap::codegen::to_powershell(&s);
    // Content-Type must NOT sit in the -Headers hashtable (Invoke-RestMethod rejects it there)...
    assert!(!ps.contains("\"Content-Type\" ="), "Content-Type left in -Headers:\n{ps}");
    // ...it goes to -ContentType instead.
    assert!(ps.contains("-ContentType \"application/json\""), "{ps}");
    // $ and backtick escaped so PowerShell treats them literally, not as expansion/escape.
    assert!(ps.contains("`$x"), "body $ not escaped:\n{ps}");
    assert!(ps.contains("a`$b``c"), "header $/backtick not escaped:\n{ps}");
}

#[test]
fn graphql_detection_and_body() {
    let body = r#"{"query":"query GetUser($id: ID!) { user(id: $id) { name } }","variables":{"id":"1"},"operationName":"GetUser"}"#;
    let g = detect_graphql("POST", "http://h/graphql", &[], body).unwrap();
    assert!(g.query.starts_with("query GetUser"));
    assert!(g.variables.contains("\"id\": \"1\""));
    assert_eq!(g.operation_name, "GetUser");
    assert_eq!(graphql_operation(&g.query), ("query".to_string(), "GetUser".to_string()));
    assert_eq!(graphql_operation("mutation { x }").0, "mutation");
    assert!(detect_graphql("POST", "http://h/api", &[], r#"{"name":"x"}"#).is_none());
    assert!(detect_graphql("POST", "http://h/api", &[], "not json").is_none());
    let batch = detect_graphql("POST", "http://h/", &[], r#"[{"query":"{ a }"}]"#).unwrap();
    assert_eq!(batch.query, "{ a }");
    let ct = detect_graphql("POST", "http://h/", &[("Content-Type".into(), "application/graphql".into())], "{ me { id } }").unwrap();
    assert_eq!(ct.query, "{ me { id } }");
    let get = detect_graphql("GET", "http://h/graphql?query=%7B%20a%20%7D&operationName=Op", &[], "").unwrap();
    assert_eq!((get.query.as_str(), get.operation_name.as_str()), ("{ a }", "Op"));
    let b = build_graphql_body("{ a }", "{\"x\": 1}", "Op").unwrap();
    let v: serde_json::Value = serde_json::from_str(&b).unwrap();
    assert_eq!(v["query"], "{ a }");
    assert_eq!(v["variables"]["x"], 1);
    assert_eq!(v["operationName"], "Op");
    assert!(build_graphql_body("{ a }", "[1]", "").is_err());
    assert!(build_graphql_body("{ a }", "{bad", "").is_err());
    // GraphQL mode adds a JSON content type when sending
    let r = SavedRequest { method: "POST".into(), url: "http://h/graphql".into(), body: b, body_mode: "graphql".into(), ..Default::default() };
    assert!(prepare(&r, &[]).headers.iter().any(|(k, v)| k == "Content-Type" && v == "application/json"));
}

#[test]
fn raw_headers_roundtrip() {
    let rows = vec![KvRow::new("A", "1"), KvRow { enabled: false, key: "B".into(), value: "x: y".into() }];
    let raw = rows_to_raw(&rows);
    assert_eq!(raw, "A: 1\n# B: x: y");
    assert_eq!(raw_to_rows(&raw), rows);
    assert_eq!(raw_to_rows("junk\n: v\nC:3").len(), 1);
}

#[test]
fn redirect_location_resolution() {
    assert_eq!(resolve_location("http://h:1/a/b?x", "/c"), "http://h:1/c");
    assert_eq!(resolve_location("http://h/a/b", "c"), "http://h/a/c");
    assert_eq!(resolve_location("http://h/a", "http://o/z"), "http://o/z");
    assert_eq!(url_authority("http://h/x").unwrap(), "h:80");
    assert_eq!(url_authority("http://127.0.0.1:81?q").unwrap(), "127.0.0.1:81");
}
