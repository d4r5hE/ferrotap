use ferrotap::g2::*;
use ferrotap::model::*;
use ferrotap::ui::GridApp;
use std::collections::BTreeMap;
use std::sync::mpsc::channel;

fn make_sample_session(id: usize, host: &str, path: &str, method: &str, status: u16) -> SessionRecord {
    let mut req_headers = BTreeMap::new();
    req_headers.insert("Host".to_string(), host.to_string());
    let mut resp_headers = BTreeMap::new();
    resp_headers.insert("Content-Type".to_string(), "application/json".to_string());

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
        response_body: "{\"status\": \"ok\"}".to_string(),
        start_time: "2026-10-14 12:00:00.000".to_string(),
        duration_ms: 125,
        mark: None,
        comment: String::new(),
        response_raw_size: Some(100),
        response_decoded_size: Some(100),
        response_trailers: vec![],
        process_name: Some("test.exe".to_string()),
        pid: Some(1234),
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
fn test_saved_filter_ui_state_transitions_and_actions() {
    let temp_dir = std::env::temp_dir().join(format!("ferrotap_filter_ui_test_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp_dir);
    let _ = std::fs::create_dir_all(&temp_dir);
    let filters_path = temp_dir.join("saved_filters.json");
    std::env::set_var("FERROTAP_SAVED_FILTERS", &filters_path);

    let s1 = make_sample_session(1, "api.example.com", "/v1/users", "GET", 200);
    let s2 = make_sample_session(2, "static.example.com", "/style.css", "GET", 404);
    let s3 = make_sample_session(3, "api.example.com", "/v1/auth", "POST", 500);
    let samples = vec![s1, s2, s3];

    let (_tx, rx) = channel();
    if std::env::var_os("FERROTAP_UI_STATE").is_none() { std::env::set_var("FERROTAP_UI_STATE", std::env::temp_dir().join(format!("ferrotap_ui_test_{}.json", std::process::id()))); }
    let mut app = GridApp::new(rx, "test");

    // Initial state: empty filters
    assert_eq!(app.saved_filters.filters().len(), 0);

    // 1. Add custom filter via SavedFiltersManager
    let rule_errors = AdvancedFilterRule {
        name: "Server & Client Errors".to_string(),
        combinator: Combinator::Any,
        conditions: vec![
            FilterCondition::new(FilterField::Status, ConditionOp::GreaterThan, "399"),
        ],
        enabled: true,
    };
    let id1 = app.saved_filters.add("Error Responses", rule_errors.clone());
    assert_eq!(app.saved_filters.filters().len(), 1);
    assert_eq!(app.saved_filters.filters()[0].name, "Error Responses");
    assert!(app.saved_filters.filters()[0].enabled);

    // Match testing against the rule
    let error_ids: Vec<usize> = samples.iter().filter(|s| rule_errors.matches(s)).map(|s| s.id).collect();
    assert_eq!(error_ids, vec![2, 3]);

    // 2. Test Rename action
    let renamed = app.saved_filters.rename(&id1, "All HTTP Errors");
    assert!(renamed);
    assert_eq!(app.saved_filters.filters()[0].name, "All HTTP Errors");

    // 3. Test Toggle Enable/Disable
    assert_eq!(app.saved_filters.toggle_enabled(&id1), Some(false));
    assert!(!app.saved_filters.filters()[0].enabled);
    assert_eq!(app.saved_filters.toggle_enabled(&id1), Some(true));
    assert!(app.saved_filters.filters()[0].enabled);

    // 4. Test Duplicate action
    let dup_id = app.saved_filters.duplicate(&id1);
    assert!(dup_id.is_some());
    assert_eq!(app.saved_filters.filters().len(), 2);
    let dup_filter = &app.saved_filters.filters()[1];
    assert_eq!(dup_filter.name, "All HTTP Errors (Copy)");
    assert_eq!(dup_filter.rule.conditions.len(), 1);

    // 5. Test Persistence to disk
    let save_res = app.saved_filters.save(&filters_path);
    assert!(save_res.is_ok());
    assert!(filters_path.exists());

    // Load back and verify state
    let loaded = SavedFiltersManager::load(&filters_path);
    assert_eq!(loaded.filters().len(), 2);
    assert_eq!(loaded.filters()[0].name, "All HTTP Errors");
    assert_eq!(loaded.filters()[1].name, "All HTTP Errors (Copy)");

    // 6. Test Reset Filters (Disables active rule & unselects active filter)
    app.saved_filters.active_filter_id = Some(id1.clone());
    app.filter_rule = Some(app.saved_filters.filters()[0].rule.clone());
    assert!(app.filter_rule.is_some());
    app.saved_filters.reset_filters();
    app.filter_rule = None;
    assert!(app.saved_filters.active_filter_id.is_none());
    assert!(app.filter_rule.is_none());

    // 7. Test Revert to Defaults
    let defaults = SavedFiltersManager::default_presets();
    assert!(!defaults.is_empty());
    app.saved_filters.revert(defaults);
    assert_eq!(app.saved_filters.filters().len(), 2);
    assert_eq!(app.saved_filters.filters()[0].name, "HTTP 4xx / 5xx Errors");
    assert_eq!(app.saved_filters.filters()[1].name, "JSON Responses");

    // 8. Test Delete action
    let first_id = app.saved_filters.filters()[0].id.clone();
    assert!(app.saved_filters.remove(&first_id));
    assert_eq!(app.saved_filters.filters().len(), 1);
    assert_eq!(app.saved_filters.filters()[0].name, "JSON Responses");

    let _ = std::fs::remove_dir_all(&temp_dir);
}
