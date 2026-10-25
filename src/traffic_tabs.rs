//! G2 traffic tabs: the main capture tab plus tabs holding chosen sessions (open from the
//! selection, close, move, next / previous), and the inspector layout toggle (below / beside).
use crate::g2::{TrafficTab, TrafficTabsManager};

impl TrafficTabsManager {
    /// A new tab holding `ids`; becomes the active tab.
    pub fn tab_from(&mut self, name: &str, ids: Vec<usize>) -> usize {
        let i = self.create_tab(name);
        self.tabs[i].session_ids = ids;
        self.tabs[i].is_recording = false;
        i
    }

    /// Move tab `i` left (-1) or right (+1); the main tab stays first.
    pub fn move_tab(&mut self, i: usize, delta: i32) {
        let j = i as i32 + delta;
        if i == 0 || j < 1 || j as usize >= self.tabs.len() {
            return;
        }
        self.tabs.swap(i, j as usize);
        if self.active_tab_index == i {
            self.active_tab_index = j as usize;
        } else if self.active_tab_index == j as usize {
            self.active_tab_index = i;
        }
    }

    pub fn next(&mut self) {
        self.active_tab_index = (self.active_tab_index + 1) % self.tabs.len();
    }

    pub fn prev(&mut self) {
        self.active_tab_index = (self.active_tab_index + self.tabs.len() - 1) % self.tabs.len();
    }

    /// Session ids the active tab is limited to (None = the main tab, everything).
    pub fn active_ids(&self) -> Option<&[usize]> {
        (self.active_tab_index > 0).then(|| self.tabs[self.active_tab_index].session_ids.as_slice())
    }

    /// Add sessions to tab `i` (no duplicates).
    pub fn add_to(&mut self, i: usize, ids: &[usize]) {
        if let Some(t) = self.tabs.get_mut(i) {
            let mut have: std::collections::HashSet<usize> = t.session_ids.iter().copied().collect();
            for id in ids {
                if have.insert(*id) {
                    t.session_ids.push(*id);
                }
            }
        }
    }
}

fn tab_label(t: &TrafficTab, main: bool) -> String {
    if main { t.name.clone() } else { format!("{} ({})", t.name, t.session_ids.len()) }
}

impl crate::ui::GridApp {
    /// The tab bar above the grid.
    pub fn traffic_tabs_bar(&mut self, ui: &mut eframe::egui::Ui) {
        use eframe::egui;
        let sel = self.selected_ids();
        if !ui.ctx().egui_wants_keyboard_input() {
            let (next, prev) = ui.input(|i| (i.modifiers.command && i.key_pressed(egui::Key::PageDown), i.modifiers.command && i.key_pressed(egui::Key::PageUp)));
            if next {
                self.tabs_manager.next();
            }
            if prev {
                self.tabs_manager.prev();
            }
        }
        let tm = &mut self.tabs_manager;
        let mut close: Option<usize> = None;
        let mut mv: Option<(usize, i32)> = None;
        let mut add_sel: Option<usize> = None;
        ui.horizontal_wrapped(|ui| {
            for (i, t) in tm.tabs.iter().enumerate() {
                let r = ui.selectable_label(tm.active_tab_index == i, tab_label(t, i == 0));
                if r.clicked() {
                    tm.active_tab_index = i;
                }
                if i > 0 {
                    r.context_menu(|ui| {
                        if ui.button("Add selected sessions").clicked() {
                            add_sel = Some(i);
                            ui.close();
                        }
                        if ui.button("Move left").clicked() {
                            mv = Some((i, -1));
                            ui.close();
                        }
                        if ui.button("Move right").clicked() {
                            mv = Some((i, 1));
                            ui.close();
                        }
                        if ui.button("Close tab").clicked() {
                            close = Some(i);
                            ui.close();
                        }
                    });
                    if ui.small_button("x").on_hover_text("Close this tab").clicked() {
                        close = Some(i);
                    }
                }
            }
            ui.separator();
            if ui.add_enabled(!sel.is_empty(), egui::Button::new("+ Tab from selection")).on_hover_text("A tab showing only the selected sessions").clicked() {
                let n = tm.tabs.len();
                tm.tab_from(&format!("Tab {n}"), sel.clone());
            }
            if ui.small_button("<").on_hover_text("Previous tab (Ctrl+PageUp)").clicked() {
                tm.prev();
            }
            if ui.small_button(">").on_hover_text("Next tab (Ctrl+PageDown)").clicked() {
                tm.next();
            }
            ui.separator();
            let label = if self.layout_split_horizontal { "Inspector: beside" } else { "Inspector: below" };
            if ui.small_button(label).on_hover_text("Show the inspector below the grid or beside it").clicked() {
                self.layout_split_horizontal = !self.layout_split_horizontal;
            }
            if ui.small_button(if self.show_groups_panel { "Hide side panel" } else { "Show side panel" }).clicked() {
                self.show_groups_panel = !self.show_groups_panel;
            }
        });
        if let Some(i) = add_sel {
            self.tabs_manager.add_to(i, &sel);
        }
        if let Some((i, d)) = mv {
            self.tabs_manager.move_tab(i, d);
        }
        if let Some(i) = close {
            self.tabs_manager.close_tab(i);
        }
    }
}
