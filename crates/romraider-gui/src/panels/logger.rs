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
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use egui_plot::{Legend, Line, Plot, PlotPoints};
use romraider_defs::{parse_log_file, LoggerDocument, ResolvedLogEcu};
use romraider_io::serial::{SerialConfig, SerialTransport};
use romraider_io::Transport;
use romraider_logger::datalog::DatalogWriter;
use romraider_logger::{LoggerSession, Sample, SampleValue, SessionConfig};
use tracing::warn;

#[cfg(feature = "ecu-tools")]
use romraider_protocol::subaru::{
    self, find_derived_param, find_ssm_param, read_ssm_params_can, SsmDerivedParam, SsmParam,
    SUBARU_DERIVED_PARAMS, SUBARU_SSM_PARAMS,
};

const MAX_POINTS_PER_PARAM: usize = 600; // 60 s @ 10 Hz

/// Доступные транспорты+протоколы логгера. K-Line — legacy/upstream для
/// Subaru 2002-2006, на 2007+ блокируется анти-fuzz. SSM-CAN — Subaru
/// SSM3 проприетарный поверх ISO15765 для 2007+ Subaru через Tactrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoggerMode {
    /// Serial K-Line SSM2 через `log_defs.xml` (как было раньше).
    KLineSSM,
    /// Subaru SSM3 поверх CAN через Tactrix Openport. Требует
    /// feature `ecu-tools` (тянет `romraider-kernel`, GPL-3.0).
    #[cfg(feature = "ecu-tools")]
    SsmCan,
}

impl LoggerMode {
    fn label(&self) -> &'static str {
        match self {
            Self::KLineSSM => "K-Line SSM2 (serial)",
            #[cfg(feature = "ecu-tools")]
            Self::SsmCan => "SSM3 over CAN (Tactrix)",
        }
    }
    fn filename_prefix(&self) -> &'static str {
        match self {
            Self::KLineSSM => "k-line",
            #[cfg(feature = "ecu-tools")]
            Self::SsmCan => "ssm-can",
        }
    }
}

pub struct LoggerPanel {
    mode: LoggerMode,
    log_def: Option<DefState>,
    resolved: Option<ResolvedLogEcu>,
    ecu_id: String,
    selected_params: BTreeSet<String>,
    port: String,
    baud: u32,
    interval_ms: u64,
    timeout_ms: u64,
    save_path: PathBuf,
    /// Когда `false`, кнопка Save отображает текст из `save_path`. Когда `true`,
    /// прячем и используем дефолтный auto-timestamp путь.
    auto_save_name: bool,

    worker: Option<Worker>,
    history: BTreeMap<String, VecDeque<[f64; 2]>>,
    /// `parameter_id → units string` — для отрисовки live readout с правильными
    /// units. Populated в start_*_worker когда формируется subscription.
    units_lookup: BTreeMap<String, String>,
    started_at: Option<Instant>,
    error: Option<String>,
}

/// Sensible defaults для SSM3-CAN mode — diagnostic kit покрывающий
/// engine health + AVCS (для проверки ремня/OCV) + fuel delivery
/// (для weak-pump / saturated-injectors диагностики).
const SSM_CAN_DEFAULTS: &[&str] = &[
    "RPM",
    "Coolant Temp",
    "IAT",
    "MAF",
    "TPS",
    "MAP",
    "Intake AVCS Right",
    "Intake AVCS Left",
    "AVCS Diff (R-L)",
    "Fuel Injector #1 PW",
    "Injector Duty Cycle",
];

