import pathlib
p = pathlib.Path(r'D:\Projects\Ferrotap\src\ui.rs')
t = p.read_text(encoding='utf-8').replace('\r\n', '\n')

def rep(old, new):
    global t
    assert t.count(old) >= 1, 'anchor missing: ' + old[:60]
    t = t.replace(old, new, 1)

rep('use crate::model::{SessionEvent, SessionModel, SessionRecord, SortCol};',
    'use crate::model::{ColumnState, GridColumn, SessionEvent, SessionMark, SessionModel, SessionRecord, SortCol};\nuse crate::prefs::UiPrefs;\n\nconst MARKS: [SessionMark; 7] = [SessionMark::Red, SessionMark::Blue, SessionMark::Green, SessionMark::Orange, SessionMark::Purple, SessionMark::Copper, SessionMark::Strikeout];')

rep('    pub https_msg: String,\n}\n',
    '    pub https_msg: String,\n    prefs: UiPrefs,\n    prefs_path: std::path::PathBuf,\n    last_saved: String,\n    mark_filter: Option<SessionMark>,\n    show_columns: bool,\n    comment_edit: Option<(usize, String)>,\n}\n')

rep('        Self {\n            autotest,',
'''        let prefs_path = UiPrefs::default_path();
        let prefs = UiPrefs::load(&prefs_path);
        let mut model = SessionModel::default();
        prefs.to_model(&mut model);
        let last_saved = serde_json::to_string(&prefs).unwrap_or_default();
        Self {
            prefs,
            prefs_path,
            last_saved,
            mark_filter: None,
            show_columns: false,
            comment_edit: None,
            autotest,''')
rep('            model: SessionModel::default(),', '            model,')

rep('while let Ok(ev) = self.rx.try_recv() {',
    'while let Ok(mut ev) = self.rx.try_recv() {\n            if let SessionEvent::New(s) = &mut ev {\n                self.prefs.apply_to(s);\n            }')

rep('        self.https_window(ui.ctx());\n        let visible = self.model.visible();',
'''        Panel::top("filters").show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("Method:");
                ui.add(egui::TextEdit::singleline(&mut self.prefs.method).hint_text("GET").desired_width(60.0));
                ui.label("Status:");
                ui.add(egui::TextEdit::singleline(&mut self.prefs.status).hint_text("404").desired_width(50.0));
                ui.label("URL contains:");
                ui.add(egui::TextEdit::singleline(&mut self.prefs.url).hint_text("/api/").desired_width(160.0));
                ui.label("Mark:");
                egui::ComboBox::from_id_salt("mark_filter")
                    .selected_text(self.mark_filter.map(|m| m.as_str()).unwrap_or("any"))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.mark_filter, None, "any");
                        for m in MARKS {
                            ui.selectable_value(&mut self.mark_filter, Some(m), m.as_str());
                        }
                    });
                ui.separator();
                ui.label("Search:");
                ui.add(egui::TextEdit::singleline(&mut self.model.search_query).hint_text("full text (url, headers, bodies, comments)").desired_width(260.0));
                ui.checkbox(&mut self.model.search_in_headers, "Headers");
                ui.checkbox(&mut self.model.search_in_bodies, "Bodies");
                ui.checkbox(&mut self.model.search_case_sensitive, "Match case");
                if ui.button("Reset filters").clicked() {
                    self.prefs.method.clear();
                    self.prefs.status.clear();
                    self.prefs.url.clear();
                    self.mark_filter = None;
                    self.model.filter.clear();
                    self.model.search_query.clear();
                }
                ui.separator();
                if ui.button("Columns...").clicked() {
                    self.show_columns = !self.show_columns;
                }
            });
        });
        self.model.structured_filter = self.prefs.criteria(self.mark_filter);
        self.columns_window(ui.ctx());
        self.comment_window(ui.ctx());
        self.https_window(ui.ctx());
        let visible = self.model.visible();''')

