use ferrotap::g2::*;
use ferrotap::model::*;
use ferrotap::ui::GridApp;
use std::collections::BTreeMap;
use std::sync::mpsc::channel;

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
fn test_filter_builder_live_grid_and_restart_persistence() {
    let temp_dir = std::env::temp_dir().join(format!("ferrotap_restart_test_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp_dir);
    std::fs::create_dir_all(&temp_dir).unwrap();
    // GridApp::new loads saved filters from here: keep the test away from the real data\saved_filters.json
    let store = temp_dir.join("saved_filters.json");
    std::env::set_var("FERROTAP_SAVED_FILTERS", &store);

    // 1. Setup sample session data for the traffic grid
    let s1 = make_sample_session(1, "api.example.com", "/v1/auth/login", "POST", 200);
    let s2 = make_sample_session(2, "cdn.example.com", "/assets/style.css", "GET", 200);
    let s3 = make_sample_session(3, "api.example.com", "/v1/users", "POST", 201);
    let s4 = make_sample_session(4, "example.com", "/favicon.ico", "GET", 404);
    let samples = vec![s1, s2, s3, s4];

    let (_tx, rx) = channel();
    if std::env::var_os("FERROTAP_UI_STATE").is_none() { std::env::set_var("FERROTAP_UI_STATE", std::env::temp_dir().join(format!("ferrotap_ui_test_{}.json", std::process::id()))); }
    let mut app = GridApp::new(rx, "test");

    // 2. Set filter builder condition: Method == 'GET'
    app.filter_builder_rows = vec![
        FilterCondition::new(FilterField::Method, ConditionOp::Equals, "GET"),
    ];
    app.filter_builder_any = false;
    let filter_name = "Only GET Requests".to_string();
    app.filter_builder_name = filter_name.clone();

    // Build the rule using GridApp logic
    let rule = app.filter_builder_rule();
    assert_eq!(rule.combinator, Combinator::All);
    assert_eq!(rule.conditions.len(), 1);

    // Verify filtering against sample sessions
    let visible_ids: Vec<usize> = samples.iter().filter(|s| rule.matches(s)).map(|s| s.id).collect();
    assert_eq!(visible_ids, vec![2, 4], "Visible rows in traffic grid should be GET only");

    // 3. Save it as a named filter, make it the active one, and write it to disk (what "Save" + "Apply" do)
    let saved_id = app.saved_filters.add(&filter_name, rule.clone());
    app.saved_filters.active_filter_id = Some(saved_id.clone());
    app.saved_filters.save(&store).unwrap();

    // 4. Restart: a new GridApp loads the saved filters and restores the active filter on its own
    let (_tx2, rx2) = channel();
    if std::env::var_os("FERROTAP_UI_STATE").is_none() { std::env::set_var("FERROTAP_UI_STATE", std::env::temp_dir().join(format!("ferrotap_ui_test_{}.json", std::process::id()))); }
    let restarted = GridApp::new(rx2, "test");
    assert_eq!(restarted.saved_filters.filters().len(), 1);
    assert_eq!(restarted.saved_filters.filters()[0].name, filter_name);
    assert_eq!(restarted.saved_filters.active_filter_id.as_deref(), Some(saved_id.as_str()));
    let active_rule = restarted.filter_rule.as_ref().expect("the active filter is restored at startup");
    let visible_after_restart: Vec<usize> = samples.iter().filter(|s| active_rule.matches(s)).map(|s| s.id).collect();
    assert_eq!(visible_after_restart, vec![2, 4], "the grid is still filtered after a restart");

    // 5. A rule applied without saving it by name (active_rule) wins over the active saved filter
    let mut manager = SavedFiltersManager::load(&store);
    manager.active_rule = Some(AdvancedFilterRule::single(FilterCondition::new(FilterField::Method, ConditionOp::Equals, "POST")));
    manager.save(&store).unwrap();
    let (_tx3, rx3) = channel();
    if std::env::var_os("FERROTAP_UI_STATE").is_none() { std::env::set_var("FERROTAP_UI_STATE", std::env::temp_dir().join(format!("ferrotap_ui_test_{}.json", std::process::id()))); }
    let restarted = GridApp::new(rx3, "test");
    let rule = restarted.filter_rule.as_ref().expect("the applied rule is restored");
    let visible: Vec<usize> = samples.iter().filter(|s| rule.matches(s)).map(|s| s.id).collect();
    assert_eq!(visible, vec![1, 3]);

    // 6. Clearing the filter (no active rule or filter) starts unfiltered
    let mut manager = SavedFiltersManager::load(&store);
    manager.active_rule = None;
    manager.active_filter_id = None;
    manager.save(&store).unwrap();
    let (_tx4, rx4) = channel();
    if std::env::var_os("FERROTAP_UI_STATE").is_none() { std::env::set_var("FERROTAP_UI_STATE", std::env::temp_dir().join(format!("ferrotap_ui_test_{}.json", std::process::id()))); }
    assert!(GridApp::new(rx4, "test").filter_rule.is_none());

    let _ = std::fs::remove_dir_all(&temp_dir);
}
