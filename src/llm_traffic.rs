//! G11 AI/LLM traffic tools: LLM API call inspector (model, messages, tools, tokens), agent call grouping, LLM response cache.
//! Owned by one builder; the app calls `llm_traffic_menu` from the toolbar and `llm_traffic_windows` every frame.
//! The analysis (parsing, prices, grouping, cache store) is in `llm_analysis.rs`.
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui::{self, Color32, RichText};

use crate::llm_analysis::*;
use crate::model::SessionRecord;
use crate::ui::GridApp;

/// Longest text shown for one message part (the rest is cut, with a note).
const TEXT_LIMIT: usize = 4000;
/// Most rows a list shows.
const LIST_LIMIT: usize = 500;

type Fingerprint = (usize, usize, u16, usize);

/// This module's UI state (kept in `GridApp::ext.llm`).
#[derive(Default)]
pub struct State {
    pub show_inspector: bool,
    pub show_calls: bool,
    pub show_agents: bool,
    pub show_cache: bool,
    pub show_prices: bool,
    /// Agent window: also list conversations of a single call.
    pub show_single_calls: bool,
    /// The editable price table (loaded on first use).
    pub prices: Option<PriceTable>,
    /// Where the price table lives (None = `PriceTable::default_path()`).
    pub prices_path: Option<PathBuf>,
    /// Parsed sessions by id, with a fingerprint of the record so changed rows are parsed again.
    parsed: HashMap<usize, (Fingerprint, Option<Arc<LlmCall>>)>,
    /// Rows already offered to the cache's auto-add.
    cache_scanned: usize,
    /// Last message shown in the cache / price windows.
    pub status: String,
}

impl State {
    pub fn any_open(&self) -> bool {
        self.show_inspector || self.show_calls || self.show_agents || self.show_cache || self.show_prices
    }

    pub fn prices_path(&self) -> PathBuf {
        self.prices_path.clone().unwrap_or_else(PriceTable::default_path)
    }

    pub fn prices(&mut self) -> &mut PriceTable {
        if self.prices.is_none() {
            self.prices = Some(PriceTable::load(&self.prices_path()));
        }
        self.prices.as_mut().unwrap()
    }

    /// Every LLM call among `rows`, in row order (parsed once per session and remembered).
    pub fn calls(&mut self, rows: &[SessionRecord]) -> Vec<Arc<LlmCall>> {
        let mut out = Vec::new();
        for r in rows {
            if !r.method.eq_ignore_ascii_case("POST") {
                continue;
            }
            let fp = (r.request_body.len(), r.response_body.len(), r.status, r.url.len());
            let e = self.parsed.entry(r.id).or_insert_with(|| (fp, parse(r).map(Arc::new)));
            if e.0 != fp {
                *e = (fp, parse(r).map(Arc::new));
            }
            if let Some(c) = &e.1 {
                out.push(c.clone());
            }
        }
        if self.parsed.len() > rows.len() * 2 + 64 {
            let ids: std::collections::HashSet<usize> = rows.iter().map(|r| r.id).collect();
            self.parsed.retain(|k, _| ids.contains(k));
        }
        out
    }

    /// Offers newly captured rows to the cache when "add captured calls automatically" is on.
    pub fn auto_add_to_cache(&mut self, rows: &[SessionRecord]) {
        if rows.len() < self.cache_scanned {
            self.cache_scanned = 0;
        }
        if rows.len() == self.cache_scanned {
            return;
        }
        let start = self.cache_scanned;
        self.cache_scanned = rows.len();
        let Ok(mut g) = global_cache().write() else { return };
        if !g.auto_add {
            return;
        }
        let mut added = false;
        for r in &rows[start..] {
            added |= g.add(r);
        }
        if added {
            let _ = g.save(&LlmCache::default_path());
        }
    }
}

