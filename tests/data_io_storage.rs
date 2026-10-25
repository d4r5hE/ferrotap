//! G6: own format (.ftap) round trip, snapshots in folders, auto-save pruning, compare.
use std::collections::BTreeMap;

use ferrotap::data_io::{self, ExportFormat};
use ferrotap::model::SessionRecord;

fn rec(id: usize, method: &str, url: &str, status: u16, body: &str, dur: u128) -> SessionRecord {
    let mut rq = BTreeMap::new();
    rq.insert("Host".to_string(), "example.com".to_string());
    SessionRecord {
        id,
        method: method.to_string(),
        url: url.to_string(),
        status,
        request_headers: rq,
        response_body: body.to_string(),
        request_body: "req".to_string(),
        start_time: "2026-10-16T10:00:00+00:00".to_string(),
        duration_ms: dur,
        comment: "c".to_string(),
        ttfb_ms: Some(4),
        response_trailers: vec![("Signature".to_string(), "x".to_string())],
        ..Default::default()
    }
}

#[test]
fn ftap_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("s.ftap");
    let s = vec![rec(1, "GET", "http://a/1", 200, "one", 10), rec(2, "PUT", "http://a/2", 500, "two", 20)];
    assert_eq!(data_io::export_sessions(&p, ExportFormat::Ftap, &s).unwrap(), 2);
    let back = data_io::import_path(&p).unwrap();
    assert_eq!(back.len(), 2);
    for (a, b) in s.iter().zip(&back) {
        assert_eq!(serde_json::to_value(a).unwrap(), serde_json::to_value(b).unwrap());
    }
    // a plain array (old sessions.json dump) is accepted too
    let p2 = dir.path().join("dump.json");
    std::fs::write(&p2, serde_json::to_string(&s).unwrap()).unwrap();
    assert_eq!(data_io::import_path(&p2).unwrap().len(), 2);
    // a folder of session files imports all of them (har + ftap + json)
    data_io::export_sessions(&dir.path().join("t.har"), ExportFormat::Har, &s).unwrap();
    assert_eq!(data_io::import_path(dir.path()).unwrap().len(), 6);
    // garbage is an error
    let p3 = dir.path().join("bad.ftap");
    std::fs::write(&p3, "{\"format\":\"other\",\"version\":1,\"sessions\":[]}").unwrap();
    assert!(data_io::import_path(&p3).is_err());
}

#[test]
fn snapshots_save_list_rename_clone_delete() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let s = vec![rec(1, "GET", "http://a/1", 200, "one", 10)];

    assert!(data_io::list_snapshots(root).is_empty());
    data_io::create_folder(root, "Team A").unwrap();
    assert_eq!(data_io::list_folders(root), vec!["Team A".to_string()]);

    let p1 = data_io::save_snapshot(root, "", "first", &s).unwrap();
    let p2 = data_io::save_snapshot(root, "Team A", "second", &s).unwrap();
    assert!(p2.starts_with(root.join("Team A")));
    assert!(data_io::save_snapshot(root, "", "first", &s).is_err(), "duplicate name must fail");
    assert!(data_io::save_snapshot(root, "", "  ", &s).is_err());

    let list = data_io::list_snapshots(root);
    let labels: Vec<String> = list.iter().map(|x| x.label()).collect();
    assert_eq!(labels, vec!["first".to_string(), "Team A/second".to_string()]);
    assert_eq!(data_io::load_snapshot(&p1).unwrap()[0].url, "http://a/1");

    let r = data_io::rename_snapshot(&p1, "renamed").unwrap();
    assert!(!p1.exists() && r.exists());
    assert!(data_io::rename_snapshot(&r, "renamed").is_ok());
    let c1 = data_io::clone_snapshot(&r).unwrap();
    let c2 = data_io::clone_snapshot(&r).unwrap();
    assert_eq!(c1.file_stem().unwrap(), "renamed copy");
    assert_eq!(c2.file_stem().unwrap(), "renamed copy 2");
    assert!(data_io::rename_snapshot(&c1, "renamed").is_err(), "rename onto an existing name must fail");
    assert_eq!(data_io::load_snapshot(&c2).unwrap().len(), 1);

    data_io::delete_snapshot(&c1).unwrap();
    data_io::delete_snapshot(&p2).unwrap();
    let labels: Vec<String> = data_io::list_snapshots(root).iter().map(|x| x.label()).collect();
    assert_eq!(labels, vec!["renamed".to_string(), "renamed copy 2".to_string()]);
}

