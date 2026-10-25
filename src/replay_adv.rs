//! G1 / G12 advanced replay: repeat N times with bounded concurrency and a pause between starts,
//! then a results summary (status counts, min / average / max time, size per run). The window is
//! opened from the grid's context menu ("Replay advanced...").
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use crate::model::{composer_send, ComposerRequest, SessionRecord};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdvancedOptions {
    /// Times each selected session is sent.
    pub count: usize,
    /// Requests in flight at once (1 = one after the other).
    pub concurrency: usize,
    /// Pause between request starts, in milliseconds.
    pub delay_ms: u64,
}

impl Default for AdvancedOptions {
    fn default() -> Self {
        AdvancedOptions { count: 10, concurrency: 1, delay_ms: 0 }
    }
}

/// One finished run.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RunResult {
    /// Job number (0-based, in start order).
    pub n: usize,
    pub session_id: usize,
    pub url: String,
    pub status: u16,
    pub ms: u64,
    pub size: usize,
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Summary {
    pub runs: usize,
    pub ok: usize,
    pub failed: usize,
    /// (status, count) ascending; status 0 = no response.
    pub by_status: Vec<(u16, usize)>,
    pub min_ms: u64,
    pub avg_ms: u64,
    pub max_ms: u64,
    pub total_bytes: usize,
    /// Wall time of the whole batch.
    pub elapsed_ms: u64,
}

/// Status counts and timing statistics. A run "fails" with no response or a 5xx.
pub fn summarize(results: &[RunResult], elapsed: Duration) -> Summary {
    let mut s = Summary { runs: results.len(), elapsed_ms: elapsed.as_millis() as u64, ..Default::default() };
    let mut codes = std::collections::BTreeMap::new();
    let mut total_ms = 0u64;
    for r in results {
        *codes.entry(r.status).or_insert(0usize) += 1;
        if r.status == 0 || r.status >= 500 || !r.error.is_empty() {
            s.failed += 1;
        } else {
            s.ok += 1;
        }
        total_ms += r.ms;
        s.total_bytes += r.size;
    }
    s.by_status = codes.into_iter().collect();
    if !results.is_empty() {
        s.min_ms = results.iter().map(|r| r.ms).min().unwrap_or(0);
        s.max_ms = results.iter().map(|r| r.ms).max().unwrap_or(0);
        s.avg_ms = total_ms / results.len() as u64;
    }
    s
}

/// The jobs: every replayable session, `count` times, interleaved (a, b, a, b, ...).
pub fn jobs(sessions: &[SessionRecord], count: usize) -> Vec<(usize, ComposerRequest)> {
    let reqs: Vec<(usize, ComposerRequest)> = sessions
        .iter()
        .filter(|s| crate::g1_replay::replayable(s))
        .map(|s| (s.id, crate::g1_replay::session_to_request(s)))
        .collect();
    let mut out = Vec::new();
    for _ in 0..count.max(1) {
        out.extend(reqs.iter().cloned());
    }
    out
}

/// Run the jobs through `proxy`. Each result is sent on `tx` as it finishes; `stop` cancels runs
/// that have not started. Blocks until every started run is done.
pub fn run(jobs: Vec<(usize, ComposerRequest)>, proxy: &str, opt: AdvancedOptions, tx: mpsc::Sender<RunResult>, stop: Arc<AtomicBool>) {
    let jobs = Arc::new(jobs);
    let next = Arc::new(AtomicUsize::new(0));
    // Start pacing is shared by all workers: the k-th request starts no earlier than k * delay.
    let pace = Arc::new(Mutex::new(Instant::now()));
    let workers = opt.concurrency.clamp(1, 64).min(jobs.len().max(1));
    std::thread::scope(|sc| {
        for _ in 0..workers {
            let (jobs, next, pace, tx, stop) = (jobs.clone(), next.clone(), pace.clone(), tx.clone(), stop.clone());
            sc.spawn(move || loop {
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                let n = next.fetch_add(1, Ordering::SeqCst);
                let Some((sid, req)) = jobs.get(n) else { return };
                if opt.delay_ms > 0 && n > 0 {
                    let wait = {
                        let mut at = pace.lock().unwrap();
                        let start = (*at + Duration::from_millis(opt.delay_ms)).max(Instant::now());
                        *at = start;
                        start.saturating_duration_since(Instant::now())
                    };
                    std::thread::sleep(wait);
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                }
                let t = Instant::now();
                let r = composer_send(req, proxy);
                let _ = tx.send(RunResult {
                    n,
                    session_id: *sid,
                    url: req.url.clone(),
                    status: r.status,
                    ms: t.elapsed().as_millis() as u64,
                    size: r.body.len(),
                    error: r.error,
                });
            });
        }
    });
}

// ------------------------------------------------------------------------------------------ window

#[derive(Default)]
pub struct ReplayUi {
    pub show: bool,
    pub opt: Option<AdvancedOptions>,
    /// Sessions chosen when the window was opened.
    pub sessions: Vec<SessionRecord>,
    pub results: Vec<RunResult>,
    pub started: Option<Instant>,
    pub finished: Option<Duration>,
    rx: Option<mpsc::Receiver<RunResult>>,
    stop: Option<Arc<AtomicBool>>,
    pub expected: usize,
}

