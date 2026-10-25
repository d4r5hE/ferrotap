//! Import Fiddler Classic AutoResponder rules (.farx).
use ferrotap::farx::parse_farx;
use ferrotap::rules_ext::{import_rule_set, plan_request, ReqCtx};

const FARX: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<AutoResponder LastSave="2026-01-01T00:00:00" FiddlerVersion="5.0">
  <State Enabled="true" Fallthrough="true" UseLatency="false">
    <ResponseRule Match="regex:^https://api\.test/v\d+/users$" Action="*404" Enabled="true" />
    <ResponseRule Match="EXACT:http://x.test/a?b=1" Action="*drop" Enabled="true" />
    <ResponseRule Match="METHOD:POST /upload" Action="*delay:250" Enabled="true" />
    <ResponseRule Match="old.example.com" Action="*redir:https://new.example.com/" Enabled="false" />
    <ResponseRule Match="cdn.test" Action="http://localhost:8080/" Enabled="true" />
    <ResponseRule Match="logo.png" Action="C:\mocks\logo.png" Enabled="true" />
    <ResponseRule Match="NOT:static" Action="*200" Enabled="true" />
    <ResponseRule Match="*" Action="*bpu" Enabled="true" />
  </State>
</AutoResponder>"#;

#[test]
fn farx_rules_import() {
    let r = parse_farx(FARX).unwrap();
    assert_eq!(r.len(), 8);
    assert_eq!((r[0].match_kind.as_str(), r[0].action.as_str(), r[0].status), ("regex", "canned", 404));
    let plan = plan_request(&r[..1], &ReqCtx { method: "GET", url: "https://api.test/v2/users", ..Default::default() });
    assert_eq!(plan.response.unwrap().status, 404);
    assert_eq!(r[1].action, "close");
    assert!(plan_request(&r[1..2], &ReqCtx { method: "GET", url: "http://x.test/a?b=1", ..Default::default() }).close, "EXACT: escapes regex characters");
    assert_eq!((r[2].method.as_str(), r[2].pattern.as_str(), r[2].delay_ms), ("POST", "/upload", 250));
    assert_eq!((r[3].action.as_str(), r[3].target.as_str(), r[3].enabled), ("redirect", "https://new.example.com/", false));
    assert_eq!((r[4].action.as_str(), r[4].target.as_str()), ("map_remote", "http://localhost:8080/"));
    assert_eq!((r[5].action.as_str(), r[5].file.as_str()), ("file", "C:\\mocks\\logo.png"));
    assert!(!r[6].enabled && r[6].name.contains("NOT:"));
    assert!(!r[7].enabled && r[7].name.contains("unsupported"));
    assert!(r.iter().all(|x| x.id != 0));
    assert!(parse_farx("<rules/>").is_err());

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("fiddler.farx"), FARX).unwrap();
    assert_eq!(import_rule_set(&dir.path().join("fiddler.farx")).unwrap().len(), 8);
    assert_eq!(import_rule_set(dir.path()).unwrap().len(), 8, "folders include .farx files");
}