/// Defaults для K-Line SSM2 mode — самые востребованные стандартные
/// SSM-параметры из log_defs.xml.
const KLINE_DEFAULTS: &[&str] = &[
    "Engine Speed",
    "Coolant Temperature",
    "Mass Air Flow",
    "Throttle Opening Angle",
    "Manifold Absolute Pressure",
    "Air/Fuel Correction #1",
    "Knock Correction",
];

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
        // Default mode = SSM3-CAN если фича доступна (для современных Subaru
        // 2007+ это primary path), иначе K-Line (legacy / 2002-2006).
        #[cfg(feature = "ecu-tools")]
        let default_mode = LoggerMode::SsmCan;
        #[cfg(not(feature = "ecu-tools"))]
        let default_mode = LoggerMode::KLineSSM;

        let mut selected = BTreeSet::new();
        #[cfg(feature = "ecu-tools")]
        if default_mode == LoggerMode::SsmCan {
            for name in SSM_CAN_DEFAULTS {
                selected.insert((*name).into());
            }
        }

        Self {
            mode: default_mode,
            log_def: None,
            resolved: None,
            ecu_id: "base".into(),
            selected_params: selected,
            port: String::new(),
            baud: 4800,
            interval_ms: 100,
            timeout_ms: 1500,
            save_path: default_log_path(default_mode),
            auto_save_name: true,
            worker: None,
            history: BTreeMap::new(),
            units_lookup: BTreeMap::new(),
            started_at: None,
            error: None,
        }
    }
}

/// `$HOME/Documents/RomRaider/logs/<mode>-YYYY-MM-DD_HH-MM-SS.csv`.
fn default_log_path(mode: LoggerMode) -> PathBuf {
    let home = std::env::var_os("HOME").unwrap_or_else(|| ".".into());
    let dir = PathBuf::from(home)
        .join("Documents")
        .join("RomRaider")
        .join("logs");
    let ts = format_ts_now();
    dir.join(format!("{}-{}.csv", mode.filename_prefix(), ts))
}

fn format_ts_now() -> String {
    // YYYY-MM-DD_HH-MM-SS using local time approximation via UNIX epoch
    // (без external chrono — std достаточно для grep-friendly имени файла).
    let epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    // simple Y-M-D conversion (Howard Hinnant date algo, public domain)
    let z = epoch / 86400 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    let sec = (epoch % 86400) as u32;
    let h = sec / 3600;
    let mi = (sec / 60) % 60;
    let s = sec % 60;
    format!("{y:04}-{m:02}-{d:02}_{h:02}-{mi:02}-{s:02}")
}

impl LoggerPanel {
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        self.drain_samples();

        self.render_mode_selector(ui);
        ui.separator();

        if let Some(err) = self.error.clone() {
            ui.horizontal(|ui| {
                ui.colored_label(egui::Color32::LIGHT_RED, err);
                if ui.small_button("✕").clicked() {
                    self.error = None;
                }
            });
        }

        match self.mode {
            LoggerMode::KLineSSM => {
                self.render_def_row(ui);
                ui.separator();
                if self.log_def.is_some() {
                    self.render_ecu_and_params(ui);
                    ui.separator();
                }
            }
            #[cfg(feature = "ecu-tools")]
            LoggerMode::SsmCan => {
                self.render_ssm_can_params(ui);
                ui.separator();
            }
        }

        self.render_save_path(ui);
        ui.separator();
        self.render_connection_controls(ui);
        ui.separator();

        self.render_live_readout(ui);
        ui.separator();

        self.render_plot(ui);