#[test]
fn snapshots_root_env_override() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("FERROTAP_SNAPSHOTS", dir.path());
    assert_eq!(data_io::snapshots_root(), dir.path());
    std::env::remove_var("FERROTAP_SNAPSHOTS");
    assert!(data_io::snapshots_root().ends_with("Documents/Ferrotap/Snapshots") || data_io::snapshots_root().ends_with(r"Documents\Ferrotap\Snapshots"));
}

#[test]
fn autosave_keeps_last_n() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path().join("auto");
    let s = vec![rec(1, "GET", "http://a/1", 200, "one", 10)];
    for i in 0..5 {
        data_io::autosave_write(&d, &format!("20261016-10000{i}"), &s, 3).unwrap();
    }
    std::fs::write(d.join("keep-me.ftap"), "x").unwrap();
    let mut names: Vec<String> = std::fs::read_dir(&d).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names, vec!["autosave-20261016-100002.ftap", "autosave-20261016-100003.ftap", "autosave-20261016-100004.ftap", "keep-me.ftap"]);
    let deleted = data_io::prune_autosaves(&d, 1).unwrap();
    assert_eq!(deleted.len(), 2);
    assert!(d.join("autosave-20261016-100004.ftap").exists());
    assert!(d.join("keep-me.ftap").exists());
    assert_eq!(data_io::load_snapshot(&d.join("autosave-20261016-100004.ftap")).unwrap().len(), 1);

    // config persists; off by default
    let cfg = data_io::AutoSaveCfg::load(dir.path());
    assert!(!cfg.enabled);
    let mut cfg2 = cfg.clone();
    cfg2.enabled = true;
    cfg2.interval_min = 3;
    cfg2.keep = 4;
    cfg2.save(dir.path()).unwrap();
    assert_eq!(data_io::AutoSaveCfg::load(dir.path()), cfg2);
    assert_eq!(cfg2.dir(dir.path()), dir.path().join("Autosave"));
}

#[test]
fn compare_snapshots() {
    let a = vec![
        rec(1, "GET", "http://a/same", 200, "x", 10),
        rec(2, "GET", "http://a/status", 200, "x", 10),
        rec(3, "GET", "http://a/size", 200, "x", 10),
        rec(4, "GET", "http://a/slow", 200, "x", 10),
        rec(5, "GET", "http://a/gone", 200, "x", 10),
        rec(6, "GET", "http://a/dup", 200, "x", 10),
    ];
    let b = vec![
        rec(11, "GET", "http://a/same", 200, "x", 12),
        rec(12, "GET", "http://a/status", 404, "x", 10),
        rec(13, "GET", "http://a/size", 200, "xyz", 10),
        rec(14, "GET", "http://a/slow", 200, "x", 900),
        rec(15, "POST", "http://a/same", 200, "x", 10),
        rec(16, "GET", "http://a/dup", 200, "x", 10),
        rec(17, "GET", "http://a/dup", 200, "x", 10),
    ];
    let r = data_io::compare_sessions(&a, &b, 5);
    assert_eq!(r.added.iter().map(|x| (x.0.as_str(), x.1.as_str())).collect::<Vec<_>>(), vec![("POST", "http://a/same"), ("GET", "http://a/dup")]);
    assert_eq!(r.removed.iter().map(|x| x.1.as_str()).collect::<Vec<_>>(), vec!["http://a/gone"]);
    let ch: Vec<(&str, Vec<&str>)> = r.changed.iter().map(|c| (c.url.as_str(), c.what.clone())).collect();
    assert_eq!(ch, vec![("http://a/status", vec!["status"]), ("http://a/size", vec!["size"]), ("http://a/slow", vec!["duration"])]);
    assert_eq!(r.changed[0].old.0, 200);
    assert_eq!(r.changed[0].new.0, 404);
    assert_eq!(r.same, 2);
    // with zero tolerance the 10 -> 12 ms change counts too
    assert_eq!(data_io::compare_sessions(&a, &b, 0).changed.len(), 4);
}