impl ReplayUi {
    pub fn running(&self) -> bool {
        self.rx.is_some()
    }

    /// Start a batch in the background (results arrive through `poll`).
    pub fn start(&mut self, proxy: String) {
        let opt = self.opt.unwrap_or_default();
        let jobs = jobs(&self.sessions, opt.count);
        self.expected = jobs.len();
        self.results.clear();
        self.finished = None;
        self.started = Some(Instant::now());
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        self.rx = Some(rx);
        self.stop = Some(stop.clone());
        std::thread::spawn(move || run(jobs, &proxy, opt, tx, stop));
    }

    pub fn cancel(&mut self) {
        if let Some(s) = &self.stop {
            s.store(true, Ordering::Relaxed);
        }
    }

    /// Collect finished runs; true while the batch is still going.
    pub fn poll(&mut self) -> bool {
        let Some(rx) = &self.rx else { return false };
        loop {
            match rx.try_recv() {
                Ok(r) => self.results.push(r),
                Err(mpsc::TryRecvError::Empty) => return true,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.rx = None;
                    self.stop = None;
                    self.finished = self.started.map(|t| t.elapsed());
                    self.results.sort_by_key(|r| r.n);
                    return false;
                }
            }
        }
    }
}

impl crate::ui::GridApp {
    /// Open the advanced replay window for the selected sessions.
    pub fn replay_adv_open(&mut self) {
        let ids: std::collections::HashSet<usize> = self.selected_ids().into_iter().collect();
        let sessions: Vec<SessionRecord> = self.model.rows.iter().filter(|r| ids.contains(&r.id)).cloned().collect();
        let st = &mut self.ext.replay;
        if st.running() {
            st.show = true;
            return;
        }
        st.sessions = sessions;
        st.results.clear();
        st.finished = None;
        st.opt.get_or_insert_with(AdvancedOptions::default);
        st.show = true;
    }

    pub fn replay_adv_window(&mut self, ctx: &eframe::egui::Context) {
        use eframe::egui::{self, Color32, RichText};
        let st = &mut self.ext.replay;
        if !st.show {
            return;
        }
        if st.poll() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        let mut open = true;
        let (mut go, mut stop) = (false, false);
        egui::Window::new("Replay advanced").open(&mut open).default_width(560.0).show(ctx, |ui| {
            let n = st.sessions.iter().filter(|s| crate::g1_replay::replayable(s)).count();
            ui.label(format!("{n} session(s) selected (CONNECT tunnels are skipped)"));
            let running = st.running();
            let opt = st.opt.get_or_insert_with(AdvancedOptions::default);
            ui.add_enabled_ui(!running, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Times");
                    ui.add(egui::DragValue::new(&mut opt.count).range(1..=10_000));
                    ui.label("In parallel");
                    ui.add(egui::DragValue::new(&mut opt.concurrency).range(1..=64));
                    ui.label("Delay between starts (ms)");
                    ui.add(egui::DragValue::new(&mut opt.delay_ms).range(0..=600_000));
                });
            });
            let count = opt.count;
            ui.horizontal(|ui| {
                if running {
                    ui.label(format!("Running... {}/{}", st.results.len(), st.expected));
                    if ui.button("Stop").clicked() {
                        stop = true;
                    }
                } else if ui.add_enabled(n > 0, egui::Button::new(format!("Run {} request(s)", n * count))).clicked() {
                    go = true;
                }
            });
            if !st.results.is_empty() {
                ui.separator();
                let el = st.finished.or_else(|| st.started.map(|t| t.elapsed())).unwrap_or_default();
                let s = summarize(&st.results, el);
                ui.label(RichText::new(format!(
                    "{} runs in {} ms: {} ok, {} failed; time min {} / avg {} / max {} ms; {} bytes",
                    s.runs, s.elapsed_ms, s.ok, s.failed, s.min_ms, s.avg_ms, s.max_ms, s.total_bytes
                )).strong());
                ui.label(format!("Status: {}", s.by_status.iter().map(|(c, k)| format!("{}: {k}", if *c == 0 { "no response".to_string() } else { c.to_string() })).collect::<Vec<_>>().join(", ")));
                egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                    egui::Grid::new("replay_runs").striped(true).show(ui, |ui| {
                        for h in ["#", "Session", "Status", "Time (ms)", "Size", "URL / error"] {
                            ui.strong(h);
                        }
                        ui.end_row();
                        for r in &st.results {
                            ui.label((r.n + 1).to_string());
                            ui.label(format!("#{}", r.session_id));
                            let c = if r.status == 0 || r.status >= 500 { Color32::from_rgb(230, 90, 90) } else if r.status >= 400 { Color32::from_rgb(230, 160, 40) } else { Color32::LIGHT_GREEN };
                            ui.colored_label(c, if r.status == 0 { "-".to_string() } else { r.status.to_string() });
                            ui.label(r.ms.to_string());
                            ui.label(r.size.to_string());
                            ui.label(if r.error.is_empty() { r.url.clone() } else { r.error.clone() });
                            ui.end_row();
                        }
                    });
                });
            }
        });
        if go {
            st.start(crate::ui::our_proxy_server());
        }
        if stop {
            st.cancel();
        }
        st.show = open;
        if !open {
            st.cancel();
        }
    }
}
