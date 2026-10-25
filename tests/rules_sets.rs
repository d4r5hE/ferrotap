//! G5 rule sets: export / import JSON files (and a folder of them); the file picker's listing.
use ferrotap::model::Rule;
use ferrotap::rules_ext::*;

fn rules() -> Vec<Rule> {
    let mut a = Rule { name: "mock api".into(), pattern: "api.example.com".into(), action: "canned".into(), status: 200, body: "{}".into(), group: "mocks".into(), ..Default::default() };
    a.id = 11;
    let b = Rule { name: "slow".into(), pattern: "cdn".into(), action: "modify".into(), delay_ms: 500, ..Default::default() };
    vec![a, b]
}

#[test]
fn round_trip_gives_fresh_ids() {
    let text = rules_to_json(&rules());
    assert!(text.contains(RULESET_FORMAT));
    let back = rules_from_json(&text).unwrap();
    assert_eq!(back.len(), 2);
    assert_eq!(back[0].name, "mock api");
    assert_eq!(back[0].group, "mocks");
    assert_eq!(back[1].delay_ms, 500);
    assert_ne!(back[0].id, 11);
    assert_ne!(back[0].id, back[1].id);
}

#[test]
fn accepts_bare_arrays_and_rejects_junk() {
    let arr = serde_json::to_string(&rules()).unwrap(); // the rules.json layout
    assert_eq!(rules_from_json(&arr).unwrap().len(), 2);
    assert!(rules_from_json("{\"x\":1}").is_err());
    assert!(rules_from_json("nope").is_err());
}

#[test]
fn files_and_folders() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.json");
    let b = dir.path().join("b.json");
    assert_eq!(export_rule_set(&a, &rules()).unwrap(), 2);
    export_rule_set(&b, &rules()[..1]).unwrap();
    std::fs::write(dir.path().join("notes.txt"), "ignored").unwrap();
    assert_eq!(import_rule_set(&a).unwrap().len(), 2);
    let all = import_rule_set(dir.path()).unwrap();
    assert_eq!(all.len(), 3);
    assert!(import_rule_set(&dir.path().join("missing.json")).is_err());

    let (dirs, files) = ferrotap::filepick::list_dir(dir.path(), &["json".to_string()]);
    assert!(dirs.is_empty());
    assert_eq!(files.len(), 2);
    let (_, all_files) = ferrotap::filepick::list_dir(dir.path(), &[]);
    assert_eq!(all_files.len(), 3);
}

#[test]
fn nested_groups_duplicate_and_toggle() {
    let mk = |name: &str, group: &str| Rule { name: name.into(), pattern: "x".into(), group: group.into(), ..Default::default() };
    let mut rules = vec![mk("a", "api/mocks"), mk("b", ""), mk("c", "api"), mk("d", "web")];
    assert_eq!(group_names(&rules), vec!["", "api", "api/mocks", "web"]);
    assert!(in_group("api/mocks", "api") && !in_group("apis", "api") && !in_group("api", "api/mocks"));
    assert_eq!(group_depth("api/mocks"), 1);
    set_group_enabled(&mut rules, "api", false);
    assert!(rules[0].group_off && rules[2].group_off && !rules[3].group_off);
    assert!(!group_enabled(&rules, "api/mocks"));
    set_group_enabled(&mut rules, "api", true);
    let copies = duplicate_group(&rules, "api");
    let groups: Vec<_> = copies.iter().map(|r| r.group.as_str()).collect();
    assert_eq!(groups, ["api (copy)/mocks", "api (copy)"]);
    assert!(copies.iter().all(|c| rules.iter().all(|r| r.id != c.id || c.id == 0)));
    rules.extend(copies);
    assert_eq!(duplicate_group(&rules, "api")[0].group, "api (copy 2)/mocks");
}
