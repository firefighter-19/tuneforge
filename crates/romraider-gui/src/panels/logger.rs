//! Logger panel — live XY-plot значений с ECU.
//!
//! Поток:
//! 1. Кнопка `Load log_defs.xml…` → парс + резолв указанного ECU (default `base`).
//! 2. В прокручиваемом списке выбираешь параметры (checkbox).
//! 3. Заполняешь port + baud + interval, жмёшь `▶ Start`.
//! 4. Worker-thread в фоне дёргает `LoggerSession::poll_once` каждые `interval`
//!    миллисекунд, отправляет `Sample` в `std::sync::mpsc` канал.
//! 5. На каждом UI-frame `poll_samples` drain-ит канал и кладёт точки в
//!    `history: BTreeMap<param_id, VecDeque>` (cap = 600).
//! 6. `egui_plot::Plot` рисует по одной `Line` на параметр.
//! 7. `⏹ Stop` ставит `stop_flag`, worker dies grаcefully.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use egui_plot::{Legend, Line, Plot, PlotPoints};
use romraider_defs::{parse_log_file, LoggerDocument, ResolvedLogEcu};
use romraider_io::serial::{SerialConfig, SerialTransport};
use romraider_io::Transport;
use romraider_logger::{LoggerSession, Sample, SessionConfig};
use tracing::warn;

const MAX_POINTS_PER_PARAM: usize = 600; // 60 s @ 10 Hz

pub struct LoggerPanel {
    log_def: Option<DefState>,
    resolved: Option<ResolvedLogEcu>,
    ecu_id: String,
    selected_params: BTreeSet<String>,
    port: String,
    baud: u32,
    interval_ms: u64,
    timeout_ms: u64,

    worker: Option<Worker>,
    history: BTreeMap<String, VecDeque<[f64; 2]>>,
    started_at: Option<Instant>,
    error: Option<String>,
}

struct DefState {
    path: PathBuf,
    doc: LoggerDocument,
}

struct Worker {
    stop_flag: Arc<AtomicBool>,
    sample_rx: mpsc::Receiver<Sample>,
    _handle: JoinHandle<()>,
}

impl Default for LoggerPanel {
    fn default() -> Self {
        Self {
            log_def: None,
            resolved: None,
            ecu_id: "base".into(),
            selected_params: BTreeSet::new(),
            port: String::new(),
            baud: 4800,
            interval_ms: 100,
            timeout_ms: 1500,
            worker: None,
            history: BTreeMap::new(),
            started_at: None,
            error: None,
        }
    }
}

impl LoggerPanel {
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        self.drain_samples();

        self.render_def_row(ui);
        ui.separator();

        if let Some(err) = self.error.clone() {
            ui.horizontal(|ui| {
                ui.colored_label(egui::Color32::LIGHT_RED, err);
                if ui.small_button("✕").clicked() {
                    self.error = None;
                }
            });
        }

        if self.log_def.is_some() {
            self.render_ecu_and_params(ui);
            ui.separator();
            self.render_serial_controls(ui);
            ui.separator();
        }

        self.render_plot(ui);

