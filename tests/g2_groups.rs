// G2 traffic groups: rebuilt only on change, pin/unpin (and saved), remove a group's sessions, every group-by.
use ferrotap::g2::*;
use ferrotap::model::*;
use ferrotap::prefs::UiPrefs;
use std::collections::BTreeMap;

fn session(id: usize, host: &str, status: u16, proc_name: &str) -> SessionRecord {
    let mut req = BTreeMap::new();
    req.insert("Host".to_string(), host.to_string());
    SessionRecord {
        id,
        method: if id % 2 == 0 { "POST" } else { "GET" }.to_string(),
        url: format!("https://{host}/p{id}"),
        status,
        http_version: "HTTP/1.1".to_string(),
        response_http_version: "HTTP/1.1".to_string(),
        request_headers: req,
        response_headers: BTreeMap::new(),
        request_body: String::new(),
        response_body: String::new(),
        start_time: "2026-10-16 12:00:00.000".to_string(),
        duration_ms: 10,
        mark: None,
        comment: String::new(),
        response_raw_size: None,
        response_decoded_size: None,
        response_trailers: vec![],
        process_name: Some(proc_name.to_string()),
        pid: Some(1),
        client_ip: None,
        remote_ip: None,
        tls_version: None,
        dns_ms: None,
        connect_ms: None,
        tls_ms: None,
        ttfb_ms: None,
        ..Default::default()
    }
}

fn rows() -> Vec<SessionRecord> {
    vec![session(1, "a.com", 200, "chrome.exe"), session(2, "b.com", 404, "curl.exe"),
         session(3, "a.com", 500, "chrome.exe"), session(4, "c.com", 200, "chrome.exe")]
}

#[test]
fn refresh_rebuilds_only_when_something_changed() {
    let mut m = TrafficGroupsManager::new();
    let mut r = rows();
    assert!(m.refresh(&r));
    assert!(!m.refresh(&r), "nothing changed: no rebuild");
    r[1].status = 200; // a session completed / changed
    assert!(m.refresh(&r));
    r.push(session(5, "d.com", 200, "x.exe"));
    assert!(m.refresh(&r));
    m.set_group_by(GroupBy::Process);
    assert!(m.refresh(&r));
    m.toggle_pin("chrome.exe");
    assert!(m.refresh(&r));
    assert!(!m.refresh(&r));
}

#[test]
fn pinned_groups_sort_first_and_ids_of() {
    let mut m = TrafficGroupsManager::new();
    let r = rows();
    m.refresh(&r);
    let keys: Vec<&str> = m.groups.iter().map(|g| g.key.as_str()).collect();
    assert_eq!(keys, vec!["a.com", "b.com", "c.com"]);
    assert_eq!(m.ids_of("a.com"), vec![1, 3]);
    m.toggle_pin("c.com");
    m.refresh(&r);
    assert_eq!(m.groups[0].key, "c.com");
    assert!(m.groups[0].pinned);
    m.toggle_pin("c.com");
    m.refresh(&r);
    assert_eq!(m.groups[0].key, "a.com");
}

#[test]
fn remove_a_groups_sessions() {
    let mut model = SessionModel::default();
    model.rows = rows();
    let mut m = TrafficGroupsManager::new();
    m.refresh(&model.rows);
    let ids = m.ids_of("a.com");
    assert_eq!(model.remove_ids(&ids), 2);
    assert!(m.refresh(&model.rows));
    assert_eq!(m.groups.iter().map(|g| g.key.as_str()).collect::<Vec<_>>(), vec!["b.com", "c.com"]);
}

#[test]
fn every_group_by_has_a_label_and_groups() {
    let r = rows();
    for gb in GroupBy::ALL {
        assert!(!gb.label().is_empty());
        let mut m = TrafficGroupsManager::new();
        m.set_group_by(gb);
        m.refresh(&r);
        let total: usize = m.groups.iter().map(|g| g.session_ids.len()).sum();
        assert_eq!(total, r.len(), "{:?} must place every session in one group", gb);
    }
    let mut m = TrafficGroupsManager::new();
    m.set_group_by(GroupBy::StatusClass);
    m.refresh(&r);
    assert_eq!(m.ids_of("2xx Success"), vec![1, 4]);
}

#[test]
fn pinned_groups_are_saved_with_the_ui_state() {
    let dir = std::env::temp_dir().join(format!("ferrotap_groups_test_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("ui_state.json");
    let mut p = UiPrefs::default();
    p.pinned_groups = vec!["a.com".into(), "chrome.exe".into()];
    p.save(&path).unwrap();
    assert_eq!(UiPrefs::load(&path).pinned_groups, vec!["a.com".to_string(), "chrome.exe".to_string()]);
    let _ = std::fs::remove_dir_all(&dir);
}