impl GridApp {
    /// Toolbar entry: the "AI" menu that opens this module's windows.
    pub fn llm_traffic_menu(&mut self, ui: &mut egui::Ui) {
        let s = &mut self.ext.llm;
        ui.menu_button("AI \u{25be}", |ui| {
            let items: [(&mut bool, &str, &str); 5] = [
                (&mut s.show_inspector, "LLM inspector...", "Model, prompt, messages, tools, tokens, latency and cost of the selected LLM API call"),
                (&mut s.show_calls, "LLM calls...", "Every captured LLM API call with token and cost totals"),
                (&mut s.show_agents, "Agent calls...", "Consecutive calls of one AI-agent conversation grouped together"),
                (&mut s.show_cache, "LLM response cache...", "Serve identical LLM requests from captured responses"),
                (&mut s.show_prices, "Price table...", "Editable per-model token prices used for cost estimates"),
            ];
            for (flag, label, hint) in items {
                if ui.selectable_label(*flag, label).on_hover_text(hint).clicked() {
                    *flag = !*flag;
                    ui.close();
                }
            }
        });
    }

    /// Windows this module draws each frame.
    pub fn llm_traffic_windows(&mut self, ctx: &egui::Context) {
        self.ext.llm.auto_add_to_cache(&self.model.rows);
        if !self.ext.llm.any_open() {
            return;
        }
        let ids = self.selected_ids();
        if let Some(id) = draw_windows(ctx, &mut self.ext.llm, &self.model.rows, self.model.selected, &ids) {
            self.model.selected = Some(id);
            self.ext.sel.only(id);
        }
    }
}

fn short(s: &str, n: usize) -> String {
    let s = s.replace(['\n', '\r'], " ");
    if s.chars().count() <= n {
        s
    } else {
        format!("{}...", s.chars().take(n).collect::<String>())
    }
}

fn limited(s: &str) -> String {
    let n = s.chars().count();
    if n <= TEXT_LIMIT {
        s.to_string()
    } else {
        format!("{}\n... ({} more characters)", s.chars().take(TEXT_LIMIT).collect::<String>(), n - TEXT_LIMIT)
    }
}

