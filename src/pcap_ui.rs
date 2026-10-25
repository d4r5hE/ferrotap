//! G19 Packets window: open / save packet files, display filter, packet list with details and hex,
//! conversations (with related HTTP sessions), endpoints, protocol hierarchy, IO graph, DNS log,
//! follow TCP stream.
use eframe::egui::{self, Color32, RichText};

use crate::pcap::{self, Frame, Packet};
use crate::pfilter;

#[derive(Default)]
pub struct PacketsUi {
    pub show: bool,
    pub path: String,
    pub frames: Vec<Frame>,
    pub packets: Vec<Packet>,
    /// Capture start (seconds since the epoch) for matching HTTP sessions.
    pub t0: f64,
    pub filter: String,
    filter_for: Option<String>,
    pub filter_err: Option<String>,
    /// Indices into `packets` that pass the filter.
    pub visible: Vec<usize>,
    pub sel: Option<usize>,
    pub tab: u8,
    pub interval: f64,
    pub bytes_mode: bool,
    pub save_path: String,
    pub msg: String,
    pub follow: Option<(usize, usize, Vec<(bool, Vec<u8>)>)>,
    pub keylog_path: String,
    /// Decrypted application data of the followed stream (or the error).
    pub decrypted: Option<Result<crate::tlsdecrypt::Decrypted, String>>,
    /// Statistics of the visible packets, computed when a tab needs them and kept until the
    /// packets, the filter or the sessions change (recomputing per frame is slow for big files).
    stats_key: Option<(usize, usize, Option<String>, usize)>,
    conv_cache: Option<Vec<(pcap::Conversation, Vec<usize>)>>,
    ep_cache: Option<Vec<(String, usize, usize)>>,
    hier_cache: Option<Vec<(String, usize, usize)>>,
}

impl PacketsUi {
    pub fn load(&mut self, p: &std::path::Path) -> Result<usize, String> {
        let frames = pcap::read_file(p)?;
        self.t0 = frames.iter().map(|f| f.ts).find(|t| *t > 0.0).unwrap_or(0.0);
        self.packets = pcap::decode_all(&frames);
        self.frames = frames;
        self.sel = None;
        self.follow = None;
        self.filter_for = None;
        self.stats_key = None;
        self.refilter();
        Ok(self.packets.len())
    }

    /// Re-apply the filter when it changed.
    pub fn refilter(&mut self) {
        if self.filter_for.as_deref() == Some(self.filter.as_str()) {
            return;
        }
        self.filter_for = Some(self.filter.clone());
        match pfilter::parse(&self.filter) {
            Ok(e) => {
                self.filter_err = None;
                self.visible = (0..self.packets.len()).filter(|&i| e.as_ref().is_none_or(|e| pfilter::eval(e, &self.packets[i]))).collect();
            }
            Err(e) => self.filter_err = Some(e),
        }
    }

}

const TABS: [&str; 7] = ["Packets", "Conversations", "Endpoints", "Protocols", "IO graph", "DNS", "Follow stream"];

impl crate::ui::GridApp {
    pub fn packets_menu(&mut self, ui: &mut egui::Ui) {
        if ui.selectable_label(self.ext.packets.show, "Packets").on_hover_text("Open pcap / pcapng files: filter, conversations, follow streams").clicked() {
            self.ext.packets.show = !self.ext.packets.show;
        }
    }

