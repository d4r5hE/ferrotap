import sys
P = r"D:\Projects\Ferrotap\src\ui.rs"
t = open(P, encoding="utf-8", newline="").read().replace("\r\n", "\n")

def rep(old, new):
    global t
    if old not in t:
        print("MISSING:", old[:60]); sys.exit(1)
    t = t.replace(old, new, 1)

rep("    pub comp_headers: String,\n", """    pub comp_hdr_rows: Vec<crate::model::KvRow>,
    pub comp_params: Vec<crate::model::KvRow>,
    pub comp_tab: u8,
    pub comp_collections: Vec<crate::model::Collection>,
    pub comp_history: Vec<crate::model::HistoryEntry>,
    pub comp_dir: std::path::PathBuf,
    pub comp_loaded: bool,
    pub comp_save_coll: String,
    pub comp_save_name: String,
    pub comp_msg: String,
    comp_last: Option<crate::model::SavedRequest>,
""")
rep("            comp_headers: String::new(),\n", """            comp_hdr_rows: Vec::new(),
            comp_params: Vec::new(),
            comp_tab: 0,
            comp_collections: Vec::new(),
            comp_history: Vec::new(),
            comp_dir: crate::model::collections_dir(),
            comp_loaded: false,
            comp_save_coll: "My Collection".into(),
            comp_save_name: String::new(),
            comp_msg: String::new(),
            comp_last: None,
""")
rep('"cheaders" => self.comp_headers = v.replace(";;", "\\n"),',
    '"cheaders" => self.comp_hdr_rows = crate::model::rows_from_header_lines(&v.replace(";;", "\\n")),')
rep('"csend" => { if let Some(c) = self.ctx_for_hook.clone() { self.composer_send(&c); } }',
    '''"csend" => { if let Some(c) = self.ctx_for_hook.clone() { self.composer_send(&c); } }
                "cparams" => self.comp_params = crate::model::parse_query(&format!("x?{v}")).1,
                "cclear" => { self.comp_method = "GET".into(); self.comp_url.clear(); self.comp_params.clear(); self.comp_hdr_rows.clear(); self.comp_body.clear(); self.comp_resp = None; }
                "csave" => { if let Some((c, n)) = v.split_once('|') { self.comp_msg = match self.comp_save(c, n) { Ok(()) => format!("saved {n}"), Err(e) => e }; } }
                "cload" => { if let Some((c, n)) = v.split_once('|') { let ok = self.comp_load(c, n); self.comp_msg = format!("load {n}: {ok}"); } }
                "cdel" => { if let Some((c, n)) = v.split_once('|') { let ok = self.comp_delete_request(c, n); self.comp_msg = format!("del {n}: {ok}"); } }
                "chist" => { if let Ok(i) = v.parse::<usize>() { let ok = self.comp_load_history(i); self.comp_msg = format!("hist {i}: {ok}"); } }''')
rep('"url": self.comp_url,', '''"url": self.comp_url, "params": self.comp_params, "headers_rows": self.comp_hdr_rows, "body": self.comp_body,
                "collections": self.comp_collections, "history_len": self.comp_history.len(), "last_history": self.comp_history.first(), "msg": self.comp_msg,''')