/// 1234567 -> "1,234,567"
pub fn fmt_tokens(n: u64) -> String {
    let t = n.to_string();
    let mut out = String::new();
    for (i, ch) in t.chars().enumerate() {
        if i > 0 && (t.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

pub fn fmt_cost(c: Option<f64>) -> String {
    match c {
        None => "no price".into(),
        Some(x) if x == 0.0 => "$0".into(),
        Some(x) if x < 0.01 => format!("${x:.5}"),
        Some(x) => format!("${x:.4}"),
    }
}

fn role_color(role: &str) -> Color32 {
    match role {
        "user" => Color32::from_rgb(70, 130, 220),
        "assistant" | "model" => Color32::from_rgb(40, 160, 90),
        "tool" | "function" => Color32::from_rgb(200, 130, 30),
        _ => Color32::GRAY,
    }
}

fn part_ui(ui: &mut egui::Ui, p: &Part) {
    match p {
        Part::Text(t) => {
            ui.label(limited(t));
        }
        Part::Thinking(t) => {
            ui.label(RichText::new(format!("[thinking] {}", limited(t))).italics().weak());
        }
        Part::ToolCall(c) => {
            ui.label(RichText::new(format!("[tool call] {}({})", c.name, limited(&c.arguments))).monospace());
            if !c.id.is_empty() {
                ui.label(RichText::new(format!("id {}", c.id)).small().weak());
            }
        }
        Part::ToolResult { id, content } => {
            let for_id = if id.is_empty() { String::new() } else { format!(" for {id}") };
            ui.label(RichText::new(format!("[tool result{for_id}] {}", limited(content))).monospace());
        }
        Part::Media(m) => {
            ui.label(RichText::new(format!("[{m}]")).weak());
        }
    }
}

fn message_ui(ui: &mut egui::Ui, i: usize, m: &Message) {
    ui.label(RichText::new(format!("{}. {}", i + 1, m.role)).strong().color(role_color(&m.role)));
    ui.indent(("llm_msg", i), |ui| {
        for p in &m.parts {
            part_ui(ui, p);
        }
        if m.parts.is_empty() {
            ui.label(RichText::new("(empty)").weak());
        }
    });
}

/// Draws every open LLM window over `rows` (all captured sessions). `selected` is the focused session and
/// `selected_ids` every selected one. Returns the id of a session the user clicked, to be selected in the grid.
pub fn draw_windows(ctx: &egui::Context, st: &mut State, rows: &[SessionRecord], selected: Option<usize>, selected_ids: &[usize]) -> Option<usize> {
    let mut picked = None;
    let calls = if st.show_calls || st.show_agents || st.show_inspector { st.calls(rows) } else { Vec::new() };
    let prices = st.prices().clone();

    if st.show_inspector {
        let mut open = true;
        egui::Window::new("LLM inspector").open(&mut open).default_width(560.0).default_height(560.0).show(ctx, |ui| {
            match selected.and_then(|id| calls.iter().find(|c| c.id == id)) {
                None => {
                    ui.label(match selected {
                        None => "Select a session in the grid.",
                        Some(_) => "The selected session is not a recognized LLM API call (OpenAI-compatible, Anthropic, Gemini or Ollama).",
                    });
                    if let Some(last) = calls.last() {
                        if ui.button(format!("Select the latest LLM call (#{})", last.id)).clicked() {
                            picked = Some(last.id);
                        }
                    }
                }
                Some(c) => {
                    let owned: Vec<LlmCall> = calls.iter().map(|c| (**c).clone()).collect();
                    inspector_ui(ui, c, &prices, &group_agents(&owned), &mut picked);
                }
            }
        });
        st.show_inspector &= open;
    }

    if st.show_calls {
        let mut open = true;
        egui::Window::new("LLM calls").open(&mut open).default_width(820.0).default_height(420.0).show(ctx, |ui| {
            calls_ui(ui, &calls, &prices, selected, &mut picked);
        });
        st.show_calls &= open;
    }

    if st.show_agents {
        let mut open = true;
        let mut singles = st.show_single_calls;
        egui::Window::new("Agent calls").open(&mut open).default_width(720.0).default_height(460.0).show(ctx, |ui| {
            agents_ui(ui, &calls, &prices, selected, &mut singles, &mut picked);
        });
        st.show_single_calls = singles;
        st.show_agents &= open;
    }

    if st.show_cache {
        let mut open = true;
        egui::Window::new("LLM response cache").open(&mut open).default_width(640.0).default_height(380.0).show(ctx, |ui| {
            cache_ui(ui, st, rows, selected, selected_ids, &mut picked);
        });
        st.show_cache &= open;
    }

    if st.show_prices {
        let mut open = true;
        egui::Window::new("LLM price table").open(&mut open).default_width(620.0).default_height(460.0).show(ctx, |ui| {
            prices_ui(ui, st);
        });
        st.show_prices &= open;
    }
    picked
}

fn inspector_ui(ui: &mut egui::Ui, c: &LlmCall, prices: &PriceTable, groups: &[AgentGroup], picked: &mut Option<usize>) {
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        egui::Grid::new("llm_head").num_columns(2).striped(true).show(ui, |ui| {
            let mut row = |k: &str, v: String| {
                ui.label(RichText::new(k).strong());
                ui.label(v);
                ui.end_row();
            };
            row("Session", format!("#{}  {}", c.id, c.start_time));
            row("Provider", c.provider.clone());
            row("API", c.kind.map(|k| k.label()).unwrap_or("").to_string());
            row("Model", if c.model.is_empty() { "(not given)".into() } else { c.model.clone() });
            row("Response", format!("HTTP {}{}", c.status, if c.streamed { ", streamed (reassembled)" } else { "" }));
            row("Stop reason", if c.stop_reason.is_empty() { "-".into() } else { c.stop_reason.clone() });
            if let Some(e) = &c.error {
                ui.label(RichText::new("Error").strong().color(Color32::RED));
                ui.label(RichText::new(short(e, 300)).color(Color32::RED));
                ui.end_row();
            }
        });
        if let Some(g) = groups.iter().find(|g| g.ids.len() > 1 && g.ids.contains(&c.id)) {
            let k = g.ids.iter().position(|&i| i == c.id).unwrap_or(0);
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("Step {} of {} in an agent call:", k + 1, g.ids.len()));
                for &id in &g.ids {
                    if ui.selectable_label(id == c.id, format!("#{id}")).clicked() && id != c.id {
                        *picked = Some(id);
                    }
                }
            });
        }
        ui.separator();
        ui.label(RichText::new("Tokens").strong());
        let u = &c.usage;
        if !u.reported {
            ui.label(RichText::new("The response carried no usage numbers.").weak());
        }
        egui::Grid::new("llm_tokens").num_columns(6).striped(true).show(ui, |ui| {
            for h in ["input", "cached", "cache write", "output", "thinking", "total"] {
                ui.label(RichText::new(h).strong());
            }
            ui.end_row();
            for v in [u.input, u.cached, u.cache_write, u.output, u.thinking, u.total()] {
                ui.label(fmt_tokens(v));
            }
            ui.end_row();
        });
        ui.label(RichText::new("Latency").strong());
        ui.label(format!(
            "TTFT {}   total {} ms   output {}",
            c.ttft_ms.map(|t| format!("{t} ms")).unwrap_or_else(|| "-".into()),
            c.duration_ms,
            c.tokens_per_sec().map(|t| format!("{t:.1} tokens/s")).unwrap_or_else(|| "- tokens/s".into())
        ));
        ui.label(RichText::new("Cost estimate").strong());
        if is_local_provider(&c.provider) {
            ui.label("$0 (local model)");
        } else {
            match (prices.cost(c), prices.find(&c.model)) {
                (Some(x), Some(p)) => {
                    ui.label(format!("input {}  +  cached {}  +  output {}  =  {}", fmt_cost(Some(x.input)), fmt_cost(Some(x.cached)), fmt_cost(Some(x.output)), fmt_cost(Some(x.total()))));
                    ui.label(RichText::new(format!("Price \"{}\": ${}/M in, ${}/M out (editable estimate - AI > Price table)", p.pattern, p.input_per_m, p.output_per_m)).small().weak());
                }
                _ => {
                    ui.label(format!("No price for model \"{}\" - add one in AI > Price table.", c.model));
                }
            }
        }
        ui.separator();
        egui::CollapsingHeader::new(format!("System prompt ({} chars)", c.system.chars().count())).id_salt("llm_sys").default_open(!c.system.is_empty()).show(ui, |ui| {
            if c.system.is_empty() {
                ui.label(RichText::new("(none)").weak());
            } else {
                ui.label(limited(&c.system));
            }
        });
        egui::CollapsingHeader::new(format!("Messages ({})", c.messages.len())).id_salt("llm_msgs").default_open(true).show(ui, |ui| {
            for (i, m) in c.messages.iter().enumerate() {
                message_ui(ui, i, m);
            }
        });
        egui::CollapsingHeader::new(format!("Tools offered ({})", c.tools.len())).id_salt("llm_tools").show(ui, |ui| {
            for t in &c.tools {
                ui.label(format!("{}  -  {}", t.name, short(&t.description, 200)));
            }
        });
        egui::CollapsingHeader::new(format!("Parameters ({})", c.params.len())).id_salt("llm_params").show(ui, |ui| {
            egui::Grid::new("llm_params_g").num_columns(2).striped(true).show(ui, |ui| {
                for (k, v) in &c.params {
                    ui.label(k);
                    ui.label(v);
                    ui.end_row();
                }
            });
        });
        egui::CollapsingHeader::new("Assistant reply").id_salt("llm_reply").default_open(true).show(ui, |ui| {
            for p in &c.reply.parts {
                part_ui(ui, p);
            }
            if c.reply.parts.is_empty() {
                ui.label(RichText::new("(no reply content)").weak());
            }
        });
    });
}

