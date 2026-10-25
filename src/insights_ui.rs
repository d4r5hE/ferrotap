//! The "Insights" toolbar menu and its windows (G18 + G3 overview). The analysis itself is in `insights.rs`.
//! Every window works on the sessions the grid currently shows; clicking a session selects it in the grid.
use crate::g2::GroupBy;
use crate::insights::*;
use crate::model::SessionRecord;
use crate::ui::GridApp;
use eframe::egui::{self, Color32, RichText};
use std::collections::HashMap;

/// Most entries any list shows (keeps the windows fast on big captures).
const LIST_LIMIT: usize = 300;

impl GridApp {
    /// Toolbar entry: the "Insights" menu that opens this module's windows.
    pub fn insights_menu(&mut self, ui: &mut egui::Ui) {
        let s = &mut self.ext.insights;
        ui.menu_button("Insights \u{25be}", |ui| {
            let items: [(&mut bool, &str, &str); 7] = [
                (&mut s.show_overview, "Overview...", "Statistics of the visible (or selected) sessions"),
                (&mut s.show_waterfall, "Waterfall && slowest hosts...", "Per-page timeline with slow requests highlighted"),
                (&mut s.show_errors, "Error dashboard...", "4xx / 5xx, failed connections, TLS errors, timeouts"),
                (&mut s.show_requests, "Duplicates && caching...", "Repeated requests and cache headers explained"),
                (&mut s.show_sizes, "Sizes && compression...", "Uncompressed text, largest payloads, images"),
                (&mut s.show_hygiene, "Response-header notes...", "Security and caching headers missing on your sites"),
                (&mut s.show_cookies, "Cookies && third parties...", "Cookies per domain and requests to other sites"),
            ];
            if ui.selectable_label(self.ext.show_protocols, "Protocols (HTTP/2, HTTP/3)...").on_hover_text("Client and server HTTP versions per host, fallbacks, HTTP/3 offers").clicked() {
                self.ext.show_protocols = !self.ext.show_protocols;
                ui.close();
            }
            for (flag, label, hint) in items {
                if ui.selectable_label(*flag, label.replace("&&", "&")).on_hover_text(hint).clicked() {
                    *flag = !*flag;
                    ui.close();
                }
            }
        });
    }

    /// Windows this module draws each frame.
    pub fn insights_windows(&mut self, ctx: &egui::Context) {
        if !self.ext.insights.any_open() {
            return;
        }
        // The same rows the grid shows: text / structured filters, the advanced filter rule and the group.
        let mut visible = self.model.visible();
        if let Some(ref rule) = self.filter_rule {
            visible.retain(|&i| rule.matches(&self.model.rows[i]));
        }
        if let Some(ref g) = self.groups_manager.selected_group {
            let by = self.groups_manager.group_by.unwrap_or(GroupBy::Host);
            visible.retain(|&i| by.key_for(&self.model.rows[i]) == *g);
        }
        let rows: Vec<&SessionRecord> = visible.iter().map(|&i| &self.model.rows[i]).collect();
        let selected = self.model.selected;
        let selected_row = selected.and_then(|id| self.model.rows.iter().find(|r| r.id == id));
        if let Some(id) = draw_windows(ctx, &mut self.ext.insights, &rows, selected_row) {
            self.model.selected = Some(id);
        }
    }
}

