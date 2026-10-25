//! G1: every context-menu copy action yields the expected output; clear filters are all offered.
use ferrotap::codegen::{to_curl, to_fetch, to_powershell, to_python};
use ferrotap::g1::{copy_row, copy_summary, ClearKind};
use ferrotap::g1_replay::{copy_text_for, CopyAction};
use ferrotap::model::SessionRecord;

fn sample() -> SessionRecord {
    let mut s = SessionRecord {
        id: 5,
        method: "POST".into(),
        url: "https://api.test/v1/items".into(),
        status: 201,
        request_body: "{\"a\":1}".into(),
        ..Default::default()
    };
    s.request_headers.insert("Content-Type".into(), "application/json".into());
    s.request_headers.insert("Authorization".into(), "Bearer tok123".into());
    s
}

#[test]
fn url_row_summary_actions() {
    let s = sample();
    assert_eq!(copy_text_for(CopyAction::Url, &s), "https://api.test/v1/items");
    assert_eq!(copy_text_for(CopyAction::Summary, &s), copy_summary(&s));
    assert_eq!(copy_text_for(CopyAction::Summary, &s), "#5 POST 201 https://api.test/v1/items");
    assert_eq!(copy_text_for(CopyAction::Row, &s), copy_row(&s));
    assert!(copy_text_for(CopyAction::Row, &s).starts_with("5\tPOST\t201\t"));
}

#[test]
fn curl_action_dispatches_to_curl_generator() {
    let s = sample();
    let t = copy_text_for(CopyAction::Curl, &s);
    assert_eq!(t, to_curl(&s));
    assert!(t.starts_with("curl"));
    assert!(t.contains("POST"));
    assert!(t.contains("https://api.test/v1/items"));
    assert!(t.contains("Bearer tok123"));
}

#[test]
fn powershell_action_dispatches_to_powershell_generator() {
    let s = sample();
    let t = copy_text_for(CopyAction::PowerShell, &s);
    assert_eq!(t, to_powershell(&s));
    assert!(t.contains("Invoke-"));
    assert!(t.contains("https://api.test/v1/items"));
    assert!(t.contains("Bearer tok123"));
}

#[test]
fn fetch_action_dispatches_to_fetch_generator() {
    let s = sample();
    let t = copy_text_for(CopyAction::Fetch, &s);
    assert_eq!(t, to_fetch(&s));
    assert!(t.contains("fetch(\"https://api.test/v1/items\""));
    assert!(t.contains("method: \"POST\""));
}

#[test]
fn python_action_dispatches_to_python_generator() {
    let s = sample();
    let t = copy_text_for(CopyAction::Python, &s);
    assert_eq!(t, to_python(&s));
    assert!(t.contains("requests.post"));
    assert!(t.contains("https://api.test/v1/items"));
}

#[test]
fn exports_are_distinct_per_action() {
    let s = sample();
    let all = [CopyAction::Curl, CopyAction::PowerShell, CopyAction::Fetch, CopyAction::Python]
        .map(|a| copy_text_for(a, &s));
    for i in 0..all.len() {
        for j in i + 1..all.len() {
            assert_ne!(all[i], all[j]);
        }
    }
}

#[test]
fn clear_menu_offers_every_filter_once() {
    let k = ClearKind::ALL_KINDS;
    assert_eq!(k.len(), 15); // + 1xx, marked, non-marked, completed, cached, non-cached
    for i in 0..k.len() {
        for j in i + 1..k.len() {
            assert_ne!(k[i], k[j]);
        }
    }
    assert!(k.contains(&ClearKind::All) && k.contains(&ClearKind::Unselected));
}

#[test]
fn clear_cached_and_completed() {
    use ferrotap::model::{SessionEvent, SessionModel, SessionRecord};
    let mut m = SessionModel::default();
    let mk = |id: usize, status: u16, h: &[(&str, &str)]| {
        let mut s = SessionRecord { id, method: "GET".into(), url: format!("https://c.test/{id}"), status, ..Default::default() };
        for (k, v) in h {
            s.response_headers.insert(k.to_string(), v.to_string());
        }
        s
    };
    for s in [mk(1, 304, &[]), mk(2, 200, &[("x-cache", "HIT from edge")]), mk(3, 200, &[("age", "120")]), mk(4, 200, &[]), mk(5, 0, &[])] {
        m.apply(SessionEvent::New(s));
    }
    assert!(ferrotap::g1::is_cached(&m.rows[0]) && !ferrotap::g1::is_cached(&m.rows[3]));
    assert_eq!(m.clone().clear_kind(ClearKind::Cached), 3);
    assert_eq!(m.clone().clear_kind(ClearKind::NonCached), 2);
    assert_eq!(m.clear_kind(ClearKind::Completed), 4);
    assert_eq!(m.rows.len(), 1);
}