fn totals_line(ui: &mut egui::Ui, t: &Totals) {
    ui.label(format!(
        "{} calls   input {}   (cached {})   output {}   (thinking {})   cost {}{}",
        t.calls,
        fmt_tokens(t.input),
        fmt_tokens(t.cached),
        fmt_tokens(t.output),
        fmt_tokens(t.thinking),
        fmt_cost(Some(t.cost)),
        if t.unpriced > 0 { format!("  ({} calls without a price)", t.unpriced) } else { String::new() }
    ));
}

fn calls_ui(ui: &mut egui::Ui, calls: &[Arc<LlmCall>], prices: &PriceTable, selected: Option<usize>, picked: &mut Option<usize>) {
    let t = totals(calls.iter().map(|c| &**c), prices);
    totals_line(ui, &t);
    ui.label(RichText::new("Costs are estimates from the editable price table. Click a row to select it in the grid.").small().weak());
    if calls.is_empty() {
        ui.label("No LLM API calls captured yet.");
        return;
    }
    egui::CollapsingHeader::new("By model").id_salt("llm_by_model").show(ui, |ui| {
        let mut models: Vec<String> = calls.iter().map(|c| c.model.clone()).collect();
        models.sort();
        models.dedup();
        egui::Grid::new("llm_models").num_columns(5).striped(true).show(ui, |ui| {
            for h in ["model", "calls", "input", "output", "cost"] {
                ui.label(RichText::new(h).strong());
            }
            ui.end_row();
            for m in models {
                let t = totals(calls.iter().filter(|c| c.model == m).map(|c| &**c), prices);
                ui.label(if m.is_empty() { "(unknown)".to_string() } else { m });
                ui.label(t.calls.to_string());
                ui.label(fmt_tokens(t.input));
                ui.label(fmt_tokens(t.output));
                ui.label(fmt_cost(Some(t.cost)));
                ui.end_row();
            }
        });
    });
    egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
        egui::Grid::new("llm_calls").num_columns(10).striped(true).show(ui, |ui| {
            for h in ["#", "time", "provider", "model", "input", "output", "cost", "TTFT", "total", "stop"] {
                ui.label(RichText::new(h).strong());
            }
            ui.end_row();
            for c in calls.iter().rev().take(LIST_LIMIT) {
                if ui.selectable_label(selected == Some(c.id), format!("#{}", c.id)).on_hover_text("Select this session in the grid").clicked() {
                    *picked = Some(c.id);
                }
                ui.label(c.start_time.get(11..19).unwrap_or(""));
                ui.label(short(&c.provider, 28));
                ui.label(short(&c.model, 36));
                ui.label(fmt_tokens(c.usage.input));
                ui.label(fmt_tokens(c.usage.output));
                ui.label(fmt_cost(prices.cost(c).map(|x| x.total())));
                ui.label(c.ttft_ms.map(|t| format!("{t} ms")).unwrap_or_else(|| "-".into()));
                ui.label(format!("{} ms", c.duration_ms));
                let stop = match &c.error {
                    Some(e) => RichText::new(short(e, 30)).color(Color32::RED),
                    None => RichText::new(short(&c.stop_reason, 24)),
                };
                ui.label(stop);
                ui.end_row();
            }
        });
        if calls.len() > LIST_LIMIT {
            ui.label(RichText::new(format!("... and {} older calls", calls.len() - LIST_LIMIT)).italics());
        }
    });
}