thread_local! {
    /// Fingerprint of the rows the windows show this frame (set by `draw_windows`).
    static ROWS_FP: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Cheap fingerprint of the shown rows: changes when sessions are added, removed or updated.
fn rows_fingerprint(rows: &[&SessionRecord]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ rows.len() as u64;
    for r in rows {
        for v in [r.id as u64, r.status as u64, r.response_body.len() as u64, r.duration_ms as u64, r.response_headers.len() as u64, r.request_headers.len() as u64] {
            h = (h ^ v).wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

/// The statistics are slow to compute over many sessions: remember each result until the rows
/// (or `key`, the other inputs) change.
fn memo<T: Send + Sync + 'static>(ui: &egui::Ui, key: impl std::hash::Hash, f: impl FnOnce() -> T) -> std::sync::Arc<T> {
    use std::hash::Hasher;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut h);
    let id = egui::Id::new(("insights_memo", std::any::type_name::<T>(), h.finish()));
    let fp = ROWS_FP.with(|c| c.get());
    if let Some((k, v)) = ui.ctx().data(|d| d.get_temp::<(u64, std::sync::Arc<T>)>(id)) {
        if k == fp {
            return v;
        }
    }
    let v = std::sync::Arc::new(f());
    ui.ctx().data_mut(|d| d.insert_temp(id, (fp, v.clone())));
    v
}

fn short(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}...", s.chars().take(n).collect::<String>())
    }
}

/// A clickable session entry; clicking it asks the grid to select that session.
fn session_link(ui: &mut egui::Ui, id: usize, text: &str, selected: Option<usize>, picked: &mut Option<usize>) {
    if ui
        .selectable_label(selected == Some(id), format!("#{id}  {}", short(text, 110)))
        .on_hover_text("Click to select this session in the grid")
        .clicked()
    {
        *picked = Some(id);
    }
}

fn row_text(r: &SessionRecord) -> String {
    format!("{} {} {}", r.status, r.method, r.url)
}

/// Lists sessions by id as clickable entries.
fn id_list(ui: &mut egui::Ui, ids: &[usize], by_id: &HashMap<usize, &SessionRecord>, selected: Option<usize>, picked: &mut Option<usize>) {
    for &id in ids.iter().take(LIST_LIMIT) {
        let text = by_id.get(&id).map(|r| row_text(r)).unwrap_or_default();
        session_link(ui, id, &text, selected, picked);
    }
    if ids.len() > LIST_LIMIT {
        ui.label(RichText::new(format!("... and {} more", ids.len() - LIST_LIMIT)).italics());
    }
}

/// Label + proportional bar per entry (a small histogram).
fn count_bars(ui: &mut egui::Ui, salt: &str, list: &[(String, usize)], total: usize) {
    egui::Grid::new(salt).num_columns(2).show(ui, |ui| {
        for (k, n) in list.iter().take(50) {
            ui.label(short(k, 50));
            let frac = if total == 0 { 0.0 } else { *n as f32 / total as f32 };
            ui.add(egui::ProgressBar::new(frac).desired_width(160.0).text(format!("{n}  ({:.0}%)", frac * 100.0)));
            ui.end_row();
        }
    });
    if list.len() > 50 {
        ui.label(RichText::new(format!("... and {} more", list.len() - 50)).italics());
    }
}

