//! G7 Composer extras: a cookies editor (kept in step with the Cookie header) and raw text mode
//! for query parameters and cookies (`name=value` per line, `#` disables a line).
use crate::model::KvRow;

/// `name=value` lines (`#` prefix = disabled).
pub fn pairs_to_raw(rows: &[KvRow]) -> String {
    rows.iter().map(|r| format!("{}{}={}", if r.enabled { "" } else { "# " }, r.key, r.value)).collect::<Vec<_>>().join("\n")
}

pub fn raw_to_pairs(text: &str) -> Vec<KvRow> {
    text.lines()
        .filter_map(|l| {
            let t = l.trim();
            let (enabled, t) = match t.strip_prefix('#') {
                Some(r) => (false, r.trim()),
                None => (true, t),
            };
            if t.is_empty() {
                return None;
            }
            let (k, v) = t.split_once('=').unwrap_or((t, ""));
            (!k.trim().is_empty()).then(|| KvRow { enabled, key: k.trim().to_string(), value: v.trim().to_string() })
        })
        .collect()
}

/// Cookies of the (enabled) Cookie header rows.
pub fn cookies_from_headers(headers: &[KvRow]) -> Vec<KvRow> {
    headers
        .iter()
        .filter(|h| h.enabled && h.key.eq_ignore_ascii_case("cookie"))
        .flat_map(|h| crate::inspect::parse_cookie_header(&h.value))
        .map(|(k, v)| KvRow { enabled: true, key: k, value: v })
        .collect()
}

/// Put the enabled cookies into a single Cookie header (removed when there are none).
pub fn set_cookie_header(headers: &mut Vec<KvRow>, cookies: &[KvRow]) {
    let value = cookies.iter().filter(|c| c.enabled && !c.key.trim().is_empty()).map(|c| format!("{}={}", c.key.trim(), c.value)).collect::<Vec<_>>().join("; ");
    let pos = headers.iter().position(|h| h.key.eq_ignore_ascii_case("cookie"));
    headers.retain(|h| !h.key.eq_ignore_ascii_case("cookie"));
    if !value.is_empty() {
        let row = KvRow { enabled: true, key: "Cookie".into(), value };
        match pos {
            Some(i) if i <= headers.len() => headers.insert(i, row),
            _ => headers.push(row),
        }
    }
}

#[derive(Default)]
pub struct ComposerMore {
    pub params_raw: bool,
    pub params_raw_text: String,
    pub cookies: Vec<KvRow>,
    pub cookies_raw: bool,
    pub cookies_raw_text: String,
    /// Cookie header value the cookie table was loaded from (reload when the header changes).
    pub cookies_from: Option<String>,
}

impl crate::ui::GridApp {
    /// Params tab with a Table / Raw switch; false = draw the table (the caller's editor).
    pub fn composer_params_raw(&mut self, ui: &mut eframe::egui::Ui) -> bool {
        use eframe::egui::{self, Color32, RichText};
        let st = &mut self.ext.composer;
        ui.horizontal(|ui| {
            let was = st.params_raw;
            ui.radio_value(&mut st.params_raw, false, "Table");
            ui.radio_value(&mut st.params_raw, true, "Raw");
            if !was && st.params_raw {
                st.params_raw_text = pairs_to_raw(&self.comp_params);
            }
        });
        if !st.params_raw {
            return false;
        }
        ui.label(RichText::new("One 'name=value' per line; a leading '#' disables the line.").small().color(Color32::GRAY));
        if ui.add(egui::TextEdit::multiline(&mut st.params_raw_text).code_editor().desired_rows(5).desired_width(f32::INFINITY)).changed() {
            self.comp_params = raw_to_pairs(&st.params_raw_text);
        }
        true
    }