fn agents_ui(ui: &mut egui::Ui, calls: &[Arc<LlmCall>], prices: &PriceTable, selected: Option<usize>, singles: &mut bool, picked: &mut Option<usize>) {
    let owned: Vec<LlmCall> = calls.iter().map(|c| (**c).clone()).collect();
    let groups = group_agents(&owned);
    let by_id: HashMap<usize, &LlmCall> = owned.iter().map(|c| (c.id, c)).collect();
    let multi = groups.iter().filter(|g| g.ids.len() > 1).count();
    ui.horizontal(|ui| {
        ui.label(format!("{multi} agent calls (2+ steps), {} conversations in all", groups.len()));
        ui.checkbox(singles, "Show single calls");
    });
    ui.label(RichText::new("A step continues a conversation when its message list repeats the previous step's messages and adds the reply and tool results. Click a step to select it in the grid.").small().weak());
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for g in groups.iter().rev().filter(|g| *singles || g.ids.len() > 1).take(LIST_LIMIT) {
            let steps: Vec<&LlmCall> = g.ids.iter().filter_map(|id| by_id.get(id).copied()).collect();
            let t = totals(steps.iter().copied(), prices);
            let tools: usize = steps.iter().map(|c| c.reply.parts.iter().filter(|p| matches!(p, Part::ToolCall(_))).count()).sum();
            let title = format!(
                "#{}  {}  {}  -  {} steps, {} tool calls, in {} / out {} tokens, {}, {} ms",
                g.ids[0],
                short(&g.provider, 24),
                short(&g.model, 30),
                g.ids.len(),
                tools,
                fmt_tokens(t.input),
                fmt_tokens(t.output),
                fmt_cost(Some(t.cost)),
                t.duration_ms
            );
            let has_sel = selected.map(|s| g.ids.contains(&s)).unwrap_or(false);
            let color = if has_sel { Color32::from_rgb(70, 130, 220) } else { ui.visuals().text_color() };
            egui::CollapsingHeader::new(RichText::new(title).strong().color(color)).id_salt(("llm_agent", g.ids[0])).show(ui, |ui| {
                egui::Grid::new(("llm_agent_g", g.ids[0])).num_columns(7).striped(true).show(ui, |ui| {
                    for h in ["step", "session", "new msgs", "input", "output", "cost", "reply"] {
                        ui.label(RichText::new(h).strong());
                    }
                    ui.end_row();
                    let mut prev_len = 0;
                    for (k, c) in steps.iter().enumerate() {
                        ui.label((k + 1).to_string());
                        if ui.selectable_label(selected == Some(c.id), format!("#{}", c.id)).clicked() {
                            *picked = Some(c.id);
                        }
                        ui.label(format!("+{}", c.messages.len().saturating_sub(prev_len)));
                        prev_len = c.messages.len();
                        ui.label(fmt_tokens(c.usage.input));
                        ui.label(fmt_tokens(c.usage.output));
                        ui.label(fmt_cost(prices.cost(c).map(|x| x.total())));
                        let names: Vec<String> = c.reply.parts.iter().filter_map(|p| if let Part::ToolCall(t) = p { Some(t.name.clone()) } else { None }).collect();
                        let reply = if !names.is_empty() { format!("calls {}", names.join(", ")) } else { short(&c.reply_text(), 60) };
                        ui.label(reply);
                        ui.end_row();
                    }
                });
            });
        }
    });
}