/// Draws every open insights window over `rows` (the sessions the grid shows).
/// Returns the id of a session the user clicked, to be selected in the grid.
pub fn draw_windows(ctx: &egui::Context, st: &mut State, rows: &[&SessionRecord], selected_row: Option<&SessionRecord>) -> Option<usize> {
    let mut picked = None;
    let selected = selected_row.map(|r| r.id);
    let by_id: HashMap<usize, &SessionRecord> = rows.iter().map(|r| (r.id, *r)).collect();
    ROWS_FP.with(|c| c.set(rows_fingerprint(rows)));

    if st.show_overview {
        let mut open = true;
        egui::Window::new("Overview").open(&mut open).default_width(460.0).show(ctx, |ui| {
            overview_ui(ui, st, rows, selected_row, &mut picked);
        });
        st.show_overview &= open;
    }
    if st.show_waterfall {
        let mut open = true;
        egui::Window::new("Waterfall").open(&mut open).default_width(720.0).default_height(480.0).show(ctx, |ui| {
            waterfall_ui(ui, st, rows, selected, &mut picked);
        });
        st.show_waterfall &= open;
    }
    if st.show_errors {
        let mut open = true;
        egui::Window::new("Error dashboard").open(&mut open).default_width(560.0).show(ctx, |ui| {
            errors_ui(ui, rows, &by_id, selected, &mut picked);
        });
        st.show_errors &= open;
    }
    if st.show_requests {
        let mut open = true;
        egui::Window::new("Duplicates & caching").open(&mut open).default_width(620.0).show(ctx, |ui| {
            requests_ui(ui, rows, &by_id, selected, &mut picked);
        });
        st.show_requests &= open;
    }
    if st.show_sizes {
        let mut open = true;
        egui::Window::new("Sizes & compression").open(&mut open).default_width(620.0).show(ctx, |ui| {
            sizes_ui(ui, st, rows, selected, &mut picked);
        });
        st.show_sizes &= open;
    }
    if st.show_hygiene {
        let mut open = true;
        egui::Window::new("Response-header notes").open(&mut open).default_width(600.0).show(ctx, |ui| {
            hygiene_ui(ui, rows, &by_id, selected, &mut picked);
        });
        st.show_hygiene &= open;
    }
    if st.show_cookies {
        let mut open = true;
        egui::Window::new("Cookies & third parties").open(&mut open).default_width(680.0).show(ctx, |ui| {
            cookies_ui(ui, st, rows, selected, &mut picked);
        });
        st.show_cookies &= open;
    }
    picked
}

fn overview_ui(ui: &mut egui::Ui, st: &mut State, rows: &[&SessionRecord], selected_row: Option<&SessionRecord>, picked: &mut Option<usize>) {
    ui.horizontal(|ui| {
        ui.radio_value(&mut st.overview_selected_only, false, format!("Visible sessions ({})", rows.len()));
        ui.radio_value(&mut st.overview_selected_only, true, "Selected session");
    });
    let scope: Vec<&SessionRecord> = if st.overview_selected_only { selected_row.into_iter().collect() } else { rows.to_vec() };
    if scope.is_empty() {
        ui.label(RichText::new(if st.overview_selected_only { "No session selected." } else { "No sessions to summarise." }).italics());
        return;
    }
    let scope_key = ("overview", st.overview_selected_only, selected_row.map(|r| r.id));
    let o = memo(ui, scope_key, || overview(&scope));
    let pr = st.prefs.clone();
    ui.collapsing("Preferences (units, timing phases)", |ui| {
        ui.horizontal(|ui| {
            ui.label("Time:");
            ui.radio_value(&mut st.prefs.time, crate::insights::TimeUnit::Ms, "ms");
            ui.radio_value(&mut st.prefs.time, crate::insights::TimeUnit::Seconds, "seconds");
            ui.separator();
            ui.label("Size:");
            ui.radio_value(&mut st.prefs.size, crate::insights::SizeUnit::Auto, "auto");
            ui.radio_value(&mut st.prefs.size, crate::insights::SizeUnit::Bytes, "B");
            ui.radio_value(&mut st.prefs.size, crate::insights::SizeUnit::Kb, "KB");
            ui.radio_value(&mut st.prefs.size, crate::insights::SizeUnit::Mb, "MB");
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("Phases:");
            for (i, name) in crate::insights::PHASES.iter().enumerate() {
                ui.checkbox(&mut st.prefs.phases[i], *name);
            }
        });
    });
    ui.horizontal(|ui| {
        if ui.button("Copy").on_hover_text("Copy these statistics as text").clicked() {
            ui.ctx().copy_text(overview_text(&o));
        }
        if ui.button("Waterfall...").clicked() {
            st.show_waterfall = true;
        }
    });
    ui.separator();
    egui::ScrollArea::vertical().id_salt("ins_overview").show(ui, |ui| {
        egui::Grid::new("ins_ov_totals").num_columns(2).striped(true).show(ui, |ui| {
            ui.label("Sessions");
            ui.label(o.sessions.to_string());
            ui.end_row();
            ui.label("Request bytes (bodies)");
            ui.label(crate::insights::fmt_size(o.request_bytes, pr.size));
            ui.end_row();
            ui.label("Response bytes (transferred)");
            ui.label(crate::insights::fmt_size(o.response_bytes, pr.size));
            ui.end_row();
            ui.label("First request");
            ui.label(o.first_start.clone().unwrap_or_else(|| "-".into()));
            ui.end_row();
            ui.label("Time span");
            ui.label(o.span_ms.map(|s| crate::insights::fmt_time(s as f64, pr.time)).unwrap_or_else(|| "-".into()));
            ui.end_row();
            ui.label("Average duration");
            ui.label(crate::insights::fmt_time(o.avg_ms, pr.time));
            ui.end_row();
            ui.label("Max duration");
            ui.horizontal(|ui| {
                ui.label(crate::insights::fmt_time(o.max_ms as f64, pr.time));
                if let Some(id) = o.max_id {
                    if ui.link(format!("#{id}")).on_hover_text("Select in grid").clicked() {
                        *picked = Some(id);
                    }
                }
            });
            ui.end_row();
        });
        egui::CollapsingHeader::new("Average timing phases").id_salt("ins_ov_phases").default_open(true).show(ui, |ui| {
            egui::Grid::new("ins_ov_phase_grid").num_columns(3).striped(true).show(ui, |ui| {
                for (i, &(name, avg, n)) in memo(ui, ("phases", scope_key), || crate::insights::phase_averages(&scope)).iter().enumerate() {
                    if !pr.phases[i] {
                        continue;
                    }
                    ui.label(name);
                    ui.label(if n > 0 { crate::insights::fmt_time(avg, pr.time) } else { "-".into() });
                    ui.label(RichText::new(format!("{n} measured")).small());
                    ui.end_row();
                }
            });
        });
        let n = o.sessions;
        for (title, list, open) in [
            ("By status", &o.by_status, true),
            ("By content type", &o.by_content_type, true),
            ("By host", &o.by_host, false),
            ("By process", &o.by_process, false),
            ("Duration histogram", &o.duration_histogram, false),
            ("Response size histogram", &o.size_histogram, false),
        ] {
            egui::CollapsingHeader::new(title).id_salt(("ins_ov", title)).default_open(open).show(ui, |ui| {
                count_bars(ui, &format!("ins_ov_bars_{title}"), list, n);
            });
        }
    });
}