        if self.worker.is_some() {
            ui.ctx()
                .request_repaint_after(Duration::from_millis(self.interval_ms / 2 + 10));
        }
    }

    fn render_mode_selector(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Mode:");
            let running = self.worker.is_some();
            egui::ComboBox::from_id_salt("logger_mode")
                .selected_text(self.mode.label())
                .show_ui(ui, |ui| {
                    if ui
                        .add_enabled(
                            !running,
                            egui::SelectableLabel::new(
                                self.mode == LoggerMode::KLineSSM,
                                LoggerMode::KLineSSM.label(),
                            ),
                        )
                        .clicked()
                    {
                        self.switch_mode(LoggerMode::KLineSSM);
                    }
                    #[cfg(feature = "ecu-tools")]
                    if ui
                        .add_enabled(
                            !running,
                            egui::SelectableLabel::new(
                                self.mode == LoggerMode::SsmCan,
                                LoggerMode::SsmCan.label(),
                            ),
                        )
                        .clicked()
                    {
                        self.switch_mode(LoggerMode::SsmCan);
                    }
                });
        });
    }

    fn switch_mode(&mut self, new_mode: LoggerMode) {
        if new_mode == self.mode {
            return;
        }
        self.mode = new_mode;
        self.selected_params.clear();
        self.history.clear();
        // Populate sensible defaults for the new mode.
        match new_mode {
            #[cfg(feature = "ecu-tools")]
            LoggerMode::SsmCan => {
                for n in SSM_CAN_DEFAULTS {
                    self.selected_params.insert((*n).into());
                }
            }
            LoggerMode::KLineSSM => {
                // Если ECU уже резолвлен — auto-pick defaults которые есть в resolved.
                if let Some(r) = &self.resolved {
                    for n in KLINE_DEFAULTS {
                        if r.find_parameter(n).is_some() {
                            self.selected_params.insert((*n).into());
                        }
                    }
                }
            }
        }
        if self.auto_save_name {
            self.save_path = default_log_path(new_mode);
        }
    }

    #[cfg(feature = "ecu-tools")]
    fn render_ssm_can_params(&mut self, ui: &mut egui::Ui) {
        let selected_count = self.selected_params.len();
        let total = SUBARU_SSM_PARAMS.len() + SUBARU_DERIVED_PARAMS.len();
        let header = format!(
            "▾ Parameters: {selected_count} selected of {total} ({} raw + {} derived)",
            SUBARU_SSM_PARAMS.len(),
            SUBARU_DERIVED_PARAMS.len()
        );
        egui::CollapsingHeader::new(header)
            .id_salt("ssm_can_params_header")
            .default_open(false)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("ssm_can_params")
                    .max_height(260.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new("RAW (читаются с ECU):").strong());
                        for p in SUBARU_SSM_PARAMS {
                            let mut on = self.selected_params.contains(p.name);
                            let label = format!("{} ({}) [{}]", p.name, p.units, p.id);
                            if ui.checkbox(&mut on, label).changed() {
                                if on {
                                    self.selected_params.insert(p.name.into());
                                } else {
                                    self.selected_params.remove(p.name);
                                }
                            }
                        }
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new("DERIVED (computed from raw, auto-include deps):")
                                .strong(),
                        );
                        for d in SUBARU_DERIVED_PARAMS {
                            let mut on = self.selected_params.contains(d.name);
                            let label = format!(
                                "{} ({}) = f({})",
                                d.name,
                                d.units,
                                d.depends_on.join("+")
                            );
                            if ui.checkbox(&mut on, label).changed() {
                                if on {
                                    self.selected_params.insert(d.name.into());
                                } else {
                                    self.selected_params.remove(d.name);
                                }
                            }
                        }
                    });
            });
    }

    fn render_save_path(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Save to:");
            let path_str = self.save_path.display().to_string();
            ui.add(
                egui::TextEdit::singleline(&mut path_str.clone())
                    .desired_width(380.0)
                    .interactive(false),
            );
            if ui.button("📁 Change…").clicked() {
                if let Some(p) = rfd::FileDialog::new()
                    .add_filter("CSV log", &["csv"])
                    .set_file_name(
                        self.save_path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("log.csv"),
                    )
                    .save_file()
                {
                    self.save_path = p;
                    self.auto_save_name = false;
                }
            }
            if !self.auto_save_name && ui.button("⟲ Auto").clicked() {
                self.save_path = default_log_path(self.mode);
                self.auto_save_name = true;
            }
        });
    }

    fn render_connection_controls(&mut self, ui: &mut egui::Ui) {
        let running = self.worker.is_some();
        let start_enabled = !running
            && !self.selected_params.is_empty()
            && match self.mode {
                LoggerMode::KLineSSM => {
                    self.log_def.is_some() && self.resolved.is_some() && !self.port.is_empty()
                }
                #[cfg(feature = "ecu-tools")]
                LoggerMode::SsmCan => true, // Tactrix-CAN не требует port/baud
            };

        // K-Line-специфичные поля (port + baud) только в K-Line mode.
        if matches!(self.mode, LoggerMode::KLineSSM) {
            ui.horizontal(|ui| {
                ui.label("Port:");
                ui.add_enabled(
                    !running,
                    egui::TextEdit::singleline(&mut self.port).desired_width(180.0),
                );
                ui.label("Baud:");
                ui.add_enabled(!running, egui::DragValue::new(&mut self.baud).speed(100.0));
            });
        }

        ui.horizontal(|ui| {
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
            let header = format!(
                "▾ Parameters: {} selected of {}",
                self.selected_params.len(),
                resolved.parameters.len()
            );
            egui::CollapsingHeader::new(header)
                .id_salt("kline_params_header")
                .default_open(false)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("kline_params")
                        .max_height(260.0)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for p in &resolved.parameters {
                                let mut on = self.selected_params.contains(&p.id);
                                let label =
                                    format!("{}  ({})", p.id, p.metric.as_deref().unwrap_or("-"));
                                if ui.checkbox(&mut on, label).changed() {
                                    if on {
                                        self.selected_params.insert(p.id.clone());
                                    } else {
                                        self.selected_params.remove(&p.id);
                                    }
                                }
                            }
                        });
                });
        } else if !self.ecu_id.is_empty() {
            ui.colored_label(
                egui::Color32::YELLOW,
                format!("ECU `{}` not resolvable from this def.", self.ecu_id),
            );
        }
    }

    /// Numeric live readout — последнее значение каждого подписанного param-а.
    /// Auto-обновляется через `request_repaint_after` пока worker крутится.
    fn render_live_readout(&mut self, ui: &mut egui::Ui) {
        if self.history.is_empty() && self.selected_params.is_empty() {
            return;
        }
        ui.label("Live readout:");
        let cols = 3; // 3-колоночная сетка, ~равномерно
        egui::Grid::new("live_readout")
            .num_columns(cols * 2)
            .spacing([16.0, 4.0])
            .show(ui, |ui| {
                let mut i = 0;
                for name in &self.selected_params {
                    let last = self.history.get(name).and_then(|h| h.back().copied());
                    let units = self.units_lookup.get(name).map(String::as_str).unwrap_or("");
                    ui.monospace(format!("{name}:"));
                    let value_text = match last {
                        Some([_, v]) => {
                            // Авто-precision: тип-int если ≥100 и нет десятичной,
                            // иначе 2 знака после.
                            if v.abs() >= 100.0 || v.fract().abs() < 0.005 {
                                format!("{v:>8.0} {units}")
                            } else {
                                format!("{v:>8.2} {units}")
                            }
                        }
                        None => "      — ".into(),
                    };
                    ui.monospace(value_text);
                    i += 1;
                    if i % cols == 0 {
                        ui.end_row();
                    }
                }
                if i % cols != 0 {
                    ui.end_row();
                }
            });
    }

    fn render_plot(&mut self, ui: &mut egui::Ui) {
        if self.history.is_empty() {
            ui.label("(no data yet)");
            return;
        }
        ui.label("Plots (linked X-axis):");
        // Shared X-axis group: pan/zoom одной — синхронизуются все. Y per-plot
        // (каждый param сам auto-scale-ится в свой диапазон, RPM не давит AVCS).
        let x_link = egui::Id::new("logger_plots_x_link");
        // Render order = same as live readout (selected_params order).
        // Каждый plot ~140px, ScrollArea если много params.
        egui::ScrollArea::vertical()
            .id_salt("plots_scroll")
            .max_height(640.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for name in &self.selected_params {
                    let Some(history) = self.history.get(name) else {
                        continue;
                    };
                    if history.is_empty() {
                        continue;
                    }
                    let points: PlotPoints =
                        history.iter().copied().collect::<Vec<_>>().into();
                    let units = self
                        .units_lookup
                        .get(name)
                        .map(String::as_str)
                        .unwrap_or("");
                    let title = if units.is_empty() {
                        name.clone()
                    } else {
                        format!("{name}  [{units}]")
                    };
                    ui.label(egui::RichText::new(&title).monospace().strong());
                    Plot::new(format!("plot_{name}"))
                        .height(120.0)
                        .show_axes([true, true])
                        .legend(Legend::default().position(egui_plot::Corner::RightTop))
                        .link_axis(x_link, egui::Vec2b::new(true, false))
                        // Strip-chart UX: Y auto-fits в свой диапазон (отдельно
                        // на каждый plot — RPM 0-7000 не давит AVCS ±2°).
                        // X drag-pan только для скролла по времени.
                        // Zoom (scroll wheel + pinch) **полностью отключён** —
                        // от него все plots начинали жить странной жизнью.
                        // Double-click на plot — reset auto-bounds (built-in egui).
                        .allow_drag(egui::Vec2b::new(true, false))
                        .allow_zoom(false)
                        .allow_scroll(false)
                        .allow_boxed_zoom(false)
                        .auto_bounds(egui::Vec2b::new(true, true))
                        .show(ui, |plot_ui| {
                            plot_ui.line(Line::new(points).name(name));
                        });
                    ui.add_space(4.0);
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
                // Если selection пустой — auto-pick K-Line defaults которые есть в этом ECU.
                if self.selected_params.is_empty() && matches!(self.mode, LoggerMode::KLineSSM) {
                    for n in KLINE_DEFAULTS {
                        if r.find_parameter(n).is_some() {
                            self.selected_params.insert((*n).into());
                        }
                    }
                }
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
        // Create parent dirs for save path
        if let Some(parent) = self.save_path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                self.error = Some(format!("Cannot create log dir {}: {e}", parent.display()));
                return;
            }
        }

        match self.mode {
            LoggerMode::KLineSSM => self.start_kline_worker(),
            #[cfg(feature = "ecu-tools")]
            LoggerMode::SsmCan => self.start_ssm_can_worker(),
        }
    }

    fn start_kline_worker(&mut self) {
        let Some(resolved) = self.resolved.as_ref() else {
            self.error = Some("Resolve an ECU first.".into());
            return;
        };

        // 1. Скомпилировать выбранные параметры + populate units lookup.
        let mut session = LoggerSession::new(SessionConfig {
            timeout: Duration::from_millis(self.timeout_ms),
            ..SessionConfig::default()
        });
        self.units_lookup.clear();
        for id in &self.selected_params {
            let Some(p) = resolved.find_parameter(id) else {
                self.error = Some(format!("Parameter `{id}` missing in resolved ECU"));
                return;
            };
            if let Some(m) = &p.metric {
                self.units_lookup.insert(id.clone(), m.clone());
            }
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

        // 3. CSV writer.
        let writer = match DatalogWriter::create(&self.save_path) {
            Ok(w) => w,
            Err(e) => {
                self.error = Some(format!("CSV open failed: {e}"));
                return;
            }
        };

        // 4. Channels + thread.
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_clone = Arc::clone(&stop_flag);
        let (tx, rx) = mpsc::channel::<Sample>();
        let interval = Duration::from_millis(self.interval_ms);

        let handle = std::thread::Builder::new()
            .name("romraider-logger-kline".into())
            .spawn(move || kline_worker(session, transport, writer, tx, stop_clone, interval))
            .expect("spawn k-line worker");

        self.history.clear();
        self.started_at = Some(Instant::now());
        self.error = None;
        self.worker = Some(Worker {
            stop_flag,
            sample_rx: rx,
            _handle: handle,
        });
    }

    #[cfg(feature = "ecu-tools")]
    fn start_ssm_can_worker(&mut self) {
        // 1. Резолв подписок: разделяем на raw и derived + populate units lookup.
        let mut chosen_raw: Vec<&'static SsmParam> = Vec::new();
        let mut chosen_derived: Vec<&'static SsmDerivedParam> = Vec::new();
        self.units_lookup.clear();
        for name in &self.selected_params {
            if let Some(p) = find_ssm_param(name) {
                chosen_raw.push(p);
                self.units_lookup.insert(name.clone(), p.units.into());
            } else if let Some(d) = find_derived_param(name) {
                chosen_derived.push(d);
                self.units_lookup.insert(name.clone(), d.units.into());
            } else {
                self.error = Some(format!("Unknown SSM param: {name}"));
                return;
            }
        }
        // Auto-add deps of derived (но в units lookup их кладём только если ещё нет).
        for d in &chosen_derived {
            for dep in d.depends_on {
                if !chosen_raw.iter().any(|p| p.name == *dep) {
                    if let Some(p) = find_ssm_param(dep) {
                        chosen_raw.push(p);
                        self.units_lookup
                            .entry((*dep).into())
                            .or_insert_with(|| p.units.into());
                    }
                }
            }
        }
        if chosen_raw.is_empty() {
            self.error = Some("No params subscribed.".into());
            return;
        }

        // 2. Open Tactrix CAN.
        use romraider_io::tactrix::{TactrixConfig, TactrixTransport};
        let cfg = TactrixConfig::iso15765_500k();
        let mut tr = match TactrixTransport::open(&cfg) {
            Ok(t) => t,
            Err(e) => {
                self.error = Some(format!(
                    "Tactrix CAN open failed: {e}\nLaunch GUI with sudo for USB-bulk."
                ));
                return;
            }
        };
        if let Err(e) = tr.set_can_flow_control_filter(0x7E8, 0x7E0) {
            self.error = Some(format!("CAN flow-control filter failed: {e}"));
            return;
        }

        // 3. SSM-CAN ECU init.
        let timeout = Duration::from_millis(self.timeout_ms);
        if let Err(e) = subaru::ecu_init_can(&mut tr, timeout) {
            self.error = Some(format!("SSM-CAN ECU init failed: {e}"));
            return;
        }

        // 4. CSV writer.
        let writer = match DatalogWriter::create(&self.save_path) {
            Ok(w) => w,
            Err(e) => {
                self.error = Some(format!("CSV open failed: {e}"));
                return;
            }
        };

        // 5. Channels + thread.
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_clone = Arc::clone(&stop_flag);
        let (tx, rx) = mpsc::channel::<Sample>();
        let interval = Duration::from_millis(self.interval_ms);

        let handle = std::thread::Builder::new()
            .name("romraider-logger-ssm-can".into())
            .spawn(move || {
                ssm_can_worker(
                    tr,
                    chosen_raw,
                    chosen_derived,
                    writer,
                    tx,
                    stop_clone,
                    interval,
                    timeout,
                )
            })
            .expect("spawn ssm-can worker");

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

fn kline_worker(
    session: LoggerSession,
    mut transport: SerialTransport,
    mut writer: DatalogWriter,
    tx: mpsc::Sender<Sample>,
    stop_flag: Arc<AtomicBool>,
    interval: Duration,
) {
    while !stop_flag.load(Ordering::Relaxed) {
        let started = Instant::now();
        match session.poll_once(&mut transport) {
            Ok(sample) => {
                if let Err(e) = writer.write_sample(&sample) {
                    warn!(?e, "csv write failed");
                }
                if tx.send(sample).is_err() {
                    break;
                }
            }
            Err(e) => warn!(?e, "k-line poll_once failed; continuing"),
        }
        if let Some(rem) = interval.checked_sub(started.elapsed()) {
            std::thread::sleep(rem);
        }
    }
    let _ = writer.flush();
}

#[cfg(feature = "ecu-tools")]
fn ssm_can_worker(
    mut tr: romraider_io::tactrix::TactrixTransport,
    chosen_raw: Vec<&'static SsmParam>,
    chosen_derived: Vec<&'static SsmDerivedParam>,
    mut writer: DatalogWriter,
    tx: mpsc::Sender<Sample>,
    stop_flag: Arc<AtomicBool>,
    interval: Duration,
    timeout: Duration,
) {
    use std::collections::HashMap;
    while !stop_flag.load(Ordering::Relaxed) {
        let started = Instant::now();
        match read_ssm_params_can(&mut tr, &chosen_raw, timeout) {
            Ok(per_param) => {
                let mut values = Vec::with_capacity(chosen_raw.len() + chosen_derived.len());
                let mut by_name: HashMap<&str, f64> = HashMap::new();
                for (p, raw) in chosen_raw.iter().zip(&per_param) {
                    let v = (p.scale)(raw);
                    by_name.insert(p.name, v);
                    values.push(SampleValue {
                        parameter_id: p.name.into(),
                        raw: raw.clone(),
                        value: v,
                    });
                }
                for d in &chosen_derived {
                    let inputs: Vec<f64> = d
                        .depends_on
                        .iter()
                        .map(|n| by_name.get(n).copied().unwrap_or(0.0))
                        .collect();
                    let v = (d.compute)(&inputs);
                    values.push(SampleValue {
                        parameter_id: d.name.into(),
                        raw: Vec::new(),
                        value: v,
                    });
                }
                let sample = Sample {
                    timestamp: std::time::SystemTime::now(),
                    values,
                };
                if let Err(e) = writer.write_sample(&sample) {
                    warn!(?e, "csv write failed");
                }
                if tx.send(sample).is_err() {
                    break;
                }
            }
            Err(e) => warn!(?e, "ssm-can poll failed; continuing"),
        }
        if let Some(rem) = interval.checked_sub(started.elapsed()) {
            std::thread::sleep(rem);
        }
    }
    let _ = writer.flush();
}