rep('    /// Run the native window (blocks until closed).',
'''    /// Column picker: show/hide each grid column (persisted).
    pub fn columns_window(&mut self, ctx: &egui::Context) {
        if !self.show_columns { return; }
        let mut open = true;
        egui::Window::new("Columns").open(&mut open).show(ctx, |ui| {
            for c in GridColumn::ALL {
                let mut v = self.model.column_state.is_visible(c);
                if ui.checkbox(&mut v, c.title()).changed() {
                    self.model.column_state.set_visible(c, v);
                }
            }
            if ui.button("Reset to defaults").clicked() {
                self.model.column_state = ColumnState::default();
            }
        });
        if !open { self.show_columns = false; }
    }

    /// Comment editor opened from the row context menu.
    pub fn comment_window(&mut self, ctx: &egui::Context) {
        let Some((id, mut text)) = self.comment_edit.take() else { return };
        let mut save = false;
        let mut cancel = false;
        egui::Window::new(format!("Comment on session #{id}")).collapsible(false).show(ctx, |ui| {
            ui.add(egui::TextEdit::multiline(&mut text).desired_rows(3).desired_width(320.0));
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() { save = true; }
                if ui.button("Cancel").clicked() { cancel = true; }
            });
        });
        if save {
            self.model.set_comment(id, text);
            self.sync_annotation(id);
        } else if !cancel {
            self.comment_edit = Some((id, text));
        }
    }

    fn sync_annotation(&mut self, id: usize) {
        if let Some(r) = self.model.rows.iter().find(|r| r.id == id) {
            self.prefs.sync_row(r);
        }
    }

    /// Run the native window (blocks until closed).''')

