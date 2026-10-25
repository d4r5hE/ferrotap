//! Integration tests (public API only): filters, full-text search, live updates, disk persistence.
use ferrotap::model::{SessionEvent, SessionMark, SessionModel, SessionRecord};
use ferrotap::prefs::UiPrefs;

fn rec(id: usize, method: &str, url: &str, status: u16) -> SessionRecord {
    SessionRecord { id, method: method.into(), url: url.into(), status, ..Default::default() }
}

fn sample() -> Vec<SessionRecord> {
    let mut a = rec(1, "GET", "http://a.com/index", 200);
    a.response_body = "hello NEEDLE world".into();
    let mut b = rec(2, "POST", "http://b.com/api/login", 404);
    b.request_headers.insert("X-Trace".into(), "zebra-123".into());
    vec![a, b, rec(3, "GET", "http://b.com/api/items", 500), rec(4, "GET", "http://c.com/img.png", 200)]
}

fn build(p: &UiPrefs) -> SessionModel {
    let mut m = SessionModel::default();
    p.to_model(&mut m);
    for mut r in sample() {
        p.apply_to(&mut r);
        m.apply(SessionEvent::New(r));
    }
    m
}

#[test]
fn filter_by_method_status_and_url() {
    let mut p = UiPrefs::default();
    let mut m = build(&p);
    assert_eq!(m.visible().len(), 4);
    p.method = "get".into();
    m.structured_filter = p.criteria(None);
    assert_eq!(m.visible(), vec![0, 2, 3]);
    p.method.clear();
    p.status = "404".into();
    m.structured_filter = p.criteria(None);
    assert_eq!(m.visible(), vec![1]);
    p.status.clear();
    p.url = "/API/".into();
    m.structured_filter = p.criteria(None);
    assert_eq!(m.visible(), vec![1, 2]);
    p.method = "GET".into();
    m.structured_filter = p.criteria(None);
    assert_eq!(m.visible(), vec![2]);
}

#[test]
fn full_text_search_headers_and_bodies() {
    let mut m = build(&UiPrefs::default());
    m.search_query = "needle".into();
    assert_eq!(m.visible(), vec![0]);
    m.search_in_bodies = false;
    assert!(m.visible().is_empty());
    m.search_in_bodies = true;
    m.search_query = "ZEBRA".into();
    assert_eq!(m.visible(), vec![1]);
    m.search_in_headers = false;
    assert!(m.visible().is_empty());
    m.search_in_headers = true;
    m.search_case_sensitive = true;
    assert!(m.visible().is_empty());
}

#[test]
fn live_mark_comment_events_update_table_and_persist_to_disk() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ui_state.json");
    let mut p = UiPrefs::default();
    let mut m = build(&p);

    m.apply(SessionEvent::Mark { id: 2, mark: Some(SessionMark::Red) });
    m.apply(SessionEvent::Comment { id: 2, comment: "login fails".into() });
    m.apply(SessionEvent::Mark { id: 4, mark: Some(SessionMark::Blue) });
    assert_eq!(m.rows[1].mark, Some(SessionMark::Red));
    assert_eq!(m.rows[1].comment, "login fails");
    m.structured_filter = p.criteria(Some(SessionMark::Red));
    assert_eq!(m.visible(), vec![1]);
    m.structured_filter = p.criteria(None);
    m.search_query = "login fails".into();
    assert_eq!(m.visible(), vec![1]);
    m.search_query.clear();

    for id in [2usize, 4] {
        let r = m.rows.iter().find(|r| r.id == id).unwrap().clone();
        p.sync_row(&r);
    }
    p.from_model(&m);
    p.save(&path).unwrap();
    assert!(path.exists());

    let loaded = UiPrefs::load(&path);
    let m2 = build(&loaded);
    assert_eq!(m2.rows[1].mark, Some(SessionMark::Red));
    assert_eq!(m2.rows[1].comment, "login fails");
    assert_eq!(m2.rows[3].mark, Some(SessionMark::Blue));
    assert_eq!(m2.rows[0].mark, None);

    let mut r = m2.rows[3].clone();
    r.mark = None;
    let mut p3 = loaded.clone();
    p3.sync_row(&r);
    assert_eq!(p3.annotations.len(), 1);
}