fn cache_ui(ui: &mut egui::Ui, st: &mut State, rows: &[SessionRecord], selected: Option<usize>, selected_ids: &[usize], picked: &mut Option<usize>) {
    let path = LlmCache::default_path();
    let Ok(mut g) = global_cache().write() else {
        ui.label("The cache is unavailable.");
        return;
    };
    let mut changed = false;
    changed |= ui
        .checkbox(&mut g.enabled, "Serve identical LLM requests from the cache")
        .on_hover_text("Same method + URL + request body (JSON key order and whitespace ignored): the proxy answers locally and the AI API is not contacted")
        .changed();
    changed |= ui.checkbox(&mut g.auto_add, "Add captured LLM calls to the cache automatically").changed();
    ui.label(format!("{} cached responses   hits {}   misses {} (this run)", g.entries.len(), g.hits, g.misses));
    ui.horizontal(|ui| {
        if ui.button(format!("Add selected ({})", selected_ids.len())).on_hover_text("Cache the selected sessions' responses").clicked() {
            let n = selected_ids.iter().filter_map(|id| rows.iter().find(|r| r.id == *id)).filter(|r| g.add(r)).count();
            st.status = format!("Added {n} of {} selected session(s) (only successful LLM calls not cached yet).", selected_ids.len());
            changed = true;
        }
        if ui.button("Add all captured LLM calls").clicked() {
            let n = rows.iter().filter(|r| g.add(r)).count();
            st.status = format!("Added {n} LLM call(s).");
            changed = true;
        }
        if ui.button("Clear cache").clicked() {
            g.entries.clear();
            st.status = "Cache cleared.".into();
            changed = true;
        }
    });
    if !st.status.is_empty() {
        ui.label(RichText::new(&st.status).small());
    }
    ui.label(RichText::new(format!("Stored in {}", path.display())).small().weak());
    ui.separator();
    let mut remove = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        egui::Grid::new("llm_cache").num_columns(5).striped(true).show(ui, |ui| {
            for h in ["from", "model", "URL", "hits", ""] {
                ui.label(RichText::new(h).strong());
            }
            ui.end_row();
            for e in g.entries.iter().take(LIST_LIMIT) {
                if ui.selectable_label(selected == Some(e.source_id), format!("#{}", e.source_id)).on_hover_text("Select the session this response came from").clicked() {
                    *picked = Some(e.source_id);
                }
                ui.label(short(&e.model, 30));
                ui.label(short(&e.url, 60));
                ui.label(e.hits.to_string());
                if ui.small_button("Remove").clicked() {
                    remove = Some(e.key.clone());
                }
                ui.end_row();
            }
        });
    });
    if let Some(k) = remove {
        g.remove(&k);
        changed = true;
    }
    if changed {
        if let Err(e) = g.save(&path) {
            st.status = format!("Could not save the cache: {e}");
        }
    }
}

