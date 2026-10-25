use ferrotap::g2::*;
use ferrotap::model::*;
use std::collections::BTreeMap;

fn make_sample_session(id: usize, host: &str, path: &str, method: &str, status: u16) -> SessionRecord {
    let mut req_headers = BTreeMap::new();
    req_headers.insert("Host".to_string(), host.to_string());
    req_headers.insert("User-Agent".to_string(), "FerrotapTest/1.0".to_string());
    req_headers.insert("Cookie".to_string(), "session_id=abc123456; theme=dark".to_string());
    req_headers.insert("X-Custom-Trace".to_string(), format!("trace-{id}"));

    let mut resp_headers = BTreeMap::new();
    resp_headers.insert("Content-Type".to_string(), "application/json".to_string());
    resp_headers.insert("Set-Cookie".to_string(), "auth_token=jwtxyz987; Path=/".to_string());
    resp_headers.insert("X-Server".to_string(), "edge-node-01".to_string());

    SessionRecord {
        id,
        method: method.to_string(),
        url: format!("https://{}{}", host, path),
        status,
        http_version: "HTTP/1.1".to_string(),
        response_http_version: "HTTP/1.1".to_string(),
        request_headers: req_headers,
        response_headers: resp_headers,
        request_body: "{\"query\": \"ferrotap\"}".to_string(),
        response_body: "{\"status\": \"ok\", \"count\": 42}".to_string(),
        start_time: "2026-10-14 12:00:00.000".to_string(),
        duration_ms: 125,
        mark: Some(SessionMark::Green),
        comment: "test session".to_string(),
        response_raw_size: Some(1024),
        response_decoded_size: Some(2048),
        response_trailers: vec![],
        process_name: Some("curl.exe".to_string()),
        pid: Some(4567),
        client_ip: Some("127.0.0.1".to_string()),
        remote_ip: Some("93.184.216.34".to_string()),
        tls_version: Some("TLSv1.3".to_string()),
        dns_ms: Some(5),
        connect_ms: Some(15),
        tls_ms: Some(25),
        ttfb_ms: Some(40),
        ..Default::default()
    }
}

#[test]
fn test_custom_columns_extraction() {
    let session = make_sample_session(1, "api.example.com", "/v1/users", "POST", 200);

    let col_req_hdr = CustomColumnDef::new("req_h", "Trace", ColumnSource::Header("X-Custom-Trace".into()));
    assert_eq!(col_req_hdr.extract_value(&session), "trace-1");

    let col_resp_hdr = CustomColumnDef::new("resp_h", "Server", ColumnSource::Header("X-Server".into()));
    assert_eq!(col_resp_hdr.extract_value(&session), "edge-node-01");

    let col_cookie = CustomColumnDef::new("cookie_auth", "Auth Cookie", ColumnSource::Cookie("auth_token".into()));
    assert_eq!(col_cookie.extract_value(&session), "jwtxyz987");

    let col_req_cookie = CustomColumnDef::new("cookie_sess", "Sess Cookie", ColumnSource::Cookie("session_id".into()));
    assert_eq!(col_req_cookie.extract_value(&session), "abc123456");

    let col_proc = CustomColumnDef::new("proc", "Process", ColumnSource::ProcessName);
    assert_eq!(col_proc.extract_value(&session), "curl.exe");

    let col_pid = CustomColumnDef::new("pid", "PID", ColumnSource::ProcessId);
    assert_eq!(col_pid.extract_value(&session), "4567");

    let col_client_ip = CustomColumnDef::new("cip", "Client IP", ColumnSource::ClientIp);
    assert_eq!(col_client_ip.extract_value(&session), "127.0.0.1");

    let col_remote_ip = CustomColumnDef::new("rip", "Remote IP", ColumnSource::RemoteIp);
    assert_eq!(col_remote_ip.extract_value(&session), "93.184.216.34");

    let col_tls = CustomColumnDef::new("tls", "TLS", ColumnSource::TlsVersion);
    assert_eq!(col_tls.extract_value(&session), "TLSv1.3");

    let col_ttfb = CustomColumnDef::new("ttfb", "TTFB", ColumnSource::Duration);
    assert_eq!(col_ttfb.extract_value(&session), "125ms");
}

