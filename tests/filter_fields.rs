// The advanced filter builder's fields: cookies, PID, timings and TLS version, alone and in Any/All rules.
use ferrotap::g2::*;
use ferrotap::model::*;
use std::collections::BTreeMap;

fn session(id: usize, cookie: &str, pid: u32, connect_ms: u64, ttfb_ms: u64, tls: &str) -> SessionRecord {
    let mut req = BTreeMap::new();
    req.insert("Host".to_string(), "api.example.com".to_string());
    req.insert("Cookie".to_string(), cookie.to_string());
    let mut resp = BTreeMap::new();
    resp.insert("Set-Cookie".to_string(), "auth=xyz; Path=/".to_string());
    SessionRecord {
        id,
        method: "GET".to_string(),
        url: "https://api.example.com/v1".to_string(),
        status: 200,
        http_version: "HTTP/1.1".to_string(),
        response_http_version: "HTTP/1.1".to_string(),
        request_headers: req,
        response_headers: resp,
        request_body: String::new(),
        response_body: String::new(),
        start_time: "2026-10-16 12:00:00.000".to_string(),
        duration_ms: 100,
        mark: None,
        comment: String::new(),
        response_raw_size: None,
        response_decoded_size: None,
        response_trailers: vec![],
        process_name: Some("curl.exe".to_string()),
        pid: Some(pid),
        client_ip: None,
        remote_ip: None,
        tls_version: Some(tls.to_string()),
        dns_ms: None,
        connect_ms: Some(connect_ms),
        tls_ms: Some(20),
        ttfb_ms: Some(ttfb_ms),
        ..Default::default()
    }
}

fn ids(rule: &AdvancedFilterRule, rows: &[SessionRecord]) -> Vec<usize> {
    rows.iter().filter(|s| rule.matches(s)).map(|s| s.id).collect()
}

fn one(field: FilterField, op: ConditionOp, value: &str) -> AdvancedFilterRule {
    AdvancedFilterRule::single(FilterCondition::new(field, op, value))
}

#[test]
fn new_filter_fields_match_sessions() {
    let rows = vec![
        session(1, "session_id=abc; theme=dark", 100, 10, 300, "TLS 1.3"),
        session(2, "session_id=zzz; theme=light", 200, 80, 40, "TLS 1.2"),
        session(3, "theme=dark", 100, 150, 900, "TLS 1.3"),
    ];

    // a named cookie from the request Cookie header
    assert_eq!(ids(&one(FilterField::Cookie("theme".into()), ConditionOp::Equals, "dark"), &rows), vec![1, 3]);
    assert_eq!(ids(&one(FilterField::Cookie("session_id".into()), ConditionOp::Exists, ""), &rows), vec![1, 2]);
    // a cookie set by the response (Set-Cookie)
    assert_eq!(ids(&one(FilterField::Cookie("auth".into()), ConditionOp::Equals, "xyz"), &rows), vec![1, 2, 3]);

    assert_eq!(ids(&one(FilterField::Pid, ConditionOp::Equals, "100"), &rows), vec![1, 3]);
    assert_eq!(ids(&one(FilterField::ConnectMs, ConditionOp::GreaterThan, "50"), &rows), vec![2, 3]);
    assert_eq!(ids(&one(FilterField::TtfbMs, ConditionOp::LessThan, "500"), &rows), vec![1, 2]);
    assert_eq!(ids(&one(FilterField::TlsVersion, ConditionOp::Contains, "1.2"), &rows), vec![2]);

    // All: PID 100 and slow first byte; Any: TLS 1.2 or connect over 100 ms
    let all = AdvancedFilterRule {
        name: "slow on pid 100".into(),
        combinator: Combinator::All,
        conditions: vec![
            FilterCondition::new(FilterField::Pid, ConditionOp::Equals, "100"),
            FilterCondition::new(FilterField::TtfbMs, ConditionOp::GreaterThan, "500"),
        ],
        enabled: true,
    };
    assert_eq!(ids(&all, &rows), vec![3]);
    let any = AdvancedFilterRule { combinator: Combinator::Any, ..all.clone() };
    assert_eq!(ids(&any, &rows), vec![1, 3]);
    let any2 = AdvancedFilterRule {
        name: "old tls or slow connect".into(),
        combinator: Combinator::Any,
        conditions: vec![
            FilterCondition::new(FilterField::TlsVersion, ConditionOp::Contains, "1.2"),
            FilterCondition::new(FilterField::ConnectMs, ConditionOp::GreaterThan, "100"),
        ],
        enabled: true,
    };
    assert_eq!(ids(&any2, &rows), vec![2, 3]);
}
