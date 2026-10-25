//! G16 record a browsing session and turn it into a test (assertions checked on replay).
use ferrotap::api_tools::{check, AssertKind};
use ferrotap::automation::{sessions_to_test, tests_for_session};
use ferrotap::model::{ComposerResponse, SessionRecord};

fn rec(id: usize, status: u16, body: &str) -> SessionRecord {
    let mut s = SessionRecord { id, method: "GET".into(), url: format!("https://api.test/{id}"), status, duration_ms: 1500, ..Default::default() };
    s.response_headers.insert("content-type".into(), "application/json; charset=utf-8".into());
    s.set_response_body(body.as_bytes());
    s
}

#[test]
fn recorded_sessions_become_checks() {
    let t = tests_for_session(&rec(1, 200, r#"{"id":1,"name":"x","bad key":2}"#));
    let kinds: Vec<_> = t.iter().map(|a| (a.kind, a.target.as_str(), a.expected.as_str())).collect();
    assert!(kinds.contains(&(AssertKind::StatusEquals, "", "200")));
    assert!(kinds.contains(&(AssertKind::HeaderExists, "content-type", "")));
    assert!(kinds.contains(&(AssertKind::JsonPathExists, "$.id", "")));
    assert!(kinds.contains(&(AssertKind::JsonPathExists, "$.name", "")));
    assert!(!kinds.iter().any(|k| k.1.contains("bad key")));
    assert!(kinds.contains(&(AssertKind::TimeBelow, "", "4500")));

    let c = sessions_to_test("flow", &[rec(5, 201, "[]"), rec(2, 200, "{}")]);
    assert!(c.requests[0].name.starts_with("001") && c.requests[0].url.ends_with("/2"), "recording order");
    assert_eq!(c.requests[1].tests[0].expected, "201");

    // the checks pass on a matching replay and fail on a different one
    let good = ComposerResponse { status: 200, headers: vec![("Content-Type".into(), "application/json".into())], body: r#"{"id":9,"name":"y"}"#.into(), elapsed_ms: 100, ..Default::default() };
    assert!(t.iter().all(|a| check(a, &good).passed), "{:?}", t.iter().map(|a| check(a, &good)).collect::<Vec<_>>());
    let bad = ComposerResponse { status: 500, body: "oops".into(), elapsed_ms: 9000, ..Default::default() };
    assert!(t.iter().filter(|a| !check(a, &bad).passed).count() >= 4);
}