fn waterfall_ui(ui: &mut egui::Ui, st: &mut State, rows: &[&SessionRecord], selected: Option<usize>, picked: &mut Option<usize>) {
    let pg = memo(ui, ("pages", st.slow_ms as u128), || pages(rows, st.slow_ms as u128));
    ui.horizontal(|ui| {
        ui.label("Slow at");
        ui.add(egui::DragValue::new(&mut st.slow_ms).range(1..=600_000).suffix(" ms"));
        ui.separator();
        ui.label("Page");
        if pg.is_empty() {
            ui.label("-");
        } else {
            st.page = st.page.min(pg.len() - 1);
            egui::ComboBox::from_id_salt("ins_wf_page").width(380.0).selected_text(short(&pg[st.page].url, 60)).show_ui(ui, |ui| {
                for (i, p) in pg.iter().enumerate() {
                    ui.selectable_value(&mut st.page, i, format!("{} ({} requests)", short(&p.url, 70), p.bars.len()));
                }
            });
        }
    });
    if pg.is_empty() {
        ui.label(RichText::new("No sessions to show.").italics());
        return;
    }
    let p = &pg[st.page];
    let slow = p.bars.iter().filter(|b| b.slow).count();
    ui.label(format!("{} requests, {} ms from first start to last end, {} slow (red); errors in orange.", p.bars.len(), p.total_ms, slow));
    ui.separator();
    let total = p.total_ms.max(1) as f32;
    egui::ScrollArea::vertical().id_salt("ins_wf_bars").max_height(ui.available_height() * 0.62).show(ui, |ui| {
        for b in p.bars.iter().take(LIST_LIMIT) {
            ui.horizontal(|ui| {
                let label = RichText::new(format!("#{} {}", b.id, short(&b.label, 40)));
                let label = if b.slow { label.color(Color32::from_rgb(220, 60, 60)) } else { label };
                let resp = ui.add_sized([280.0, 16.0], egui::Button::selectable(selected == Some(b.id), label));
                if resp.on_hover_text(b.label.as_str()).clicked() {
                    *picked = Some(b.id);
                }
                let w = (ui.available_width() - 70.0).max(60.0);
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 16.0), egui::Sense::click());
                let x0 = rect.left() + w * (b.offset_ms as f32 / total);
                let bw = (w * (b.duration_ms as f32 / total)).max(2.0);
                let color = if b.slow {
                    Color32::from_rgb(220, 60, 60)
                } else if b.status == 0 || b.status >= 400 {
                    Color32::from_rgb(230, 150, 40)
                } else {
                    Color32::from_rgb(70, 130, 200)
                };
                ui.painter().rect_filled(rect, 0.0, ui.visuals().faint_bg_color);
                let bar = egui::Rect::from_min_size(egui::pos2(x0.min(rect.right() - 2.0), rect.top() + 3.0), egui::vec2(bw.min(rect.right() - x0).max(2.0), 10.0));
                ui.painter().rect_filled(bar, 2.0, color);
                if selected == Some(b.id) {
                    ui.painter().rect_stroke(bar.expand(1.0), 2.0, egui::Stroke::new(1.5, ui.visuals().strong_text_color()), egui::StrokeKind::Outside);
                }
                if resp.on_hover_text(format!("{}\nstarts at +{} ms, takes {} ms, status {}", b.label, b.offset_ms, b.duration_ms, b.status)).clicked() {
                    *picked = Some(b.id);
                }
                ui.label(format!("{} ms", b.duration_ms));
            });
        }
        if p.bars.len() > LIST_LIMIT {
            ui.label(RichText::new(format!("... and {} more", p.bars.len() - LIST_LIMIT)).italics());
        }
    });
    ui.separator();
    ui.label(RichText::new("Slowest hosts (all visible sessions, by average duration)").strong());
    egui::ScrollArea::vertical().id_salt("ins_wf_hosts").show(ui, |ui| {
        egui::Grid::new("ins_wf_hosts_grid").striped(true).show(ui, |ui| {
            for h in ["Host", "Requests", "Average", "Max", "Total", "Slowest"] {
                ui.label(RichText::new(h).strong());
            }
            ui.end_row();
            for h in memo(ui, "slowest_hosts", || slowest_hosts(rows)).iter().take(50) {
                ui.label(h.host.as_str());
                ui.label(h.count.to_string());
                ui.label(format!("{:.0} ms", h.avg_ms));
                ui.label(format!("{} ms", h.max_ms));
                ui.label(format!("{} ms", h.total_ms));
                if ui.selectable_label(selected == Some(h.slowest_id), format!("#{}", h.slowest_id)).clicked() {
                    *picked = Some(h.slowest_id);
                }
                ui.end_row();
            }
        });
    });
}

