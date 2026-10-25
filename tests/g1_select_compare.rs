// G1: multi-selection, select related sessions, compare two sessions, more Clear-menu kinds.
use ferrotap::compare::*;
use ferrotap::g1::ClearKind;
use ferrotap::model::*;
use ferrotap::selection::*;
use std::collections::BTreeMap;

fn rec(id: usize, method: &str, url: &str, referer: Option<&str>) -> SessionRecord {
    let mut h = BTreeMap::new();
    if let Some(r) = referer {
        h.insert("Referer".to_string(), r.to_string());
    }
    SessionRecord { id, method: method.into(), url: url.into(), status: 200, request_headers: h, ..Default::default() }
}

#[test]
fn click_ctrl_shift_selection() {
    let order = [5, 3, 9, 1, 7];
    let mut s = Selection::default();
    s.click(3, &order, false, false);
    assert_eq!(s.to_vec(), vec![3]);
    s.click(1, &order, false, true); // shift: range 3..1 in row order = 3, 9, 1
    assert_eq!(s.to_vec(), vec![1, 3, 9]);
    s.click(7, &order, true, false); // ctrl: add 7
    assert_eq!(s.to_vec(), vec![1, 3, 7, 9]);
    s.click(9, &order, true, false); // ctrl again: remove 9
    assert_eq!(s.to_vec(), vec![1, 3, 7]);
    s.click(5, &order, false, true); // shift: from the anchor (the last ctrl-click, 9) back to 5 replaces the selection
    assert_eq!(s.to_vec(), vec![3, 5, 9]);
    s.click(9, &order, false, false); // plain click: only 9
    assert_eq!(s.to_vec(), vec![9]);
    s.set(order);
    assert_eq!(s.len(), 5);
    let rows = vec![rec(1, "GET", "https://a/", None), rec(7, "GET", "https://a/", None)];
    s.retain_existing(&rows);
    assert_eq!(s.to_vec(), vec![1, 7]);
    s.clear();
    assert!(s.is_empty());
}

#[test]
fn related_sessions() {
    let mut rows = vec![
        rec(1, "GET", "https://site.com/page", None),
        rec(2, "GET", "https://cdn.com/app.js", Some("https://site.com/page")),
        rec(3, "GET", "https://site.com/api", Some("https://site.com/page#top")),
        rec(4, "GET", "https://site.com/page", None),
        rec(5, "POST", "https://site.com/page", None),
        rec(6, "GET", "https://cdn.com/x.png", Some("https://site.com/page")),
    ];
    rows[4].request_body = "a=1".into();
    assert_eq!(duplicates_of(&rows, 1), vec![1, 4]);
    assert_eq!(duplicates_of(&rows, 5), vec![5]);
    assert_eq!(same_host(&rows, 2), vec![2, 6]);
    assert_eq!(children_of(&rows, 1), vec![2, 3, 6]);
    assert_eq!(parent_of(&rows, 6), Some(5)); // the latest earlier load of the referring page (a POST here)
    assert_eq!(parent_of(&rows, 2), Some(1));
    assert_eq!(parent_of(&rows, 1), None);
    assert!(duplicates_of(&rows, 99).is_empty());
}

#[test]
fn header_differences_respect_the_ignore_list() {
    let mut a = BTreeMap::new();
    a.insert("Content-Type".to_string(), "application/json".to_string());
    a.insert("Date".to_string(), "Mon".to_string());
    a.insert("X-Only-Left".to_string(), "1".to_string());
    a.insert("Cache-Control".to_string(), "no-cache".to_string());
    let mut b = BTreeMap::new();
    b.insert("content-type".to_string(), "application/json".to_string()); // same value, other case
    b.insert("Date".to_string(), "Tue".to_string());
    b.insert("Cache-Control".to_string(), "max-age=60".to_string());
    b.insert("X-Only-Right".to_string(), "2".to_string());
    let d = header_diff(&a, &b, &ignore_set(DEFAULT_IGNORE));
    let names: Vec<&str> = d.iter().map(|x| x.name.as_str()).collect();
    assert_eq!(names, vec!["Cache-Control", "X-Only-Left", "X-Only-Right"]);
    assert_eq!(d[0].left.as_deref(), Some("no-cache"));
    assert_eq!(d[0].right.as_deref(), Some("max-age=60"));
    assert_eq!(d[1].right, None);
    assert_eq!(d[2].left, None);
    let all = header_diff(&a, &b, &ignore_set(""));
    assert!(all.iter().any(|x| x.name == "Date"));
}

#[test]
fn body_line_diff() {
    let d = line_diff("a\nb\nc\nd", "a\nc\nd\ne");
    assert_eq!(d, vec![DiffLine::Same("a".into()), DiffLine::Left("b".into()), DiffLine::Same("c".into()),
                       DiffLine::Same("d".into()), DiffLine::Right("e".into())]);
    assert!(line_diff("same", "same").iter().all(|l| matches!(l, DiffLine::Same(_))));
    assert_eq!(line_diff("", "x"), vec![DiffLine::Right("x".into())]);
    // one-line JSON bodies are pretty-printed so a single changed field shows as one changed line
    let d = line_diff(&diffable_body(r#"{"a":1,"b":2}"#), &diffable_body(r#"{"a":1,"b":3}"#));
    let changed: Vec<&DiffLine> = d.iter().filter(|l| !matches!(l, DiffLine::Same(_))).collect();
    assert_eq!(changed.len(), 2);
    assert_eq!(diffable_body("plain text"), "plain text");
    // XML/HTML bodies are also pretty-printed so a minified doc diffs line by line.
    let xml = diffable_body("<a><b>1</b><c>2</c></a>");
    assert!(xml.lines().count() > 1, "{xml}");
    let d = line_diff(&diffable_body("<a><b>1</b></a>"), &diffable_body("<a><b>9</b></a>"));
    assert!(d.iter().any(|l| matches!(l, DiffLine::Left(_))) && d.iter().any(|l| matches!(l, DiffLine::Right(_))));
}

#[test]
fn new_clear_kinds() {
    let mut m = SessionModel::default();
    m.rows = vec![rec(1, "GET", "https://a/", None), rec(2, "GET", "https://a/b", None), rec(3, "GET", "https://a/c", None)];
    m.rows[0].status = 101;
    m.rows[1].mark = Some(SessionMark::Red);
    assert_eq!(m.clear_kind(ClearKind::Status1xx), 1);
    assert_eq!(m.clear_kind(ClearKind::Marked), 1);
    assert_eq!(m.rows.len(), 1);
    assert_eq!(m.clear_kind(ClearKind::NonMarked), 1);
    assert!(m.rows.is_empty());
    for k in ClearKind::ALL_KINDS {
        assert!(!k.label().is_empty());
    }
}

#[test]
fn hex_dump_layout() {
    let d = ferrotap::inspect::hex_dump(b"GET / HTTP/1.1\r\nHost: a\r\n", 1024);
    let first = d.lines().next().unwrap();
    assert!(first.starts_with("00000000  47 45 54 20 2f 20 48 54  54 50 2f 31 2e 31 0d 0a  GET / HTTP/1.1.."), "{first}");
    assert!(d.lines().nth(1).unwrap().starts_with("00000010  48 6f 73 74"));
    let cut = ferrotap::inspect::hex_dump(&[0u8; 40], 32);
    assert_eq!(cut.lines().count(), 3);
    assert!(cut.contains("8 more bytes not shown"));
    assert!(ferrotap::inspect::hex_dump(b"", 10).contains("(empty body)"));
}
