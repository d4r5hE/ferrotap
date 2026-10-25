//! G7 collection folders: create, rename / move, delete, move a request; the tree renders.
use eframe::egui;
use ferrotap::model::{Collection, SavedRequest};

fn req(name: &str) -> SavedRequest {
    SavedRequest { name: name.into(), method: "GET".into(), url: "https://x.test/".into(), ..Default::default() }
}

#[test]
fn folder_operations() {
    let mut c = Collection::new("c");
    for n in ["root req", "auth/login", "auth/v2/token", "users/list"] {
        c.upsert(req(n));
    }
    assert_eq!(c.folder_list(), vec!["auth", "auth/v2", "users"]);
    c.create_folder("empty/inner").unwrap();
    assert!(c.create_folder("auth").is_err());
    assert!(c.create_folder(" / ").is_err());
    assert_eq!(c.folder_list(), vec!["auth", "auth/v2", "empty", "empty/inner", "users"]);

    assert_eq!(c.rename_folder("auth", "security/auth").unwrap(), 2);
    assert!(c.find("security/auth/v2/token").is_some() && c.find("auth/login").is_none());
    assert!(c.rename_folder("security", "security/x").is_err(), "cannot move into itself");

    assert_eq!(c.move_request("users/list", "").unwrap(), "list");
    assert_eq!(c.move_request("list", "security").unwrap(), "security/list");
    c.upsert(req("list"));
    assert!(c.move_request("list", "security").is_err(), "name taken");

    assert_eq!(c.delete_folder("security"), 3);
    assert_eq!(c.requests.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["root req", "list"]);
    assert_eq!(c.folder_list(), vec!["empty", "empty/inner"]);
    let back: Collection = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
    assert_eq!(back, c, "empty folders are saved");
}

fn collect_text(shape: &egui::Shape, out: &mut String) {
    match shape {
        egui::Shape::Text(t) => {
            out.push_str(t.galley.text());
            out.push('\n');
        }
        egui::Shape::Vec(v) => v.iter().for_each(|s| collect_text(s, out)),
        _ => {}
    }
}

#[test]
fn tree_renders_folders() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("FERROTAP_COLLECTIONS_DIR", dir.path());
    let (_tx, rx) = std::sync::mpsc::channel();
    if std::env::var_os("FERROTAP_UI_STATE").is_none() { std::env::set_var("FERROTAP_UI_STATE", std::env::temp_dir().join(format!("ferrotap_ui_test_{}.json", std::process::id()))); }
    let mut app = ferrotap::ui::GridApp::new(rx, "test");
    let mut c = Collection::new("API");
    c.upsert(req("auth/login"));
    c.upsert(req("ping"));
    app.comp_collections = vec![c];
    let ctx = egui::Context::default();
    // open the collapsing header first
    ctx.data_mut(|d| d.insert_persisted(egui::Id::new(("coll", "API")).with("collapsing"), true));
    let mut text = String::new();
    for _ in 0..2 {
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            let _ = app.composer_collections_tree(ui);
        });
        out.textures_delta.clear();
        text.clear();
        for s in &out.shapes {
            collect_text(&s.shape, &mut text);
        }
    }
    assert!(text.contains("API (2)"), "{text}");
}