    /// Cookies tab: table or raw editor over the Cookie header.
    pub fn composer_cookies(&mut self, ui: &mut eframe::egui::Ui) {
        use eframe::egui::{self, Color32, RichText};
        let header_now: Option<String> = self.comp_hdr_rows.iter().find(|h| h.key.eq_ignore_ascii_case("cookie")).map(|h| h.value.clone());
        let st = &mut self.ext.composer;
        if st.cookies_from != header_now || (st.cookies.is_empty() && header_now.is_some()) {
            st.cookies = cookies_from_headers(&self.comp_hdr_rows);
            st.cookies_raw_text = pairs_to_raw(&st.cookies);
            st.cookies_from = header_now.clone();
        }
        let mut changed = false;
        ui.horizontal(|ui| {
            let was = st.cookies_raw;
            ui.radio_value(&mut st.cookies_raw, false, "Table");
            ui.radio_value(&mut st.cookies_raw, true, "Raw");
            if !was && st.cookies_raw {
                st.cookies_raw_text = pairs_to_raw(&st.cookies);
            }
        });
        ui.label(RichText::new("Edits the request's Cookie header.").small().color(Color32::GRAY));
        if st.cookies_raw {
            if ui.add(egui::TextEdit::multiline(&mut st.cookies_raw_text).code_editor().desired_rows(5).desired_width(f32::INFINITY)).changed() {
                st.cookies = raw_to_pairs(&st.cookies_raw_text);
                changed = true;
            }
        } else {
            let mut del = None;
            egui::Grid::new("comp_cookies_grid").striped(true).show(ui, |ui| {
                for (i, c) in st.cookies.iter_mut().enumerate() {
                    changed |= ui.checkbox(&mut c.enabled, "").changed();
                    changed |= ui.add(egui::TextEdit::singleline(&mut c.key).hint_text("name").desired_width(160.0)).changed();
                    changed |= ui.add(egui::TextEdit::singleline(&mut c.value).hint_text("value").desired_width(260.0)).changed();
                    if ui.small_button("x").clicked() {
                        del = Some(i);
                    }
                    ui.end_row();
                }
            });
            if let Some(i) = del {
                st.cookies.remove(i);
                changed = true;
            }
            if ui.small_button("Add cookie").clicked() {
                st.cookies.push(KvRow { enabled: true, key: String::new(), value: String::new() });
            }
        }
        if changed {
            set_cookie_header(&mut self.comp_hdr_rows, &st.cookies);
            st.cookies_from = self.comp_hdr_rows.iter().find(|h| h.key.eq_ignore_ascii_case("cookie")).map(|h| h.value.clone());
        }
    }
}

/// Collections tree state (folder name inputs).
#[derive(Default)]
pub struct TreeUi {
    pub new_folder: String,
    /// (collection, folder, new name) being renamed.
    pub rename: Option<(String, String, String)>,
    pub confirm_delete: Option<(String, String)>,
}

/// What the tree asks the Composer to do.
pub enum TreeAction {
    Load(String, String),
    Delete(String, String),
}

type CollChange = Box<dyn FnOnce(&mut crate::model::Collection) -> Result<String, String>>;