        if self.worker.is_some() {
            // Поддерживаем 2x-частоту репейнта от интервала опроса.
            ui.ctx()
                .request_repaint_after(Duration::from_millis(self.interval_ms / 2 + 10));
        }
    }

    fn render_def_row(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("Load log_defs.xml…").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Logger definitions XML", &["xml"])
                    .pick_file()
                {
                    self.load_log_def(path);
                }
            }
            match &self.log_def {
                Some(d) => {
                    ui.label(d.path.display().to_string());
                }
                None => {
                    ui.label("(no log_defs.xml loaded)");
                }
            }
        });
    }

    fn render_ecu_and_params(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("ECU id:");
            let response = ui.text_edit_singleline(&mut self.ecu_id);
            if response.lost_focus() {
                self.try_resolve_ecu();
            }
            if ui.small_button("Resolve").clicked() {
                self.try_resolve_ecu();
            }
            if let Some(r) = &self.resolved {
                ui.label(format!("→ {} params", r.parameters.len()));
            }
        });

        if let Some(resolved) = &self.resolved {
            ui.label("Parameters to log (toggle):");
            egui::ScrollArea::vertical()
                .max_height(220.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for p in &resolved.parameters {
                        let mut on = self.selected_params.contains(&p.id);
                        let label = format!("{}  ({})", p.id, p.metric.as_deref().unwrap_or("-"));
                        if ui.checkbox(&mut on, label).changed() {
                            if on {
                                self.selected_params.insert(p.id.clone());
                            } else {
                                self.selected_params.remove(&p.id);
                            }
                        }
                    }
                });
        } else if !self.ecu_id.is_empty() {
            ui.colored_label(
                egui::Color32::YELLOW,
                format!("ECU `{}` not resolvable from this def.", self.ecu_id),
            );
        }
    }

    fn render_serial_controls(&mut self, ui: &mut egui::Ui) {
        let running = self.worker.is_some();
        let start_enabled = !running
            && self.log_def.is_some()
            && self.resolved.is_some()
            && !self.selected_params.is_empty()
            && !self.port.is_empty();

        ui.horizontal(|ui| {
            ui.label("Port:");
            ui.add_enabled(
                !running,
                egui::TextEdit::singleline(&mut self.port).desired_width(180.0),
            );
            ui.label("Baud:");
            ui.add_enabled(!running, egui::DragValue::new(&mut self.baud).speed(100.0));
            ui.label("Interval ms:");
            ui.add(
                egui::DragValue::new(&mut self.interval_ms)
                    .speed(10.0)
                    .range(10..=10_000),
            );
            ui.label("Timeout ms:");
            ui.add_enabled(
                !running,
                egui::DragValue::new(&mut self.timeout_ms)
                    .speed(50.0)
                    .range(100..=10_000),
            );
        });

        ui.horizontal(|ui| {
            if ui
                .add_enabled(start_enabled, egui::Button::new("▶ Start"))
                .clicked()
            {
                self.start_worker();
            }
            if ui
                .add_enabled(running, egui::Button::new("⏹ Stop"))
                .clicked()
            {
                self.stop_worker();
            }
            if ui
                .add_enabled(!self.history.is_empty(), egui::Button::new("Clear plot"))
                .clicked()
            {
                self.history.clear();
            }
            if running {
                let elapsed = self.started_at.map(|t| t.elapsed().as_secs()).unwrap_or(0);
                let total: usize = self.history.values().map(VecDeque::len).sum();
                ui.colored_label(
                    egui::Color32::LIGHT_GREEN,
                    format!("Logging… {elapsed}s  ({total} pts)"),
                );
            }
        });
    }

    fn render_plot(&mut self, ui: &mut egui::Ui) {
        Plot::new("logger_plot")
            .height(360.0)
            .legend(Legend::default())
            .show(ui, |plot_ui| {
                for (id, history) in &self.history {
                    let points: PlotPoints = history.iter().copied().collect::<Vec<_>>().into();
                    plot_ui.line(Line::new(points).name(id));
                }
            });
    }

    fn load_log_def(&mut self, path: PathBuf) {
        // Загрузка нового def прерывает текущий worker — параметры могут стать невалидными.
        self.stop_worker();
        match parse_log_file(&path) {
            Ok(doc) => {
                self.log_def = Some(DefState { path, doc });
                self.error = None;
                self.try_resolve_ecu();
            }
            Err(e) => {
                warn!(?e, "log_defs load failed");
                self.error = Some(format!("Failed to load log_defs: {e}"));
            }
        }
    }

    fn try_resolve_ecu(&mut self) {
        let Some(d) = &self.log_def else { return };
        match d.doc.resolve_ecu(&self.ecu_id) {
            Ok(r) => {
                // Удаляем из selection параметры, которых нет в новом resolved.
                let valid: BTreeSet<String> = r.parameters.iter().map(|p| p.id.clone()).collect();
                self.selected_params.retain(|p| valid.contains(p));
                self.resolved = Some(r);
                self.error = None;
            }
            Err(e) => {
                self.resolved = None;
                warn!(?e, ?self.ecu_id, "ecu resolve failed");
            }
        }
    }

    fn start_worker(&mut self) {
        let Some(resolved) = self.resolved.as_ref() else {
            self.error = Some("Resolve an ECU first.".into());
            return;
        };

        // 1. Скомпилировать выбранные параметры.
        let mut session = LoggerSession::new(SessionConfig {
            timeout: Duration::from_millis(self.timeout_ms),
            ..SessionConfig::default()
        });
        for id in &self.selected_params {
            let Some(p) = resolved.find_parameter(id) else {
                self.error = Some(format!("Parameter `{id}` missing in resolved ECU"));
                return;
            };
            match p.compile() {
                Ok(c) => session.subscribe(c),
                Err(e) => {
                    self.error = Some(format!("Compile `{id}`: {e}"));
                    return;
                }
            }
        }

        // 2. Открыть serial.
        let mut cfg = SerialConfig::ssm(&self.port);
        cfg.baud_rate = self.baud;
        let mut transport = match SerialTransport::open(&cfg) {
            Ok(t) => t,
            Err(e) => {
                self.error = Some(format!("Serial open failed: {e}"));
                return;
            }
        };
        if let Err(e) = transport.purge() {
            warn!(?e, "transport purge failed");
        }

        // 3. Channels + thread.
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_clone = Arc::clone(&stop_flag);
        let (tx, rx) = mpsc::channel::<Sample>();
        let interval = Duration::from_millis(self.interval_ms);

        let handle = std::thread::Builder::new()
            .name("romraider-logger".into())
            .spawn(move || logger_worker(session, transport, tx, stop_clone, interval))
            .expect("spawn logger worker");

        self.history.clear();
        self.started_at = Some(Instant::now());
        self.error = None;
        self.worker = Some(Worker {
            stop_flag,
            sample_rx: rx,
            _handle: handle,
        });
    }

    fn stop_worker(&mut self) {
        if let Some(w) = self.worker.take() {
            w.stop_flag.store(true, Ordering::Relaxed);
            // JoinHandle отдаём в Drop — не блокируем UI на join.
        }
        self.started_at = None;
    }

    fn drain_samples(&mut self) {
        let Some(worker) = self.worker.as_ref() else {
            return;
        };
        let t0 = self.started_at.map(|t| t.elapsed().as_secs_f64());
        let Some(t0) = t0 else { return };
        let _ = t0; // for completeness
        while let Ok(sample) = worker.sample_rx.try_recv() {
            let elapsed = self
                .started_at
                .map(|t| t.elapsed().as_secs_f64())
                .unwrap_or(0.0);
            for sv in &sample.values {
                let entry = self.history.entry(sv.parameter_id.clone()).or_default();
                entry.push_back([elapsed, sv.value]);
                if entry.len() > MAX_POINTS_PER_PARAM {
                    entry.pop_front();
                }
            }
        }
    }
}

impl Drop for LoggerPanel {
    fn drop(&mut self) {
        self.stop_worker();
    }
}

fn logger_worker(
    session: LoggerSession,
    mut transport: SerialTransport,
    tx: mpsc::Sender<Sample>,
    stop_flag: Arc<AtomicBool>,
    interval: Duration,
) {
    while !stop_flag.load(Ordering::Relaxed) {
        let started = Instant::now();
        match session.poll_once(&mut transport) {
            Ok(sample) => {
                if tx.send(sample).is_err() {
                    break; // receiver gone
                }
            }
            Err(e) => {
                warn!(?e, "logger poll_once failed; continuing");
            }
        }
        if let Some(rem) = interval.checked_sub(started.elapsed()) {
            std::thread::sleep(rem);
        }
    }
}