fn errors_ui(ui: &mut egui::Ui, rows: &[&SessionRecord], by_id: &HashMap<usize, &SessionRecord>, selected: Option<usize>, picked: &mut Option<usize>) {
    let e = memo(ui, "errors", || error_report(rows));
    ui.horizontal_wrapped(|ui| {
        for (label, n) in [
            ("4xx", e.client_errors.len()),
            ("5xx", e.server_errors.len()),
            ("Failed connections", e.failed_connections.len()),
            ("TLS errors", e.tls_errors.len()),
            ("Timeouts", e.timeouts.len()),
            ("Aborted", e.aborted.len()),
        ] {
            let t = RichText::new(format!("{label}: {n}")).strong();
            ui.label(if n > 0 { t.color(Color32::from_rgb(220, 90, 60)) } else { t });
            ui.separator();
        }
    });
    if !e.by_status.is_empty() {
        ui.label(format!("By code: {}", e.by_status.iter().map(|(c, n)| format!("{c} x{n}")).collect::<Vec<_>>().join(", ")));
    }
    ui.label(
        RichText::new("Failures are recognised from status 0 (no response), 502 / 504 / 408 responses and the session's comment, body or x-ferrotap headers (e.g. \"timed out\", \"certificate\", \"Breakpoint aborted\").")
            .small()
            .weak(),
    );
    ui.separator();
    egui::ScrollArea::vertical().id_salt("ins_err").show(ui, |ui| {
        for (title, ids) in [
            ("4xx client errors", &e.client_errors),
            ("5xx server errors", &e.server_errors),
            ("Failed connections", &e.failed_connections),
            ("TLS errors", &e.tls_errors),
            ("Timeouts", &e.timeouts),
            ("Aborted at breakpoint", &e.aborted),
        ] {
            egui::CollapsingHeader::new(format!("{title} ({})", ids.len())).id_salt(("ins_err", title)).default_open(!ids.is_empty() && ids.len() <= 20).show(ui, |ui| {
                if ids.is_empty() {
                    ui.label(RichText::new("None").italics());
                }
                id_list(ui, ids, by_id, selected, picked);
            });
        }
    });
}

