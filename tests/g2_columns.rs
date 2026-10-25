// G2 columns: process / PID / IP / request-size columns, reordering, and custom columns ("Add column...").
use ferrotap::g2::*;
use ferrotap::model::*;
use ferrotap::prefs::UiPrefs;
use ferrotap::ui::{custom_source, CUSTOM_SOURCE_KINDS};
use std::collections::BTreeMap;

fn session(id: usize, pid: u32, proc_name: &str, req_body: &str, resp_body: &str) -> SessionRecord {
    let mut req = BTreeMap::new();
    req.insert("Host".to_string(), "api.example.com".to_string());
    req.insert("X-Trace".to_string(), format!("t{id}"));
    req.insert("Cookie".to_string(), format!("sid=s{id}; theme=dark"));
    let mut resp = BTreeMap::new();
    resp.insert("Content-Type".to_string(), "application/json".to_string());
    resp.insert("X-Server".to_string(), "edge-1".to_string());
    SessionRecord {
        id,
        method: "POST".to_string(),
        url: format!("https://api.example.com/v1/items?page={id}&q=x"),
        status: 200,
        http_version: "HTTP/1.1".to_string(),
        response_http_version: "HTTP/1.1".to_string(),
        request_headers: req,
        response_headers: resp,
        request_body: req_body.to_string(),
        response_body: resp_body.to_string(),
        start_time: "2026-10-16 12:00:00.000".to_string(),
        duration_ms: 10,
        mark: None,
        comment: String::new(),
        response_raw_size: None,
        response_decoded_size: None,
        response_trailers: vec![],
        process_name: Some(proc_name.to_string()),
        pid: Some(pid),
        client_ip: Some(format!("127.0.0.{id}")),
        remote_ip: Some("93.184.216.34".to_string()),
        tls_version: None,
        dns_ms: None,
        connect_ms: None,
        tls_ms: None,
        ttfb_ms: None,
        ..Default::default()
    }
}

#[test]
fn new_columns_start_hidden_and_old_layouts_gain_them() {
    let d = ColumnState::default();
    for c in [GridColumn::ProcessName, GridColumn::Pid, GridColumn::ClientIp, GridColumn::RemoteIp, GridColumn::ReqSize] {
        assert!(!d.is_visible(c), "{:?} should start hidden", c);
    }
    assert!(d.is_visible(GridColumn::Url));

    // a layout saved before these columns existed
    let mut old = ColumnState { columns: d.columns.iter().filter(|c| !c.col.hidden_by_default()).cloned().collect() };
    let before: Vec<GridColumn> = old.columns.iter().map(|c| c.col).collect();
    old.normalize();
    assert_eq!(old.columns.len(), GridColumn::ALL.len());
    let after: Vec<GridColumn> = old.columns.iter().map(|c| c.col).collect();
    assert_eq!(&after[..before.len()], &before[..], "saved order is kept");
    assert!(!old.is_visible(GridColumn::Pid));
}

#[test]
fn columns_reorder() {
    let mut s = ColumnState::default();
    let first = s.columns[0].col;
    let second = s.columns[1].col;
    s.move_col(second, -1);
    assert_eq!(s.columns[0].col, second);
    assert_eq!(s.columns[1].col, first);
    s.move_col(second, -1); // already first: no change
    assert_eq!(s.columns[0].col, second);
    let last = s.columns.last().unwrap().col;
    s.move_col(last, 1); // already last: no change
    assert_eq!(s.columns.last().unwrap().col, last);
}

#[test]
fn sort_by_new_columns() {
    let mut m = SessionModel::default();
    m.rows = vec![session(1, 300, "b.exe", "xx", "{}"), session(2, 100, "c.exe", "x", "{}"), session(3, 200, "a.exe", "xxx", "{}")];
    m.sort = Some((SortCol::Pid, true));
    assert_eq!(m.visible().iter().map(|&i| m.rows[i].id).collect::<Vec<_>>(), vec![2, 3, 1]);
    m.sort = Some((SortCol::ProcessName, true));
    assert_eq!(m.visible().iter().map(|&i| m.rows[i].id).collect::<Vec<_>>(), vec![3, 1, 2]);
    m.sort = Some((SortCol::ReqSize, false));
    assert_eq!(m.visible().iter().map(|&i| m.rows[i].id).collect::<Vec<_>>(), vec![3, 1, 2]);
}

#[test]
fn custom_column_sources() {
    let s = session(7, 1, "curl.exe", "token=abc123", r#"{"user": {"id": 42, "name": "Ada"}, "items": [{"sku": "A1"}]}"#);
    let val = |kind: usize, arg: &str| CustomColumnDef::new("c", "c", custom_source(kind, arg)).extract_value(&s);
    assert_eq!(CUSTOM_SOURCE_KINDS.len(), 7);
    assert_eq!(val(0, "x-trace"), "t7"); // request header, case-insensitive
    assert_eq!(val(0, "X-Server"), ""); // a response header is not a request header
    assert_eq!(val(1, "x-server"), "edge-1"); // response header
    assert_eq!(val(2, "sid"), "s7"); // cookie
    assert_eq!(val(3, "page"), "7"); // query parameter
    assert_eq!(val(4, "user.name"), "Ada"); // JSON path
    assert_eq!(val(4, "user.id"), "42");
    assert_eq!(val(4, "items.0.sku"), "A1");
    assert_eq!(val(4, "user.missing"), "");
    assert_eq!(val(5, r"token=(\w+)"), "abc123"); // body regex, first group, request body as fallback
    assert_eq!(val(5, r#""name": "(\w+)""#), "Ada");
    assert_eq!(val(5, "(unclosed"), ""); // a bad regex shows nothing instead of failing
}

#[test]
fn custom_columns_and_layout_persist() {
    let dir = std::env::temp_dir().join(format!("ferrotap_cols_test_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("ui_state.json");

    let mut p = UiPrefs::default();
    p.custom_columns.push(CustomColumnDef::new("cc-0", "Trace", custom_source(0, "X-Trace")));
    p.custom_columns.push(CustomColumnDef::new("cc-1", "User", custom_source(4, "user.name")));
    p.columns.move_col(GridColumn::Url, -1);
    p.columns.set_visible(GridColumn::Pid, true);
    p.save(&path).unwrap();

    let back = UiPrefs::load(&path);
    assert_eq!(back.custom_columns.len(), 2);
    assert_eq!(back.custom_columns[0].title, "Trace");
    assert_eq!(back.custom_columns[1].source, ColumnSource::JsonPath("user.name".into()));
    let mut m = SessionModel::default();
    back.to_model(&mut m);
    assert!(m.column_state.is_visible(GridColumn::Pid));
    assert_eq!(m.column_state.columns.iter().map(|c| c.col).collect::<Vec<_>>(),
               p.columns.columns.iter().map(|c| c.col).collect::<Vec<_>>());
    let _ = std::fs::remove_dir_all(&dir);
}