i = t.index('        let grew = self.model.rows.len() > self.last_len;')
t = t[:i] + r'''        let grew = self.model.rows.len() > self.last_len;
        self.last_len = self.model.rows.len();

        let cols: Vec<(GridColumn, f32)> = self.model.column_state.columns.iter().filter(|c| c.visible).map(|c| (c.col, c.width)).collect();
        let mut pending_mark: Option<(usize, Option<SessionMark>)> = None;
        let mut pending_comment: Option<usize> = None;
        let mut pending_clear_comment: Option<usize> = None;
        let mut clicked_sort = None;
        let mut clicked_row = None;
        let sort = self.model.sort;

        egui::CentralPanel::default().show(ui, |ui| {
            let mut table = TableBuilder::new(ui).striped(true).sense(egui::Sense::click()).resizable(true);
            for (c, w) in &cols {
                table = table.column(if *c == GridColumn::Url {
                    Column::remainder().at_least(200.0).clip(true)
                } else {
                    Column::initial(*w)
                });
            }
            if self.auto_scroll && grew && !visible.is_empty() {
                table = table.scroll_to_row(visible.len() - 1, Some(egui::Align::BOTTOM));
            }
            table
                .header(22.0, |mut header| {
                    for (c, _) in &cols {
                        header.col(|ui| {
                            let sc = sort_col(*c);
                            let arrow = match sort {
                                Some((x, true)) if x == sc => " \u{25b2}",
                                Some((x, false)) if x == sc => " \u{25bc}",
                                _ => "",
                            };
                            if ui.button(RichText::new(format!("{}{arrow}", c.title())).strong()).clicked() {
                                clicked_sort = Some(sc);
                            }
                        });
                    }
                })
                .body(|body| {
                    body.rows(20.0, visible.len(), |mut row| {
                        let s = &self.model.rows[visible[row.index()]];
                        row.set_selected(self.model.selected == Some(s.id));
                        let mark = s.mark;
                        for (c, _) in &cols {
                            row.col(|ui| {
                                if let Some(bg) = mark.and_then(mark_bg) {
                                    ui.painter().rect_filled(ui.max_rect(), 0.0, bg);
                                }
                                let (text, color) = cell_text(s, *c);
                                let mut rt = RichText::new(text);
                                if let Some(col) = color { rt = rt.color(col); }
                                if mark == Some(SessionMark::Strikeout) { rt = rt.strikethrough(); }
                                ui.label(rt);
                            });
                        }
                        let resp = row.response();
                        if resp.clicked() || resp.secondary_clicked() {
                            clicked_row = Some(s.id);
                        }
                        resp.context_menu(|ui| {
                            ui.label(RichText::new(format!("Session #{}", s.id)).strong());
                            for m in MARKS {
                                let label = if s.mark == Some(m) { format!("\u{2713} Mark {}", m.as_str()) } else { format!("Mark {}", m.as_str()) };
                                if ui.button(label).clicked() {
                                    pending_mark = Some((s.id, Some(m)));
                                    ui.close();
                                }
                            }
                            if ui.add_enabled(s.mark.is_some(), egui::Button::new("Clear mark")).clicked() {
                                pending_mark = Some((s.id, None));
                                ui.close();
                            }
                            ui.separator();
                            if ui.button("Comment...").clicked() {
                                pending_comment = Some(s.id);
                                ui.close();
                            }
                            if ui.add_enabled(!s.comment.is_empty(), egui::Button::new("Clear comment")).clicked() {
                                pending_clear_comment = Some(s.id);
                                ui.close();
                            }
                        });
                    });
                });
        });
        if let Some(c) = clicked_sort {
            self.model.toggle_sort(c);
        }
        if let Some(id) = clicked_row {
            self.model.selected = Some(id);
        }
        if let Some((id, m)) = pending_mark {
            self.model.mark_session(id, m);
            self.sync_annotation(id);
        }
        if let Some(id) = pending_comment {
            let cur = self.model.rows.iter().find(|r| r.id == id).map(|r| r.comment.clone()).unwrap_or_default();
            self.comment_edit = Some((id, cur));
        }
        if let Some(id) = pending_clear_comment {
            self.model.set_comment(id, String::new());
            self.sync_annotation(id);
        }

        // Persist columns / filters / search / marks / comments whenever anything changed.
        self.prefs.from_model(&self.model);
        let snap = serde_json::to_string(&self.prefs).unwrap_or_default();
        if snap != self.last_saved && self.prefs.save(&self.prefs_path).is_ok() {
            self.last_saved = snap;
        }
    }
}

fn sort_col(c: GridColumn) -> SortCol {
    match c {
        GridColumn::Id => SortCol::Id,
        GridColumn::Status => SortCol::Status,
        GridColumn::Protocol => SortCol::Protocol,
        GridColumn::Host => SortCol::Host,
        GridColumn::Url => SortCol::Url,
        GridColumn::Method => SortCol::Method,
        GridColumn::Size => SortCol::Size,
        GridColumn::ContentType => SortCol::ContentType,
        GridColumn::Time => SortCol::Time,
        GridColumn::Duration => SortCol::Duration,
        GridColumn::Comments => SortCol::Comments,
    }
}

fn mark_bg(m: SessionMark) -> Option<Color32> {
    let (r, g, b) = match m {
        SessionMark::Red => (220, 60, 60),
        SessionMark::Blue => (60, 120, 220),
        SessionMark::Green => (60, 180, 90),
        SessionMark::Orange => (240, 150, 40),
        SessionMark::Purple => (150, 80, 200),
        SessionMark::Copper => (184, 115, 51),
        SessionMark::Strikeout => return None,
    };
    Some(Color32::from_rgba_unmultiplied(r, g, b, 80))
}

fn cell_text(s: &SessionRecord, c: GridColumn) -> (String, Option<Color32>) {
    match c {
        GridColumn::Id => (s.id.to_string(), None),
        GridColumn::Status => (s.status.to_string(), Some(status_color(s.status))),
        GridColumn::Protocol => (s.http_version.clone(), None),
        GridColumn::Host => (s.host().to_string(), None),
        GridColumn::Url => (s.path().to_string(), None),
        GridColumn::Method => (s.method.clone(), None),
        GridColumn::Size => (fmt_size(s.body_size()), None),
        GridColumn::ContentType => (s.content_type().to_string(), None),
        GridColumn::Time => (s.time_of_day().to_string(), None),
        GridColumn::Duration => (format!("{} ms", s.duration_ms), None),
        GridColumn::Comments => (s.comment.clone(), None),
    }
}
'''
p.write_text(t, encoding='utf-8', newline='\n')
print('patched')