fn requests_ui(ui: &mut egui::Ui, rows: &[&SessionRecord], by_id: &HashMap<usize, &SessionRecord>, selected: Option<usize>, picked: &mut Option<usize>) {
    let dups = memo(ui, "duplicates", || duplicates(rows));
    let cache = memo(ui, "cache", || cache_analysis(rows));
    let missing = cache.iter().filter(|c| c.missing).count();
    egui::ScrollArea::vertical().id_salt("ins_req").show(ui, |ui| {
        egui::CollapsingHeader::new(format!("Duplicate requests ({} groups)", dups.len())).id_salt("ins_dups").default_open(true).show(ui, |ui| {
            ui.label(RichText::new("Same method, URL and request body sent more than once.").small().weak());
            if dups.is_empty() {
                ui.label(RichText::new("None found").italics());
            }
            for g in dups.iter().take(LIST_LIMIT) {
                egui::CollapsingHeader::new(format!("{}x {} {}  (body {} B, repeats transferred {})", g.ids.len(), g.method, short(&g.url, 80), g.body_len, fmt_bytes(g.repeat_bytes)))
                    .id_salt(("ins_dup", g.ids[0]))
                    .show(ui, |ui| id_list(ui, &g.ids, by_id, selected, picked));
            }
        });
        egui::CollapsingHeader::new(format!("Cache analysis ({} GET 200 responses, {} without caching headers)", cache.len(), missing)).id_salt("ins_cache").default_open(true).show(ui, |ui| {
            ui.label(RichText::new("Responses without Cache-Control, Expires, ETag or Last-Modified are listed first. Expand an entry for its cache headers in plain words.").small().weak());
            for c in cache.iter().take(LIST_LIMIT) {
                ui.horizontal(|ui| {
                    if c.missing {
                        ui.label(RichText::new("no cache headers").color(Color32::from_rgb(230, 150, 40)));
                    } else {
                        ui.label(RichText::new("has cache headers").weak());
                    }
                    session_link(ui, c.id, &c.url, selected, picked);
                });
                egui::CollapsingHeader::new("explain").id_salt(("ins_cache_x", c.id)).show(ui, |ui| {
                    for n in &c.notes {
                        ui.label(n.as_str());
                    }
                });
            }
            if cache.len() > LIST_LIMIT {
                ui.label(RichText::new(format!("... and {} more", cache.len() - LIST_LIMIT)).italics());
            }
        });
    });
}