#[test]
fn test_advanced_filter_conditions() {
    let session = make_sample_session(1, "api.example.com", "/v1/users", "POST", 200);

    // Method Equals
    let cond_method = FilterCondition::new(FilterField::Method, ConditionOp::Equals, "POST");
    assert!(cond_method.eval(&session));

    // Status LessThan
    let cond_status = FilterCondition::new(FilterField::Status, ConditionOp::LessThan, "300");
    assert!(cond_status.eval(&session));

    // Host EndsWith
    let cond_host = FilterCondition::new(FilterField::Host, ConditionOp::EndsWith, "example.com");
    assert!(cond_host.eval(&session));

    // Path StartsWith
    let cond_path = FilterCondition::new(FilterField::Path, ConditionOp::StartsWith, "/v1");
    assert!(cond_path.eval(&session));

    // Header Exists
    let cond_hdr = FilterCondition::new(FilterField::ReqHeader("X-Custom-Trace".into()), ConditionOp::Exists, "");
    assert!(cond_hdr.eval(&session));

    // Cookie Contains
    let cond_cookie = FilterCondition::new(FilterField::ReqHeader("Cookie".into()), ConditionOp::Contains, "abc123");
    assert!(cond_cookie.eval(&session));

    // Regex match
    let cond_regex = FilterCondition::new(FilterField::ResponseBody, ConditionOp::RegexMatch, r#""count":\s*42"#);
    assert!(cond_regex.eval(&session));

    // Rule combination (AND / All)
    let mut rule_all = empty_rule();
    rule_all.combinator = Combinator::All;
    rule_all.conditions.push(cond_method);
    rule_all.conditions.push(cond_status);
    assert!(rule_all.matches(&session));

    // Rule combination (OR / Any)
    let mut rule_any = empty_rule();
    rule_any.combinator = Combinator::Any;
    rule_any.conditions.push(FilterCondition::new(FilterField::Method, ConditionOp::Equals, "GET"));
    rule_any.conditions.push(FilterCondition::new(FilterField::Status, ConditionOp::Equals, "200"));
    assert!(rule_any.matches(&session));
}

#[test]
fn test_saved_filters_manager() {
    let mut mgr = SavedFiltersManager::new();
    assert_eq!(mgr.filters.len(), 0);

    let mut rule = empty_rule();
    rule.conditions.push(FilterCondition::new(FilterField::Status, ConditionOp::GreaterThan, "399"));
    let id = mgr.add("Errors Only", rule);
    assert_eq!(mgr.filters.len(), 1);

    let dup_id = mgr.duplicate(&id).expect("duplicate succeeds");
    assert_eq!(mgr.filters.len(), 2);
    assert_eq!(mgr.filters[1].name, "Errors Only (Copy)");

    let toggled = mgr.toggle_enabled(&id);
    assert_eq!(toggled, Some(false));

    assert!(mgr.delete(&dup_id));
    assert_eq!(mgr.filters.len(), 1);

    mgr.reset_filters();
    assert_eq!(mgr.filters.len(), 1);
    assert!(!mgr.filters[0].enabled);
    assert_eq!(mgr.active_filter_id, None);
}

#[test]
fn test_traffic_groups_manager() {
    let mut mgr = TrafficGroupsManager::new();
    let s1 = make_sample_session(1, "api.example.com", "/v1/a", "GET", 200);
    let s2 = make_sample_session(2, "api.example.com", "/v1/b", "POST", 201);
    let mut s3 = make_sample_session(3, "cdn.example.com", "/asset.js", "GET", 200);
    s3.process_name = Some("browser.exe".into());

    let sessions = vec![s1, s2, s3];

    // Group by Host
    mgr.group_by = Some(GroupBy::Host);
    let groups = mgr.group_sessions(&sessions);
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].key, "api.example.com");
    assert_eq!(groups[0].session_ids, vec![1, 2]);
    assert_eq!(groups[1].key, "cdn.example.com");
    assert_eq!(groups[1].session_ids, vec![3]);

    // Group by Process
    mgr.group_by = Some(GroupBy::Process);
    let groups_proc = mgr.group_sessions(&sessions);
    assert_eq!(groups_proc.len(), 2);

    // Pin group
    mgr.pin_group("cdn.example.com");
    assert!(mgr.pinned_keys.contains("cdn.example.com"));
    mgr.unpin_group("cdn.example.com");
    assert!(!mgr.pinned_keys.contains("cdn.example.com"));
}

