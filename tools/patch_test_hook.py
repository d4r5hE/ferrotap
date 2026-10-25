"""Adds a file-driven test hook to src/ui.rs (idempotent). Active only when env FERROTAP_TEST_DIR is set.
Protocol: tools write <dir>/cmd.txt (lines key=value); app executes them, then writes <dir>/dump.json
(visible rows, marks, comments, visible columns, filter state) 3 frames later."""
import sys
P = r"D:\Projects\Ferrotap\src\ui.rs"
s = open(P, encoding="utf-8").read()
if "fn test_hook" in s:
    print("already patched"); sys.exit(0)

def rep(old, new):
    global s
    assert s.count(old) == 1, (old, s.count(old))
    s = s.replace(old, new)

rep("    comment_edit: Option<(usize, String)>,\n}",
    "    comment_edit: Option<(usize, String)>,\n    test_dir: Option<std::path::PathBuf>,\n    dump_in: u8,\n}")
rep("            comment_edit: None,\n",
    "            comment_edit: None,\n            test_dir: std::env::var(\"FERROTAP_TEST_DIR\").ok().map(std::path::PathBuf::from),\n            dump_in: 0,\n")
rep('        Panel::top("toolbar").show(ui, |ui| {', '        self.test_hook();\n        Panel::top("toolbar").show(ui, |ui| {')

s += r'''

impl GridApp {
    /// Test hook (env FERROTAP_TEST_DIR): executes <dir>/cmd.txt through the same model/prefs paths the
    /// controls use, then dumps the resulting UI state to <dir>/dump.json.
    fn test_hook(&mut self) {
        let Some(dir) = self.test_dir.clone() else { return };
        if self.dump_in > 0 {
            self.dump_in -= 1;
            if self.dump_in == 0 {
                self.write_dump(&dir);
            }
            return;
        }
        let cmd = dir.join("cmd.txt");
        let Ok(text) = std::fs::read_to_string(&cmd) else { return };
        let _ = std::fs::remove_file(&cmd);
        for line in text.lines() {
            let line = line.trim();
            let (k, v) = line.split_once('=').unwrap_or((line, ""));
            let by_url = |m: &SessionModel, sub: &str| m.rows.iter().find(|r| r.url.contains(sub)).map(|r| r.id);
            match k {
                "method" => self.prefs.method = v.to_string(),
                "status" => self.prefs.status = v.to_string(),
                "url" => self.prefs.url = v.to_string(),
                "search" => self.model.search_query = v.to_string(),
                "headers" => self.model.search_in_headers = v == "1",
                "bodies" => self.model.search_in_bodies = v == "1",
                "reset" => {
                    self.prefs.method.clear();
                    self.prefs.status.clear();
                    self.prefs.url.clear();
                    self.mark_filter = None;
                    self.model.filter.clear();
                    self.model.search_query.clear();
                    self.model.search_in_headers = true;
                    self.model.search_in_bodies = true;
                }
                "markfilter" => self.mark_filter = SessionMark::parse(v),
                "mark" => {
                    if let Some((c, sub)) = v.split_once('@') {
                        if let Some(id) = by_url(&self.model, sub) {
                            self.model.mark_session(id, SessionMark::parse(c));
                            self.sync_annotation(id);
                        }
                    }
                }
                "comment" => {
                    if let Some((t, sub)) = v.split_once('@') {
                        if let Some(id) = by_url(&self.model, sub) {
                            self.model.set_comment(id, t.to_string());
                            self.sync_annotation(id);
                        }
                    }
                }
                "hidecol" | "showcol" => {
                    for c in self.model.column_state.columns.iter_mut() {
                        if c.col.title().to_string().eq_ignore_ascii_case(v) {
                            c.visible = k == "showcol";
                        }
                    }
                }
                _ => {}
            }
        }
        self.dump_in = 3;
    }

    fn write_dump(&self, dir: &std::path::Path) {
        let vis = self.model.visible();
        let rows: Vec<serde_json::Value> = vis
            .iter()
            .map(|&i| {
                let r = &self.model.rows[i];
                serde_json::json!({"id": r.id, "method": r.method, "status": r.status, "url": r.url,
                    "mark": r.mark.map(|m| m.as_str()), "comment": r.comment})
            })
            .collect();
        let cols: Vec<String> = self.model.column_state.columns.iter().filter(|c| c.visible).map(|c| c.col.title().to_string()).collect();
        let v = serde_json::json!({"total": self.model.rows.len(), "visible": rows, "columns": cols,
            "filters": {"method": self.prefs.method, "status": self.prefs.status, "url": self.prefs.url,
                "search": self.model.search_query}});
        let tmp = dir.join("dump.tmp");
        if std::fs::write(&tmp, serde_json::to_string_pretty(&v).unwrap_or_default()).is_ok() {
            let _ = std::fs::rename(&tmp, dir.join("dump.json"));
        }
    }
}
'''
open(P, "w", encoding="utf-8").write(s)
print("patched")