fn size_grid(ui: &mut egui::Ui, salt: &str, items: &[SizeItem], selected: Option<usize>, picked: &mut Option<usize>) {
    egui::Grid::new(salt).striped(true).show(ui, |ui| {
        for it in items.iter().take(LIST_LIMIT) {
            ui.label(fmt_bytes(it.bytes as u64));
            ui.label(short(&it.content_type, 30));
            session_link(ui, it.id, &it.url, selected, picked);
            ui.end_row();
        }
    });
}

fn sizes_ui(ui: &mut egui::Ui, st: &mut State, rows: &[&SessionRecord], selected: Option<usize>, picked: &mut Option<usize>) {
    let comp = memo(ui, "compression", || compression_issues(rows, 1024));
    let imgs = memo(ui, "images", || image_report(rows));
    egui::ScrollArea::vertical().id_salt("ins_sizes").show(ui, |ui| {
        egui::CollapsingHeader::new(format!("Compression check ({} text responses over 1 KB sent uncompressed)", comp.len())).id_salt("ins_comp").default_open(true).show(ui, |ui| {
            ui.label(RichText::new("Text (HTML, CSS, JS, JSON, XML, SVG...) usually shrinks a lot with gzip / br / zstd; these had no Content-Encoding.").small().weak());
            if comp.is_empty() {
                ui.label(RichText::new("None found").italics());
            }
            size_grid(ui, "ins_comp_grid", &comp, selected, picked);
        });
        egui::CollapsingHeader::new("Largest payloads").id_salt("ins_large").default_open(true).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("Show top");
                ui.add(egui::DragValue::new(&mut st.top_n).range(1..=200));
            });
            let top_n = st.top_n;
            let large = memo(ui, ("largest", top_n), || largest_payloads(rows, top_n));
            size_grid(ui, "ins_large_grid", &large, selected, picked);
        });
        egui::CollapsingHeader::new(format!("Images ({} images, {} total, {} over {} KB)", imgs.count, fmt_bytes(imgs.total_bytes), imgs.large, LARGE_IMAGE_BYTES / 1024))
            .id_salt("ins_imgs")
            .default_open(true)
            .show(ui, |ui| {
                if !imgs.by_type.is_empty() {
                    ui.label(format!("By type: {}", imgs.by_type.iter().map(|(t, b)| format!("{t} {}", fmt_bytes(*b))).collect::<Vec<_>>().join(", ")));
                }
                size_grid(ui, "ins_imgs_grid", &imgs.items, selected, picked);
            });
    });
}

fn hygiene_ui(ui: &mut egui::Ui, rows: &[&SessionRecord], _by_id: &HashMap<usize, &SessionRecord>, selected: Option<usize>, picked: &mut Option<usize>) {
    let hosts = memo(ui, "hygiene", || header_hygiene(rows));
    ui.label(
        RichText::new("Informational notes for checking your own sites: response headers that are commonly recommended but were not seen. Not every site needs every header.")
            .small()
            .weak(),
    );
    ui.separator();
    if hosts.is_empty() {
        ui.label(RichText::new("Nothing to note for the visible sessions.").italics());
    }
    egui::ScrollArea::vertical().id_salt("ins_hyg").show(ui, |ui| {
        for h in hosts.iter().take(LIST_LIMIT) {
            egui::CollapsingHeader::new(format!("{}  ({} notes, {} responses)", h.host, h.notes.len(), h.responses)).id_salt(("ins_hyg", &h.host)).show(ui, |ui| {
                for n in &h.notes {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(n.header).strong());
                        ui.label(format!("missing in {} of {} responses", n.missing_in, n.applicable));
                        if ui.selectable_label(selected == Some(n.example_id), format!("e.g. #{}", n.example_id)).clicked() {
                            *picked = Some(n.example_id);
                        }
                    });
                    ui.label(RichText::new(n.explanation).weak());
                    ui.add_space(4.0);
                }
            });
        }
    });
}