#[test]
fn test_autocomplete_index() {
    let mut index = AutocompleteIndex::new();
    let s = make_sample_session(1, "api.ferrotap.dev", "/v1/query?sort=asc&page=2", "GET", 200);
    index.index_session(&s);

    let host_suggestions = index.suggest(&FilterField::Host, "api");
    assert_eq!(host_suggestions, vec!["api.ferrotap.dev"]);

    let path_suggestions = index.suggest(&FilterField::Path, "/v1");
    assert!(path_suggestions.contains(&"/v1/query?sort=asc&page=2".to_string()));

    let method_suggestions = index.suggest(&FilterField::Method, "g");
    assert_eq!(method_suggestions, vec!["GET"]);

    let status_suggestions = index.suggest(&FilterField::Status, "20");
    assert_eq!(status_suggestions, vec!["200"]);
}

fn empty_rule() -> AdvancedFilterRule {
    let mut r = AdvancedFilterRule::single(FilterCondition::new(FilterField::Method, ConditionOp::Exists, ""));
    r.conditions.clear();
    r
}

#[test]
fn test_streaming_state() {
    let mut st = StreamingState::new();
    st.handle_chunk(1, b"chunk 1 ", false);
    st.handle_chunk(1, b"chunk 2", false);
    let s = st.get_stream(1).expect("stream exists");
    assert_eq!(s.total_bytes, 15);
    assert_eq!(s.chunks.len(), 2);
    assert!(!s.complete);
    st.handle_chunk(1, b"", true);
    assert!(st.get_stream(1).expect("stream").complete);
}

#[test]
fn test_traffic_tabs_manager() {
    let mut m = TrafficTabsManager::new();
    assert_eq!(m.tabs.len(), 1);
    assert_eq!(m.active_tab_index, 0);
    let idx = m.create_tab("API Only");
    assert_eq!(m.tabs.len(), 2);
    assert_eq!(m.active_tab_index, idx);
    m.close_tab(0);
    assert_eq!(m.tabs.len(), 2, "the main capture tab cannot be closed");
    m.close_tab(idx);
    assert_eq!(m.tabs.len(), 1);
    assert_eq!(m.active_tab_index, 0);
    assert!(m.toggle_side_panel());
    assert!(!m.toggle_side_panel());
}
#[test]
fn test_filter_rule_any_all_multi_condition() {
    let s = make_sample_session(1, "api.example.com", "/v1/users", "POST", 200);
    let a = FilterCondition::new(FilterField::Method, ConditionOp::Equals, "POST");
    let b = FilterCondition::new(FilterField::Status, ConditionOp::Equals, "404");
    let mut rule = AdvancedFilterRule::single(a.clone());
    rule.conditions.push(b);
    rule.combinator = Combinator::All;
    assert!(!rule.matches(&s));
    rule.combinator = Combinator::Any;
    assert!(rule.matches(&s));
    rule.enabled = false;
    assert!(rule.matches(&s));
    assert!(rule.describe().len() > 0);
}

#[test]
fn test_saved_filters_lifecycle() {
    let s = make_sample_session(1, "api.example.com", "/", "GET", 500);
    let mut m = SavedFiltersManager::new();
    let rule = AdvancedFilterRule::single(FilterCondition::new(FilterField::Status, ConditionOp::Equals, "200"));
    let id = m.add("only200", rule);
    assert!(!m.matches(&s));
    assert_eq!(m.toggle_enabled(&id), Some(false));
    assert!(m.matches(&s));
    m.toggle_enabled(&id);
    let dup = m.duplicate(&id).expect("dup");
    assert_eq!(m.filters().len(), 2);
    assert!(m.filters().iter().any(|f| f.id == dup && f.name.contains("Copy")));
    m.reset_filters();
    assert!(m.matches(&s));
    assert!(m.delete(&dup));
    assert!(!m.delete("nope"));
    m.revert(vec![]);
    assert!(m.filters().is_empty());
}

#[test]
fn test_timing_and_protocol_fields() {
    let s = make_sample_session(1, "api.example.com", "/", "GET", 200);
    assert_eq!(s.connect_ms, Some(15));
    assert_eq!(s.tls_ms, Some(25));
    assert_eq!(s.ttfb_ms, Some(40));
    assert_eq!(s.tls_version.as_deref(), Some("TLSv1.3"));
    assert_eq!(s.http_version, "HTTP/1.1");
    let json = serde_json::to_string(&s).unwrap();
    let back: SessionRecord = serde_json::from_str(&json).unwrap();
    assert_eq!(back.ttfb_ms, Some(40));
}