impl crate::ui::GridApp {
    /// Collections with folders: open / delete / move requests, create / rename / delete folders.
    pub fn composer_collections_tree(&mut self, ui: &mut eframe::egui::Ui) -> Option<TreeAction> {
        use eframe::egui::{self, Color32, RichText};
        let mut act = None;
        let mut change: Option<(String, CollChange)> = None;
        let colls = self.comp_collections.clone();
        let t = &mut self.ext.tree;
        for c in &colls {
            egui::CollapsingHeader::new(format!("{} ({})", c.name, c.requests.len())).id_salt(("coll", &c.name)).show(ui, |ui| {
                let folders = c.folder_list();
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut t.new_folder).hint_text("new folder (a/b)").desired_width(140.0));
                    if ui.small_button("New folder").clicked() {
                        let f = t.new_folder.clone();
                        t.new_folder.clear();
                        change = Some((c.name.clone(), Box::new(move |c| c.create_folder(&f).map(|_| format!("Folder \"{f}\" created")))));
                    }
                });
                let mut rows: Vec<(usize, &crate::model::SavedRequest)> = c.requests.iter().filter(|r| !r.name.contains('/')).map(|r| (0, r)).collect();
                let mut items: Vec<(usize, Option<&String>, Option<&crate::model::SavedRequest>)> = rows.drain(..).map(|(d, r)| (d, None, Some(r))).collect();
                for f in &folders {
                    let depth = f.matches('/').count();
                    items.push((depth, Some(f), None));
                    for r in c.requests.iter().filter(|r| r.name.rsplit_once('/').is_some_and(|(p, _)| p == f)) {
                        items.push((depth + 1, None, Some(r)));
                    }
                }
                for (depth, folder, req) in items {
                    ui.horizontal(|ui| {
                        ui.add_space(14.0 * depth as f32);
                        if let Some(r) = req {
                            let leaf = r.name.rsplit('/').next().unwrap_or(&r.name);
                            if ui.button(format!("{} {leaf}", r.method)).clicked() {
                                act = Some(TreeAction::Load(c.name.clone(), r.name.clone()));
                            }
                            egui::ComboBox::from_id_salt(("mv", &c.name, &r.name)).selected_text("Move to...").width(90.0).show_ui(ui, |ui| {
                                for f in std::iter::once(String::new()).chain(folders.iter().cloned()) {
                                    let label = if f.is_empty() { "(top level)".to_string() } else { f.clone() };
                                    if ui.selectable_label(false, label).clicked() {
                                        let n = r.name.clone();
                                        change = Some((c.name.clone(), Box::new(move |c| c.move_request(&n, &f).map(|x| format!("Moved to \"{x}\"")))));
                                    }
                                }
                            });
                            if ui.small_button("Delete").clicked() {
                                act = Some(TreeAction::Delete(c.name.clone(), r.name.clone()));
                            }
                        } else if let Some(f) = folder {
                            let leaf = f.rsplit('/').next().unwrap_or(f);
                            let renaming = t.rename.as_ref().is_some_and(|(cn, fo, _)| cn == &c.name && fo == f);
                            if renaming {
                                if let Some((_, _, text)) = t.rename.as_mut() {
                                    ui.add(egui::TextEdit::singleline(text).desired_width(140.0));
                                }
                                if ui.small_button("OK").clicked() {
                                    if let Some((_, old, new)) = t.rename.take() {
                                        change = Some((c.name.clone(), Box::new(move |c| c.rename_folder(&old, &new).map(|n| format!("Folder renamed ({n} requests)")))));
                                    }
                                }
                                if ui.small_button("Cancel").clicked() {
                                    t.rename = None;
                                }
                            } else {
                                ui.label(RichText::new(format!("[{leaf}]")).strong()).on_hover_text(f.as_str());
                                if ui.small_button("Rename").on_hover_text("Rename or move (type a/b)").clicked() {
                                    t.rename = Some((c.name.clone(), f.clone(), f.clone()));
                                }
                                if t.confirm_delete.as_ref() == Some(&(c.name.clone(), f.clone())) {
                                    if ui.small_button(RichText::new("Confirm delete").color(Color32::LIGHT_RED)).clicked() {
                                        let fo = f.clone();
                                        t.confirm_delete = None;
                                        change = Some((c.name.clone(), Box::new(move |c| Ok(format!("Folder deleted ({} requests)", c.delete_folder(&fo))))));
                                    }
                                } else if ui.small_button("Delete").clicked() {
                                    t.confirm_delete = Some((c.name.clone(), f.clone()));
                                }
                            }
                        }
                    });
                }
            });
        }
        if let Some((cname, f)) = change {
            if let Some(mut c) = self.comp_collections.iter().find(|c| c.name == cname).cloned() {
                self.comp_msg = match f(&mut c).and_then(|m| crate::model::save_collection(&self.comp_dir, &c).map(|_| m)) {
                    Ok(m) => m,
                    Err(e) => e,
                };
                self.comp_reload();
            }
        }
        act
    }
}