    pub fn packets_window(&mut self, ctx: &egui::Context) {
        if !self.ext.packets.show {
            return;
        }
        let mut open = true;
        let (mut do_open, mut do_save, mut pick, mut select_sessions) = (false, false, false, None::<Vec<usize>>);
        let rows = &self.model.rows;
        let st = &mut self.ext.packets;
        if st.interval <= 0.0 {
            st.interval = 1.0;
        }
        egui::Window::new("Packets").open(&mut open).default_width(900.0).default_height(620.0).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("File");
                ui.add(egui::TextEdit::singleline(&mut st.path).hint_text("capture.pcapng").desired_width(360.0));
                if ui.small_button("Browse...").clicked() {
                    pick = true;
                }
                if ui.button("Open").clicked() {
                    do_open = true;
                }
                ui.label(RichText::new(&st.msg).color(Color32::GRAY));
            });
            ui.horizontal(|ui| {
                ui.label("Filter");
                let r = ui.add(egui::TextEdit::singleline(&mut st.filter).hint_text("tcp.port == 443 and not dns").desired_width(420.0));
                let color = if st.filter_err.is_some() { Color32::from_rgb(230, 90, 90) } else { Color32::LIGHT_GREEN };
                ui.label(RichText::new(match &st.filter_err {
                    Some(e) => e.clone(),
                    None => format!("{} of {} packets", st.visible.len(), st.packets.len()),
                }).color(color));
                r.on_hover_text(pfilter::HELP);
            });
            let comps = pfilter::complete(&st.filter);
            if !comps.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    for c in comps {
                        if ui.small_button(c).clicked() {
                            let cut = st.filter.rfind(|ch: char| ch.is_whitespace() || "()!&|".contains(ch)).map_or(0, |i| i + 1);
                            st.filter.truncate(cut);
                            st.filter.push_str(c);
                        }
                    }
                });
            }
            st.refilter();
            ui.horizontal(|ui| {
                for (i, t) in TABS.iter().enumerate() {
                    ui.selectable_value(&mut st.tab, i as u8, *t);
                }
                ui.separator();
                ui.add(egui::TextEdit::singleline(&mut st.save_path).hint_text("filtered.pcap").desired_width(160.0));
                if ui.add_enabled(!st.visible.is_empty(), egui::Button::new("Save filtered")).clicked() {
                    do_save = true;
                }
            });
            ui.separator();
            if st.packets.is_empty() {
                ui.label(RichText::new("Open a .pcap or .pcapng file (for example one saved by Wireshark or tcpdump).").italics().color(Color32::GRAY));
                return;
            }
            let vis: Vec<&Packet> = st.visible.iter().map(|&i| &st.packets[i]).collect();
            let key = (st.packets.len(), st.visible.len(), st.filter_for.clone(), rows.len());
            if st.stats_key.as_ref() != Some(&key) {
                st.stats_key = Some(key);
                (st.conv_cache, st.ep_cache, st.hier_cache) = (None, None, None);
            }
            let t0 = st.t0;
            match st.tab {
                0 => {
                    let h = ui.available_height();
                    let mut clicked = None;
                    egui::ScrollArea::vertical().id_salt("pk_list").max_height((h * 0.55).max(120.0)).auto_shrink([false, false]).show_rows(ui, 18.0, vis.len(), |ui, range| {
                        for &p in &vis[range] {
                            let text = format!(
                                "{:>6}  {:>9.4}  {:<22} {:<22} {:<6} {:>6}  {}",
                                p.no,
                                p.time,
                                p.src.map(|a| a.to_string()).unwrap_or_default(),
                                p.dst.map(|a| a.to_string()).unwrap_or_default(),
                                p.protocol,
                                p.len,
                                p.info
                            );
                            if ui.selectable_label(st.sel == Some(p.no - 1), RichText::new(text).monospace()).clicked() {
                                clicked = Some(p.no - 1);
                            }
                        }
                    });
                    if clicked.is_some() {
                        st.sel = clicked;
                    }
                    ui.separator();
                    if let Some(p) = st.sel.and_then(|i| st.packets.get(i)) {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("Frame {}: {} bytes, layers {}", p.no, p.len, p.layers.join(" / "))).strong());
                            if p.transport == "TCP" && ui.small_button("Follow TCP stream").clicked() {
                                st.follow = Some(pcap::follow_tcp(&st.frames, &st.packets, p));
                                st.decrypted = None;
                                st.tab = 6;
                            }
                        });
                        if p.transport == "TCP" || p.transport == "UDP" {
                            ui.label(format!("{} -> {}", pcap::endpoint(p.src, p.sport, &p.transport), pcap::endpoint(p.dst, p.dport, &p.transport)));
                        }
                        if let Some(s) = &p.sni {
                            ui.label(format!("TLS server name: {s}"));
                        }
                        egui::ScrollArea::vertical().id_salt("pk_hex").show(ui, |ui| {
                            ui.label(RichText::new(crate::inspect::hex_dump(&st.frames[p.no - 1].data, 16 * 1024)).monospace().small());
                        });
                    }
                }
                1 => {
                    egui::ScrollArea::vertical().id_salt("pk_conv").show(ui, |ui| {
                        egui::Grid::new("pk_conv_grid").striped(true).show(ui, |ui| {
                            for h in ["A", "B", "Transport", "Packets", "Bytes", "Start (s)", "Duration (s)", "HTTP sessions"] {
                                ui.strong(h);
                            }
                            ui.end_row();
                            let convs = st.conv_cache.get_or_insert_with(|| {
                                pcap::conversations(&vis).into_iter().take(2000).map(|c| {
                                    let rel = pcap::related_sessions(&c, rows, t0);
                                    (c, rel)
                                }).collect()
                            });
                            for (c, rel) in convs.iter() {
                                ui.label(&c.a);
                                ui.label(&c.b);
                                ui.label(&c.transport);
                                ui.label(c.packets.to_string());
                                ui.label(crate::insights::fmt_bytes(c.bytes as u64));
                                ui.label(format!("{:.3}", c.start));
                                ui.label(format!("{:.3}", c.end - c.start));
                                if rel.is_empty() {
                                    ui.label("-");
                                } else if ui.small_button(format!("{} session(s)", rel.len())).on_hover_text("Select them in the grid").clicked() {
                                    select_sessions = Some(rel.clone());
                                }
                                ui.end_row();
                            }
                        });
                    });
                }
                2 => {
                    egui::ScrollArea::vertical().id_salt("pk_ep").show(ui, |ui| {
                        egui::Grid::new("pk_ep_grid").striped(true).show(ui, |ui| {
                            for h in ["Address", "Packets", "Bytes"] {
                                ui.strong(h);
                            }
                            ui.end_row();
                            let eps = st.ep_cache.get_or_insert_with(|| pcap::endpoints(&vis).into_iter().take(2000).collect());
                            for (a, n, b) in eps.iter() {
                                ui.label(a);
                                ui.label(n.to_string());
                                ui.label(crate::insights::fmt_bytes(*b as u64));
                                ui.end_row();
                            }
                        });
                    });
                }
                3 => {
                    let total = vis.len().max(1);
                    egui::Grid::new("pk_hier").striped(true).show(ui, |ui| {
                        for h in ["Protocol", "Packets", "% packets", "Bytes"] {
                            ui.strong(h);
                        }
                        ui.end_row();
                        let hier = st.hier_cache.get_or_insert_with(|| pcap::hierarchy(&vis));
                        for (path, n, b) in hier.iter() {
                            let depth = path.matches('/').count();
                            ui.label(format!("{}{}", "  ".repeat(depth), path.rsplit('/').next().unwrap_or(path)));
                            ui.label(n.to_string());
                            ui.label(format!("{:.1}", *n as f64 * 100.0 / total as f64));
                            ui.label(crate::insights::fmt_bytes(*b as u64));
                            ui.end_row();
                        }
                    });
                }
                4 => {
                    ui.horizontal(|ui| {
                        ui.label("Interval");
                        for (v, l) in [(0.1, "100 ms"), (1.0, "1 s"), (10.0, "10 s"), (60.0, "1 min")] {
                            ui.radio_value(&mut st.interval, v, l);
                        }
                        ui.separator();
                        ui.radio_value(&mut st.bytes_mode, false, "packets");
                        ui.radio_value(&mut st.bytes_mode, true, "bytes");
                    });
                    let g = pcap::io_graph(&vis, st.interval);
                    let max = g.iter().map(|x| if st.bytes_mode { x.1 } else { x.0 }).max().unwrap_or(1).max(1);
                    ui.label(format!("{} intervals, peak {} {} per interval", g.len(), max, if st.bytes_mode { "bytes" } else { "packets" }));
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 220.0), egui::Sense::hover());
                    let painter = ui.painter_at(rect);
                    painter.rect_stroke(rect, 0.0, egui::Stroke::new(1.0, Color32::GRAY), egui::StrokeKind::Inside);
                    let n = g.len().max(1) as f32;
                    let w = rect.width() / n;
                    for (i, x) in g.iter().enumerate() {
                        let v = if st.bytes_mode { x.1 } else { x.0 } as f32 / max as f32;
                        let x0 = rect.left() + i as f32 * w;
                        let bar = egui::Rect::from_min_max(egui::pos2(x0, rect.bottom() - v * rect.height()), egui::pos2(x0 + w.max(1.0) - 0.5, rect.bottom()));
                        painter.rect_filled(bar, 0.0, Color32::from_rgb(90, 150, 230));
                    }
                }
                5 => {
                    egui::ScrollArea::vertical().id_salt("pk_dns").show(ui, |ui| {
                        egui::Grid::new("pk_dns_grid").striped(true).show(ui, |ui| {
                            for h in ["Time (s)", "", "Type", "Name", "Answers"] {
                                ui.strong(h);
                            }
                            ui.end_row();
                            for (t, resp, ty, name, ans) in pcap::dns_log(&vis) {
                                ui.label(format!("{t:.3}"));
                                ui.label(if resp { "response" } else { "query" });
                                ui.label(ty);
                                ui.label(name);
                                ui.label(ans);
                                ui.end_row();
                            }
                        });
                    });
                }
                _ => match &st.follow {
                    None => {
                        ui.label("Select a TCP packet in the Packets tab and press Follow TCP stream.");
                    }
                    Some((up, down, chunks)) => {
                        ui.horizontal(|ui| {
                            ui.label("Key log");
                            if st.keylog_path.is_empty() {
                                if let Some(p) = crate::tlsdecrypt::keylog_path() {
                                    st.keylog_path = p.display().to_string();
                                }
                            }
                            ui.add(egui::TextEdit::singleline(&mut st.keylog_path).hint_text("SSLKEYLOGFILE").desired_width(260.0));
                            if ui.button("Decrypt TLS").clicked() {
                                let c2s: Vec<u8> = chunks.iter().filter(|c| c.0).flat_map(|c| c.1.clone()).collect();
                                let s2c: Vec<u8> = chunks.iter().filter(|c| !c.0).flat_map(|c| c.1.clone()).collect();
                                st.decrypted = Some(
                                    std::fs::read_to_string(st.keylog_path.trim().trim_matches('"'))
                                        .map_err(|e| format!("key log: {e}"))
                                        .and_then(|t| crate::tlsdecrypt::decrypt(&c2s, &s2c, &crate::tlsdecrypt::parse_keylog(&t))),
                                );
                            }
                        });
                        match &st.decrypted {
                            Some(Ok(d)) => {
                                ui.colored_label(Color32::LIGHT_GREEN, format!("{} {} decrypted ({} records failed)", d.version, d.cipher, d.failed));
                                egui::ScrollArea::vertical().id_salt("pk_dec").max_height(260.0).show(ui, |ui| {
                                    ui.label(RichText::new(String::from_utf8_lossy(&d.client[..d.client.len().min(100_000)])).monospace().color(Color32::from_rgb(110, 160, 255)));
                                    ui.label(RichText::new(String::from_utf8_lossy(&d.server[..d.server.len().min(100_000)])).monospace().color(Color32::from_rgb(240, 110, 110)));
                                });
                                ui.separator();
                            }
                            Some(Err(e)) => {
                                ui.colored_label(Color32::from_rgb(230, 90, 90), e);
                            }
                            None => {}
                        }
                        ui.label(format!("client -> server {up} bytes (blue), server -> client {down} bytes (red)"));
                        egui::ScrollArea::vertical().id_salt("pk_follow").show(ui, |ui| {
                            for (from_client, data) in chunks {
                                let c = if *from_client { Color32::from_rgb(110, 160, 255) } else { Color32::from_rgb(240, 110, 110) };
                                let text: String = String::from_utf8_lossy(&data[..data.len().min(200_000)]).chars().map(|ch| if ch.is_control() && ch != '\n' && ch != '\r' && ch != '\t' { '.' } else { ch }).collect();
                                ui.label(RichText::new(text).monospace().color(c));
                            }
                        });
                    }
                },
            }
        });
        if pick {
            let start = self.ext.packets.path.clone();
            self.ext.pick.open_file("pcap_open", "Open a packet capture", &start, &["pcap", "pcapng", "cap"]);
        }
        if let Some(p) = self.ext.pick.take("pcap_open") {
            self.ext.packets.path = p.display().to_string();
            do_open = true;
        }
        let st = &mut self.ext.packets;
        if do_open {
            let p = std::path::PathBuf::from(st.path.trim().trim_matches('"'));
            st.msg = match st.load(&p) {
                Ok(n) => format!("{n} packets"),
                Err(e) => e,
            };
        }
        if do_save {
            let p = std::path::PathBuf::from(st.save_path.trim().trim_matches('"'));
            let frames: Vec<&Frame> = st.visible.iter().map(|&i| &st.frames[i]).collect();
            st.msg = match std::fs::write(&p, pcap::write_pcap(&frames)) {
                Ok(()) => format!("Saved {} packets to {}", frames.len(), p.display()),
                Err(e) => format!("Save failed: {e}"),
            };
        }
        if let Some(ids) = select_sessions {
            self.model.selected = ids.first().copied();
            self.ext.sel.set(ids);
        }
        self.ext.packets.show = open;
    }
}