// ---- Filter Builder window code path: GridApp::filter_builder_rule + SavedFiltersManager save/load ----
fn fb_samples() -> Vec<SessionRecord> {
    vec![
        make_sample_session(1, "api.example.com", "/users", "GET", 200),
        make_sample_session(2, "api.example.com", "/users", "POST", 201),
        make_sample_session(3, "cdn.example.com", "/img", "GET", 404),
        make_sample_session(4, "api.other.com", "/login", "POST", 500),
        make_sample_session(5, "cdn.example.com", "/a", "GET", 200),
    ]
}

fn fb_app(any: bool, rows: Vec<FilterCondition>) -> ferrotap::ui::GridApp {
    let (_tx, rx) = std::sync::mpsc::channel();
    if std::env::var_os("FERROTAP_UI_STATE").is_none() { std::env::set_var("FERROTAP_UI_STATE", std::env::temp_dir().join(format!("ferrotap_ui_test_{}.json", std::process::id()))); }
    let mut app = ferrotap::ui::GridApp::new(rx, "test");
    app.filter_builder_rows = rows;
    app.filter_builder_any = any;
    app
}

fn fb_ids(rule: &AdvancedFilterRule, rows: &[SessionRecord]) -> Vec<usize> {
    rows.iter().filter(|r| rule.matches(r)).map(|r| r.id).collect()
}

fn fb_any_rows() -> Vec<FilterCondition> {
    vec![
        FilterCondition::new(FilterField::Method, ConditionOp::Equals, "POST"),
        FilterCondition::new(FilterField::Status, ConditionOp::Equals, "404"),
    ]
}

fn fb_all_rows() -> Vec<FilterCondition> {
    vec![
        FilterCondition::new(FilterField::Host, ConditionOp::Contains, "example.com"),
        FilterCondition::new(FilterField::Method, ConditionOp::Equals, "GET"),
    ]
}

#[test]
fn builder_any_filter_matches_exact_rows() {
    let rule = fb_app(true, fb_any_rows()).filter_builder_rule();
    assert_eq!(rule.combinator, Combinator::Any);
    assert_eq!(fb_ids(&rule, &fb_samples()), vec![2, 3, 4]);
}

#[test]
fn builder_all_filter_matches_exact_rows() {
    let rule = fb_app(false, fb_all_rows()).filter_builder_rule();
    assert_eq!(rule.combinator, Combinator::All);
    assert_eq!(fb_ids(&rule, &fb_samples()), vec![1, 3, 5]);
}

#[test]
fn builder_save_load_same_matches() {
    let samples = fb_samples();
    let dir = std::env::temp_dir().join(format!("ferrotap_fb_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("saved_filters.json");

    let any_rule = fb_app(true, fb_any_rows()).filter_builder_rule();
    let all_rule = fb_app(false, fb_all_rows()).filter_builder_rule();
    let mut mgr = SavedFiltersManager::new();
    mgr.add("posts or 404", any_rule.clone());
    mgr.add("example GETs", all_rule.clone());
    mgr.save(&path).expect("save");

    let loaded = SavedFiltersManager::load(&path);
    assert_eq!(loaded.filters.len(), 2);
    let a = loaded.filters.iter().find(|f| f.name == "posts or 404").expect("any saved");
    let b = loaded.filters.iter().find(|f| f.name == "example GETs").expect("all saved");
    assert_eq!(fb_ids(&a.rule, &samples), vec![2, 3, 4]);
    assert_eq!(fb_ids(&b.rule, &samples), vec![1, 3, 5]);
    assert_eq!(a.rule.combinator, Combinator::Any);
    assert_eq!(b.rule.combinator, Combinator::All);

    let mut app = fb_app(false, vec![]);
    app.filter_builder_rows = a.rule.conditions.clone();
    app.filter_builder_any = a.rule.combinator == Combinator::Any;
    assert_eq!(fb_ids(&app.filter_builder_rule(), &samples), vec![2, 3, 4]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn describe_uses_readable_field_names() {
    let mut rule = AdvancedFilterRule::single(FilterCondition::new(FilterField::ReqHeader("Accept".into()), ConditionOp::Contains, "json"));
    rule.conditions.push(FilterCondition::new(FilterField::Status, ConditionOp::Equals, "404"));
    let d = rule.describe();
    assert_eq!(d, "Request Header \"Accept\" contains 'json' AND Status Code equals '404'");
    assert!(!d.contains("ReqHeader"));
}
