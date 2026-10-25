import sys
import re
import subprocess

def main():
    print("Starting breakpoint UI integration...")
    ui_path = r'D:\Projects\Ferrotap\src\ui.rs'
    with open(ui_path, 'r', encoding='utf-8') as f:
        code = f.read()

    # Check if already added
    if 'show_breakpoints' in code:
        print("show_breakpoints already in ui.rs")
    else:
        # Add fields to GridApp struct
        gridapp_fields = """    pub show_breakpoints: bool,
    pub bp_rules: Vec<crate::model::BreakpointRule>,
    pub bp_rules_sel: Option<usize>,
    pub bp_msg: String,
    pub bp_edit_url: String,
    pub bp_edit_method: String,
    pub bp_edit_status: String,
    pub bp_edit_headers: String,
    pub bp_edit_body: String,
    pub bp_active_id: Option<u64>,
"""
        code = code.replace("    show_rules: bool,\n", "    show_rules: bool,\n" + gridapp_fields)

        # Add initialization in GridApp::new
        init_fields = """            show_breakpoints: false,
            bp_rules: crate::model::global_breakpoints(),
            bp_rules_sel: None,
            bp_msg: String::new(),
            bp_edit_url: String::new(),
            bp_edit_method: String::new(),
            bp_edit_status: String::new(),
            bp_edit_headers: String::new(),
            bp_edit_body: String::new(),
            bp_active_id: None,
"""
        code = code.replace("            show_rules: false,\n", "            show_rules: false,\n" + init_fields)

        # Add toolbar button next to Rules button
        rules_btn_search = 'if ui.selectable_label(self.show_rules, "Rules").clicked() { self.show_rules = !self.show_rules; }'
        paused_badge = 'let paused_cnt = crate::model::breakpoint_hub().count();\n'
        paused_badge += '                let bp_btn_label = if paused_cnt > 0 { format!("Breakpoints ({})", paused_cnt) } else { "Breakpoints".to_string() };\n'
        paused_badge += '                if ui.selectable_label(self.show_breakpoints, bp_btn_label).clicked() { self.show_breakpoints = !self.show_breakpoints; }'
        code = code.replace(rules_btn_search, rules_btn_search + "\n                " + paused_badge)

        # Add window rendering in ui.rs update() method
        rules_window_call = 'if self.show_rules { self.rules_ui(ctx); }'
        code = code.replace(rules_window_call, rules_window_call + "\n        if self.show_breakpoints { self.breakpoints_ui(ctx); }")

        # Append breakpoints_ui implementation to impl GridApp
        breakpoints_impl = r'''
    // ---------------- Breakpoints UI ----------------
    pub fn breakpoints_ui(&mut self, ctx: &egui::Context) {
        let mut open = true;
        let mut persist = false;
        let mut add_new = false;
        let mut del = None;
        let hub = crate::model::breakpoint_hub();
        let paused_list = hub.list();

        egui::Window::new("Breakpoints & Active Pauses").open(&mut open).default_width(700.0).show(ctx, |ui| {
            ui.horizontal(|ui|
            {
                ui.heading("Breakpoints");
                if ui.button("Add Breakpoint").clicked() {
                    add_new = true;
                }
                if ui.button("Save Rules").clicked() {
                    persist = true;
                    self.bp_msg = "Breakpoints saved.".to_string();
                }
                ui.label(&self.bp_msg);
            });
            ui.label("Pause matching in-flight requests or responses before upstream delivery or client receipt.");
            ui.separator();

            // Rules table/list
            ui.collapsing("Configured Rules", |ui| {
                for (i, r) in self.bp_rules.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut r.enabled, "").changed() { persist = true; }
                        ui.checkbox(&mut r.on_request, "Req");
                        ui.checkbox(&mut r.on_response, "Resp");
                        let meth = if r.method.is_empty() { "ANY" } else { &r.method };
                        ui.label(format!("[{}]", meth));
                        let pat = if r.pattern.is_empty() { "* (any URL)" } else { &r.pattern };
                        let is_sel = self.bp_rules_sel == Some(i);
                        if ui.selectable_label(is_sel, pat).clicked() {
                            self.bp_rules_sel = if is_sel { None } else { Some(i) };
                        }
                        if ui.button("X").clicked() { del = Some(i); }
                    });
                }
            });

            // Rule editor if selected
            if let Some(i) = self.bp_rules_sel {
                if i < self.bp_rules.len() {
                    let r = &mut self.bp_rules[i];
                    ui.separator();
                    ui.label(format!("Editing Breakpoint #{}", i + 1));
                    ui.horizontal(|ui| {
                        ui.label("Name:");
                        ui.text_edit_singleline(&mut r.name);
                    });
                    ui.horizontal(|ui| {
                        ui.label("Method:");
                        ui.text_edit_singleline(&mut r.method);
                        ui.label("Pattern:");
                        ui.text_edit_singleline(&mut r.pattern);
                    });
                    ui.horizontal(|ui| {
                        ui.label("Status filter:");
                        ui.text_edit_singleline(&mut r.status);
                        ui.checkbox(&mut r.on_request, "Pause Request");
                        ui.checkbox(&mut r.on_response, "Pause Response");
                    });
                }
            }

            ui.separator();
            ui.heading(format!("Active Paused Sessions ({})", paused_list.len()));

            if paused_list.is_empty() {
                ui.label("No sessions currently paused at a breakpoint.");
            } else {
                for p in &paused_list {
                    ui.group(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(format!("#{} [Stage: {:?}] {} {}", p.id, p.stage, p.method, p.url));
                            if p.status > 0 {
                                ui.label(format!("Status: {}", p.status));
                            }
                        });
                        ui.horizontal(|ui| {
                            if ui.button("Resume (Unmodified)").clicked() {
                                hub.resume(p.id, crate::model::PausedEdits::default());
                            }
                            if ui.button("Abort / Drop").clicked() {
                                hub.abort(p.id);
                            }
                            if self.bp_active_id == Some(p.id) {
                                if ui.button("Close Inspector").clicked() {
                                    self.bp_active_id = None;
                                }
                            } else if ui.button("Inspect & Modify").clicked() {
                                self.bp_active_id = Some(p.id);
                                self.bp_edit_url = p.url.clone();
                                self.bp_edit_method = p.method.clone();
                                self.bp_edit_status = if p.status > 0 { p.status.to_string() } else { "200".to_string() };
                                self.bp_edit_headers = p.headers.iter().map(|(k, v)| format!("{}: {}\n", k, v)).collect();
                                self.bp_edit_body = String::from_utf8_lossy(&p.body_prefix).to_string();
                            }
                        });
                    });
                }
            }

            // Modification subpanel for paused session
            if let Some(active_id) = self.bp_active_id {
                if let Some(target) = paused_list.iter().find(|p| p.id == active_id) {
                    ui.separator();
                    ui.heading(format!("Modify Paused Session #{}", active_id));
                    ui.horizontal(|ui| {
                        ui.label("Method:");
                        ui.text_edit_singleline(&mut self.bp_edit_method);
                        ui.label("URL:");
                        ui.text_edit_singleline(&mut self.bp_edit_url);
                    });
                    if target.stage == crate::model::BpStage::Response {
                        ui.horizontal(|ui| {
                            ui.label("Status Code:");
                            ui.text_edit_singleline(&mut self.bp_edit_status);
                        });
                    }
                    ui.label("Headers (Header: Value per line):");
                    ui.text_edit_multiline(&mut self.bp_edit_headers);
                    ui.label("Body:");
                    ui.text_edit_multiline(&mut self.bp_edit_body);

                    ui.horizontal(|ui| {
                        if ui.button("Execute Changes & Resume").clicked() {
                            let mut hdrs = Vec::new();
                            for line in self.bp_edit_headers.lines() {
                                if let Some((k, v)) = line.split_once(':') {
                                    hdrs.push((k.trim().to_string(), v.trim().to_string()));
                                }
                            }
                            let st_opt = self.bp_edit_status.trim().parse::<u16>().ok();
                            let edits = crate::model::PausedEdits {
                                url: if self.bp_edit_url != target.url { Some(self.bp_edit_url.clone()) } else { None },
                                method: if self.bp_edit_method != target.method { Some(self.bp_edit_method.clone()) } else { None },
                                status: st_opt,
                                headers: if !hdrs.is_empty() { Some(hdrs) } else { None },
                                body: Some(self.bp_edit_body.as_bytes().to_vec()),
                            };
                            hub.resume(active_id, edits);
                            self.bp_active_id = None;
                        }
                        if ui.button("Abort Session").clicked() {
                            hub.abort(active_id);
                            self.bp_active_id = None;
                        }
                    });
                } else {
                    self.bp_active_id = None;
                }
            }
        });

        if !open { self.show_breakpoints = false; }
        if add_new {
            let mut r = crate::model::BreakpointRule::default();
            r.name = format!("Rule {}", self.bp_rules.len() + 1);
            self.bp_rules.push(r);
            self.bp_rules_sel = Some(self.bp_rules.len() - 1);
            persist = true;
        }
        if let Some(i) = del {
            if i < self.bp_rules.len() {
                self.bp_rules.remove(i);
            }
            self.bp_rules_sel = None;
            persist = true;
        }
        if persist {
            crate::model::set_global_breakpoints(self.bp_rules.clone());
        }
    }
'''
        # Insert before the last closing brace of impl GridApp
        last_brace = code.rfind('}')
        code = code[:last_brace] + breakpoints_impl + "\n}\n"

        with open(ui_path, 'w', encoding='utf-8') as f:
            f.write(code)
        print("Successfully modified src/ui.rs")

if __name__ == '__main__':
    main()
