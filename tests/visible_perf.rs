//! The grid recomputes visible rows every frame: with a search over big bodies that must stay fast.
use ferrotap::model::{SessionEvent, SessionModel, SessionRecord};
use std::time::{Duration, Instant};

fn model() -> SessionModel {
    let mut m = SessionModel::default();
    let body = "lorem ipsum dolor sit amet ".repeat(12_000); // ~320 KB
    for i in 1..=300 {
        let mut s = SessionRecord { id: i, method: "GET".into(), url: format!("https://p{}.test/x", i % 10), status: 200, ..Default::default() };
        s.response_headers.insert("content-type".into(), "text/plain".into());
        let mut b = body.clone();
        if i == 150 {
            b.push_str("needle");
        }
        s.set_response_body(b.as_bytes());
        m.apply(SessionEvent::New(s));
    }
    m
}

fn worst(m: &SessionModel) -> (Duration, usize) {
    let mut w = Duration::ZERO;
    let mut n = 0;
    for _ in 0..5 {
        let t = Instant::now();
        n = m.visible().len();
        w = w.max(t.elapsed());
    }
    (w, n)
}

#[test]
fn search_in_big_bodies_stays_fast() {
    let mut m = model();
    let (plain, n) = worst(&m);
    assert_eq!(n, 300);
    m.search_query = "needle".into();
    m.search_in_bodies = true;
    let (search, n) = worst(&m);
    assert_eq!(n, 1);
    m.search_query.clear();
    m.filter = "needle".into();
    let (filter, _) = worst(&m);
    eprintln!("visible(): plain {plain:?}, body search {search:?}, quick filter {filter:?}");
    for (what, d) in [("plain", plain), ("search", search), ("filter", filter)] {
        assert!(d < Duration::from_millis(250), "{what}: {d:?} per frame");
    }
}

#[test]
fn sorting_100k_rows_per_frame_stays_fast() {
    use ferrotap::model::SortCol;
    let mut m = SessionModel::default();
    for i in 1..=100_000usize {
        m.apply(SessionEvent::New(SessionRecord { id: i, method: "GET".into(), url: format!("https://h{}.test/p/{}", (i * 7919) % 1000, i), status: 200, duration_ms: ((i * 31) % 997) as _, ..Default::default() }));
    }
    let mut report = Vec::new();
    for col in [SortCol::Url, SortCol::Duration, SortCol::Status] {
        m.sort = Some((col, true));
        let (d, n) = worst(&m);
        assert_eq!(n, 100_000);
        report.push(format!("{col:?}: {d:?}"));
        assert!(d < Duration::from_millis(400), "sorting by {col:?}: {d:?} per frame");
    }
    eprintln!("sorted visible(): {}", report.join(", "));
}