fn prices_ui(ui: &mut egui::Ui, st: &mut State) {
    let path = st.prices_path();
    let mut msg = None;
    {
        let table = st.prices();
        ui.label(RichText::new(&table.note).color(Color32::from_rgb(200, 130, 30)));
        ui.label(RichText::new("USD per 1M tokens. A model uses the longest pattern contained in its name. Local providers (Ollama, LM Studio) cost $0.").small().weak());
        ui.horizontal(|ui| {
            if ui.button("Save").clicked() {
                msg = Some(match table.save(&path) {
                    Ok(()) => format!("Saved to {}", path.display()),
                    Err(e) => format!("Could not save: {e}"),
                });
            }
            if ui.button("Reload").clicked() {
                *table = PriceTable::load(&path);
                msg = Some("Reloaded.".to_string());
            }
            if ui.button("Reset to defaults").clicked() {
                *table = PriceTable::default();
                msg = Some("Defaults restored (not saved yet).".to_string());
            }
            if ui.button("Add row").clicked() {
                table.models.push(ModelPrice { pattern: String::new(), input_per_m: 0.0, output_per_m: 0.0, cached_input_per_m: None });
            }
        });
        let mut remove = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).max_height(340.0).show(ui, |ui| {
            egui::Grid::new("llm_prices").num_columns(5).striped(true).show(ui, |ui| {
                for h in ["model pattern", "input", "output", "cached input", ""] {
                    ui.label(RichText::new(h).strong());
                }
                ui.end_row();
                for (i, p) in table.models.iter_mut().enumerate() {
                    ui.add(egui::TextEdit::singleline(&mut p.pattern).desired_width(170.0));
                    ui.add(egui::DragValue::new(&mut p.input_per_m).speed(0.01).range(0.0..=1000.0).prefix("$"));
                    ui.add(egui::DragValue::new(&mut p.output_per_m).speed(0.01).range(0.0..=1000.0).prefix("$"));
                    ui.horizontal(|ui| {
                        let mut has = p.cached_input_per_m.is_some();
                        if ui.checkbox(&mut has, "").on_hover_text("Separate price for cached input tokens").changed() {
                            p.cached_input_per_m = if has { Some(p.input_per_m / 10.0) } else { None };
                        }
                        if let Some(c) = p.cached_input_per_m.as_mut() {
                            ui.add(egui::DragValue::new(c).speed(0.01).range(0.0..=1000.0).prefix("$"));
                        } else {
                            ui.label(RichText::new("= input").weak());
                        }
                    });
                    if ui.small_button("Delete").clicked() {
                        remove = Some(i);
                    }
                    ui.end_row();
                }
            });
        });
        if let Some(i) = remove {
            table.models.remove(i);
        }
    }
    if let Some(m) = msg {
        st.status = m;
    }
    if !st.status.is_empty() {
        ui.label(RichText::new(&st.status).small());
    }
    ui.label(RichText::new(format!("File: {}", path.display())).small().weak());
}