fn cookies_ui(ui: &mut egui::Ui, st: &mut State, rows: &[&SessionRecord], selected: Option<usize>, picked: &mut Option<usize>) {
    let domains = memo(ui, "cookies", || cookie_overview(rows));
    let detected = (*memo(ui, "main_site", || main_site(rows))).clone().unwrap_or_default();
    egui::ScrollArea::vertical().id_salt("ins_cookies").show(ui, |ui| {
        egui::CollapsingHeader::new(format!("Cookies per domain ({})", domains.len())).id_salt("ins_ck").default_open(true).show(ui, |ui| {
            if domains.is_empty() {
                ui.label(RichText::new("No cookies set or sent.").italics());
            }
            for d in domains.iter().take(LIST_LIMIT) {
                egui::CollapsingHeader::new(format!("{}  - {} set ({} B), {} sent (largest Cookie header {} B)", d.domain, d.set.len(), d.set_bytes, d.sent_names.len(), d.max_sent_header))
                    .id_salt(("ins_ck_d", &d.domain))
                    .show(ui, |ui| {
                        if !d.set.is_empty() {
                            egui::Grid::new(("ins_ck_g", &d.domain)).striped(true).show(ui, |ui| {
                                for h in ["Name", "Size", "Secure", "HttpOnly", "SameSite", "Expiry", "Path", "Set by"] {
                                    ui.label(RichText::new(h).strong());
                                }
                                ui.end_row();
                                for c in &d.set {
                                    ui.label(short(&c.name, 30));
                                    ui.label(format!("{} B", c.size));
                                    ui.label(if c.secure { "yes" } else { "no" });
                                    ui.label(if c.http_only { "yes" } else { "no" });
                                    ui.label(c.same_site.clone().unwrap_or_else(|| "(not set)".into()));
                                    ui.label(c.expiry.clone().unwrap_or_else(|| "session".into()));
                                    ui.label(short(&c.path, 20));
                                    if ui.selectable_label(selected == Some(c.session_id), format!("#{}", c.session_id)).clicked() {
                                        *picked = Some(c.session_id);
                                    }
                                    ui.end_row();
                                }
                            });
                        }
                        if !d.sent_names.is_empty() {
                            ui.label(format!("Sent by the client: {}", short(&d.sent_names.join(", "), 400)));
                        }
                    });
            }
        });
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Main site");
            ui.add(egui::TextEdit::singleline(&mut st.main_site).hint_text(if detected.is_empty() { "example.com".to_string() } else { detected.clone() }).desired_width(200.0));
            ui.label(RichText::new("(empty = detected from the first HTML page)").small().weak());
        });
        let main = if st.main_site.trim().is_empty() { detected.clone() } else { st.main_site.trim().to_string() };
        let tp = memo(ui, ("third_party", main.clone()), || third_party(rows, &main));
        egui::CollapsingHeader::new(format!("Third-party requests ({} hosts outside {})", tp.len(), if main.is_empty() { "-" } else { &main })).id_salt("ins_tp").default_open(true).show(ui, |ui| {
            egui::Grid::new("ins_tp_grid").striped(true).show(ui, |ui| {
                for h in ["Host", "Site", "Requests", "Bytes", "First"] {
                    ui.label(RichText::new(h).strong());
                }
                ui.end_row();
                for t in tp.iter().take(LIST_LIMIT) {
                    ui.label(t.host.as_str());
                    ui.label(t.site.as_str());
                    ui.label(t.count.to_string());
                    ui.label(fmt_bytes(t.bytes));
                    if ui.selectable_label(selected == Some(t.first_id), format!("#{}", t.first_id)).clicked() {
                        *picked = Some(t.first_id);
                    }
                    ui.end_row();
                }
            });
        });
    });
}