marker = "impl GridApp {\n    /// Dispatch the current Composer request"
i = t.index(marker)
t = t[:i] + r'''use crate::model::{Collection, HistoryEntry, KvRow, SavedRequest};

fn kv_editor(ui: &mut egui::Ui, id: &str, rows: &mut Vec<KvRow>) {
    let mut del = None;
    egui::Grid::new(id).num_columns(4).striped(true).show(ui, |ui| {
        for (i, r) in rows.iter_mut().enumerate() {
            ui.checkbox(&mut r.enabled, "");
            ui.add(egui::TextEdit::singleline(&mut r.key).hint_text("name").desired_width(150.0));
            ui.add(egui::TextEdit::singleline(&mut r.value).hint_text("value").desired_width(230.0));
            if ui.small_button("x").clicked() { del = Some(i); }
            ui.end_row();
        }
    });
    if let Some(i) = del { rows.remove(i); }
    if ui.button("+ Add").clicked() { rows.push(KvRow::new("", "")); }
}

impl GridApp {
    fn current_saved(&self, name: &str) -> SavedRequest {
        SavedRequest {
            name: name.to_string(),
            method: self.comp_method.clone(),
            url: self.comp_url.clone(),
            params: self.comp_params.clone(),
            headers: self.comp_hdr_rows.clone(),
            body: self.comp_body.clone(),
        }
    }

    fn apply_saved(&mut self, r: &SavedRequest) {
        self.comp_method = if r.method.is_empty() { "GET".into() } else { r.method.clone() };
        self.comp_url = r.url.clone();
        self.comp_params = r.params.clone();
        self.comp_hdr_rows = r.headers.clone();
        self.comp_body = r.body.clone();
    }

    pub fn comp_reload(&mut self) {
        self.comp_collections = crate::model::load_collections(&self.comp_dir);
        self.comp_history = crate::model::load_history(&self.comp_dir);
        self.comp_loaded = true;
    }

    /// Save the current Composer request into collection `coll` under `name` (file under collections dir).
    pub fn comp_save(&mut self, coll: &str, name: &str) -> Result<(), String> {
        let (coll, name) = (coll.trim(), name.trim());
        if coll.is_empty() || name.is_empty() { return Err("collection and request name are required".into()); }
        self.comp_reload();
        let req = self.current_saved(name);
        let mut c = self.comp_collections.iter().find(|c| c.name == coll).cloned().unwrap_or_else(|| Collection::new(coll));
        c.upsert(req);
        crate::model::save_collection(&self.comp_dir, &c)?;
        self.comp_reload();
        Ok(())
    }

    pub fn comp_load(&mut self, coll: &str, name: &str) -> bool {
        self.comp_reload();
        let r = self.comp_collections.iter().find(|c| c.name == coll).and_then(|c| c.find(name)).cloned();
        match r {
            Some(r) => { self.apply_saved(&r); self.comp_save_coll = coll.to_string(); self.comp_save_name = name.to_string(); true }
            None => false,
        }
    }

    pub fn comp_delete_request(&mut self, coll: &str, name: &str) -> bool {
        self.comp_reload();
        let Some(mut c) = self.comp_collections.iter().find(|c| c.name == coll).cloned() else { return false };
        let ok = c.remove(name);
        if ok && crate::model::save_collection(&self.comp_dir, &c).is_err() { return false; }
        self.comp_reload();
        ok
    }

    pub fn comp_load_history(&mut self, i: usize) -> bool {
        match self.comp_history.get(i).cloned() {
            Some(h) => { self.apply_saved(&h.request); true }
            None => false,
        }
    }

    /// Dispatch the current Composer request through the capture proxy on a worker thread.
    pub fn composer_send(&mut self, ctx: &egui::Context) {
        let sr = self.current_saved("");
        let req = sr.to_composer();
        self.comp_last = Some(sr);
        self.comp_sending = true;
        let tx = self.comp_tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let r = crate::model::composer_send(&req, &format!("127.0.0.1:{}", crate::proxy::PORT));
            let _ = tx.send(r);
            ctx.request_repaint();
        });
    }

    fn poll_composer(&mut self) {
        while let Ok(r) = self.comp_rx.try_recv() {
            self.comp_sending = false;
            if !self.comp_loaded { self.comp_reload(); }
            if let Some(req) = self.comp_last.take() {
                let e = HistoryEntry { ts: crate::model::now_secs(), status: r.status, error: r.error.clone(), elapsed_ms: r.elapsed_ms as u64, request: req };
                crate::model::history_push(&mut self.comp_history, e);
                let _ = crate::model::save_history(&self.comp_dir, &self.comp_history);
            }
            self.comp_resp = Some(r);
        }
    }

    /// Composer window: method, URL, key-value params/headers, body, Send, collections and history.
    pub fn composer_window(&mut self, ctx: &egui::Context) {
        self.poll_composer();
        if !self.show_composer { return; }
        if !self.comp_loaded { self.comp_reload(); }
        let mut open = true;
        let mut do_send = false;
        let mut do_save = false;
        let mut act_load: Option<(String, String)> = None;
        let mut act_del: Option<(String, String)> = None;
        let mut act_hist: Option<usize> = None;
        let mut clear_hist = false;
        egui::Window::new("Composer").open(&mut open).default_width(640.0).show(ctx, |ui| {
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("comp_method").selected_text(self.comp_method.clone()).show_ui(ui, |ui| {
                    for m in ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"] {
                        ui.selectable_value(&mut self.comp_method, m.to_string(), m);
                    }
                });
                let resp = ui.add(egui::TextEdit::singleline(&mut self.comp_url).hint_text("http://127.0.0.1:8898/path").desired_width(340.0));
                if resp.changed() && self.comp_url.contains('?') {
                    let (b, rows) = crate::model::parse_query(&self.comp_url);
                    self.comp_url = b;
                    self.comp_params.extend(rows);
                    self.comp_tab = 0;
                }
                let label = if self.comp_sending { "Sending..." } else { "Send" };
                if ui.add_enabled(!self.comp_sending, egui::Button::new(label)).clicked() { do_send = true; }
            });
            ui.horizontal(|ui| {
                ui.label("Save to collection:");
                ui.add(egui::TextEdit::singleline(&mut self.comp_save_coll).desired_width(120.0));
                ui.label("as");
                ui.add(egui::TextEdit::singleline(&mut self.comp_save_name).hint_text("request name").desired_width(140.0));
                if ui.button("Save").clicked() { do_save = true; }
            });
            if !self.comp_msg.is_empty() { ui.label(RichText::new(&self.comp_msg).small().color(Color32::GRAY)); }
            ui.horizontal(|ui| {
                let np = self.comp_params.iter().filter(|r| r.enabled).count();
                let nh = self.comp_hdr_rows.iter().filter(|r| r.enabled).count();
                ui.selectable_value(&mut self.comp_tab, 0, format!("Params ({np})"));
                ui.selectable_value(&mut self.comp_tab, 1, format!("Headers ({nh})"));
                ui.selectable_value(&mut self.comp_tab, 2, "Body");
            });
            match self.comp_tab {
                0 => kv_editor(ui, "comp_params_grid", &mut self.comp_params),
                1 => kv_editor(ui, "comp_headers_grid", &mut self.comp_hdr_rows),
                _ => { ui.add(egui::TextEdit::multiline(&mut self.comp_body).desired_rows(5).desired_width(f32::INFINITY)); }
            }
            ui.separator();
            ui.collapsing(format!("Collections ({})", self.comp_collections.len()), |ui| {
                for c in &self.comp_collections {
                    ui.collapsing(format!("{} ({})", c.name, c.requests.len()), |ui| {
                        for r in &c.requests {
                            ui.horizontal(|ui| {
                                if ui.button(format!("{} {}", r.method, r.name)).clicked() { act_load = Some((c.name.clone(), r.name.clone())); }
                                if ui.small_button("Delete").clicked() { act_del = Some((c.name.clone(), r.name.clone())); }
                            });
                        }
                    });
                }
            });
            ui.collapsing(format!("History ({})", self.comp_history.len()), |ui| {
                if ui.small_button("Clear history").clicked() { clear_hist = true; }
                egui::ScrollArea::vertical().max_height(120.0).id_salt("comp_hist_scroll").show(ui, |ui| {
                    for (i, h) in self.comp_history.iter().enumerate().take(50) {
                        let st = if h.error.is_empty() { h.status.to_string() } else { "ERR".to_string() };
                        if ui.button(format!("{} {} {}", st, h.request.method, h.request.url)).clicked() { act_hist = Some(i); }
                    }
                });
            });
            ui.separator();
            match &self.comp_resp {
                None => { ui.label(RichText::new("(not sent)").color(Color32::GRAY)); }
                Some(r) if !r.error.is_empty() => { ui.label(RichText::new(format!("Error: {}", r.error)).color(Color32::LIGHT_RED)); }
                Some(r) => {
                    ui.label(RichText::new(format!("{} {}  |  {} ms  |  {} headers", r.status, r.reason, r.elapsed_ms, r.headers.len())).strong());
                    egui::ScrollArea::vertical().max_height(180.0).id_salt("comp_resp_scroll").show(ui, |ui| {
                        for (k, v) in &r.headers { ui.label(RichText::new(format!("{k}: {v}")).monospace().small()); }
                        ui.separator();
                        ui.label(RichText::new(&r.body).monospace());
                    });
                }
            }
        });
        self.show_composer = open;
        if do_save {
            let (c, n) = (self.comp_save_coll.clone(), self.comp_save_name.clone());
            self.comp_msg = match self.comp_save(&c, &n) { Ok(()) => format!("Saved '{}' to '{}'", n.trim(), c.trim()), Err(e) => e };
        }
        if let Some((c, n)) = act_load { let ok = self.comp_load(&c, &n); self.comp_msg = if ok { format!("Loaded '{n}'") } else { format!("Could not load '{n}'") }; }
        if let Some((c, n)) = act_del { let ok = self.comp_delete_request(&c, &n); self.comp_msg = format!("Delete '{n}': {ok}"); }
        if let Some(i) = act_hist { self.comp_load_history(i); }
        if clear_hist { self.comp_history.clear(); let _ = crate::model::save_history(&self.comp_dir, &self.comp_history); }
        if do_send { self.composer_send(ctx); }
    }
}
'''
open(P, "w", encoding="utf-8", newline="\n").write(t)
print("patched")
