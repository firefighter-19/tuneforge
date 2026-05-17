//! Slice 26: GUI-обёртка над dump-rom-can + OBD-II peek-ecu-info.
//!
//! Состоит из двух **модальных окон** (egui-Window-ы поверх центрального
//! редактора):
//!
//! - **`Read ROM from ECU…`** — мульти-фаза:
//!   1. *Prep* — инструкции, кнопка `Start`
//!   2. *Running* — progress-bar для каждой фазы, real-time лог
//!   3. *Done* — кнопка `Save As…` + `Open in editor`
//!   4. *Error* — текст + `Close`
//!
//! - **`View ECU Info…`** — quick OBD-II опрос (VIN, CVN, ROM-ID,
//!   supported-PIDs-bitmap). Run → Done одной кнопкой.
//!
//! Работает только под feature `ecu-tools` (тянет `romraider-kernel`,
//! GPL-3.0). По умолчанию GUI остаётся под GPL-2.0+.
//!
//! USB-доступ требует **`sudo`** на macOS — Tactrix через libusb hard-claim.
//! Если запуск без root → `Error` с prominent инструкцией.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::Duration;

use romraider_kernel::orchestrator::{
    dump_rom_via_can, peek_ecu_info, DumpProgress, EcuInfo,
};

/// Маркеры progress для UI (производное от [`DumpProgress`]).
#[derive(Default, Debug, Clone)]
struct DumpUiState {
    obdii_done:        bool,
    sec_access_done:   bool,
    kernel_upload_pct: f32,
    dump_done:         usize,
    dump_total:        usize,
    dump_rate_bps:     f64,
    vin:               Option<String>,
    cvn:               Option<String>,
    rom_id_hex:        Option<String>,
    log_lines:         Vec<String>,
}

impl DumpUiState {
    fn apply(&mut self, ev: &DumpProgress) {
        match ev {
            DumpProgress::PhaseAObdiiWake { .. } => {
                self.log_lines.push("Phase A: OBD-II Mode 01 PID 00 ok".into());
            }
            DumpProgress::PhaseAVin { vin_raw } => {
                // Reply: `49 02 <01> <17 ASCII>` — strip 3-байт префикс.
                if vin_raw.len() >= 20 {
                    self.vin = Some(
                        vin_raw[3..20]
                            .iter()
                            .map(|&b| if (0x20..0x7F).contains(&b) { b as char } else { '.' })
                            .collect(),
                    );
                }
                self.log_lines.push(format!(
                    "Phase A: VIN = {}",
                    self.vin.as_deref().unwrap_or("?")
                ));
            }
            DumpProgress::PhaseACvn { cvn_raw } => {
                if cvn_raw.len() >= 7 {
                    self.cvn = Some(format!(
                        "{:02X} {:02X} {:02X} {:02X}",
                        cvn_raw[3], cvn_raw[4], cvn_raw[5], cvn_raw[6]
                    ));
                }
                self.log_lines.push(format!(
                    "Phase A: CVN = {}",
                    self.cvn.as_deref().unwrap_or("?")
                ));
                self.obdii_done = true;
            }
            DumpProgress::PhaseBExtSession => {
                self.log_lines.push("Phase B: ExtendedDiagSession ✓".into());
            }
            DumpProgress::PhaseBSeed { seed } => {
                self.log_lines.push(format!(
                    "Phase B: seed {:02X} {:02X} {:02X} {:02X}",
                    seed[0], seed[1], seed[2], seed[3]
                ));
            }
            DumpProgress::PhaseBKey { key } => {
                self.log_lines.push(format!(
                    "Phase B: key {:02X} {:02X} {:02X} {:02X}",
                    key[0], key[1], key[2], key[3]
                ));
            }
            DumpProgress::PhaseBGranted => {
                self.log_lines.push("Phase B: SecurityAccess granted ✓".into());
                self.sec_access_done = true;
            }
            DumpProgress::PhaseBProgSession => {
                self.log_lines.push("Phase B: ProgrammingSession ✓".into());
            }
            DumpProgress::PhaseCUploadStart { total_bytes } => {
                self.log_lines
                    .push(format!("Phase C: uploading kernel ({total_bytes} bytes)…"));
            }
            DumpProgress::PhaseCUploadDone { elapsed_secs } => {
                self.kernel_upload_pct = 1.0;
                self.log_lines
                    .push(format!("Phase C: kernel uploaded in {elapsed_secs:.2}s"));
            }
            DumpProgress::PhaseDBanner { banner } => {
                self.log_lines.push(format!("Phase D: kernel banner = {banner:?}"));
            }
            DumpProgress::PhaseEReadStart { total, .. } => {
                self.dump_total = *total;
                self.log_lines.push(format!("Phase E: reading {total} bytes…"));
            }
            DumpProgress::PhaseEReadProgress {
                done,
                total,
                rate_bps,
            } => {
                self.dump_done = *done;
                self.dump_total = *total;
                self.dump_rate_bps = *rate_bps;
            }
            DumpProgress::PhaseEDone {
                bytes_dumped,
                elapsed_secs,
            } => {
                self.log_lines.push(format!(
                    "Phase E: {bytes_dumped} bytes in {elapsed_secs:.1}s ✓"
                ));
            }
        }
    }

    fn dump_pct(&self) -> f32 {
        if self.dump_total == 0 {
            0.0
        } else {
            self.dump_done as f32 / self.dump_total as f32
        }
    }
}

enum WorkerEvent {
    Info(EcuInfo, [u8; 5]),
    Progress(DumpProgress),
    Dumped(Vec<u8>),
    Failed(String),
}

struct Worker {
    rx:     mpsc::Receiver<WorkerEvent>,
    handle: Option<JoinHandle<()>>,
    cancel: Arc<AtomicBool>,
}

impl Worker {
    fn poll(&mut self) -> Vec<WorkerEvent> {
        let mut events = Vec::new();
        while let Ok(ev) = self.rx.try_recv() {
            events.push(ev);
        }
        events
    }

    fn shutdown(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

#[derive(Default)]
enum ReadRomState {
    #[default]
    Prep,
    Running(DumpUiState),
    Done {
        bytes:    Vec<u8>,
        ui_state: DumpUiState,
    },
    Error(String),
}

#[derive(Default)]
enum ViewInfoState {
    #[default]
    Prep,
    Running,
    Done {
        info:       EcuInfo,
        rom_id_hex: String,
    },
    Error(String),
}

pub struct EcuToolsPanel {
    pub show_read_rom:  bool,
    pub show_view_info: bool,
    read_rom_state:     ReadRomState,
    view_info_state:    ViewInfoState,
    worker:             Option<Worker>,
}

impl Default for EcuToolsPanel {
    fn default() -> Self {
        Self {
            show_read_rom:   false,
            show_view_info:  false,
            read_rom_state:  ReadRomState::Prep,
            view_info_state: ViewInfoState::Prep,
            worker:          None,
        }
    }
}

impl EcuToolsPanel {
    /// Открыть окно «Read ROM» (reset state).
    pub fn open_read_rom(&mut self) {
        self.show_read_rom = true;
        self.read_rom_state = ReadRomState::Prep;
    }

    /// Открыть окно «View ECU Info» (reset state).
    pub fn open_view_info(&mut self) {
        self.show_view_info = true;
        self.view_info_state = ViewInfoState::Prep;
    }

    /// Главный entry — называется из `App::update`. Рисует оба окна если
    /// открыты, drain-ит worker-events каждый frame.
    pub fn ui(&mut self, ctx: &egui::Context) {
        // Drain worker events first
        if let Some(w) = self.worker.as_mut() {
            let events = w.poll();
            for ev in events {
                self.apply_worker_event(ev);
            }
        }

        // Если рабочий thread есть — requested repaint каждые ~100мс
        if self.worker.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        self.render_view_info_window(ctx);
        self.render_read_rom_window(ctx);
    }

    fn apply_worker_event(&mut self, ev: WorkerEvent) {
        match ev {
            WorkerEvent::Info(info, rom_id) => {
                self.view_info_state = ViewInfoState::Done {
                    info,
                    rom_id_hex: format!(
                        "{:02X} {:02X} {:02X} {:02X} {:02X}",
                        rom_id[0], rom_id[1], rom_id[2], rom_id[3], rom_id[4]
                    ),
                };
                self.worker = None;
            }
            WorkerEvent::Progress(dp) => {
                if let ReadRomState::Running(ui) = &mut self.read_rom_state {
                    ui.apply(&dp);
                }
            }
            WorkerEvent::Dumped(bytes) => {
                let prev_ui = match std::mem::take(&mut self.read_rom_state) {
                    ReadRomState::Running(ui) => ui,
                    _ => DumpUiState::default(),
                };
                self.read_rom_state = ReadRomState::Done {
                    bytes,
                    ui_state: prev_ui,
                };
                self.worker = None;
            }
            WorkerEvent::Failed(msg) => {
                // Кладём ошибку в активное окно
                if matches!(self.read_rom_state, ReadRomState::Running(_)) {
                    self.read_rom_state = ReadRomState::Error(msg);
                } else if matches!(self.view_info_state, ViewInfoState::Running) {
                    self.view_info_state = ViewInfoState::Error(msg);
                }
                self.worker = None;
            }
        }
    }

    fn render_view_info_window(&mut self, ctx: &egui::Context) {
        if !self.show_view_info {
            return;
        }
        let mut open = self.show_view_info;
        egui::Window::new("View ECU Info")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| match &self.view_info_state {
                ViewInfoState::Prep => {
                    ui.label("Опросить ECU через OBD-II (Mode 01/09).");
                    ui.label("• Tactrix Openport 2.0 подключён к OBD-II");
                    ui.label("• Зажигание ON (мотор не обязательно)");
                    ui.add_space(8.0);
                    if ui.button("▶ Start").clicked() {
                        self.start_view_info_worker();
                    }
                }
                ViewInfoState::Running => {
                    ui.spinner();
                    ui.label("Опрос ECU…");
                }
                ViewInfoState::Done { info, rom_id_hex } => {
                    ui.heading("ECU Info");
                    ui.separator();
                    ui.label(format!(
                        "VIN:    {}",
                        info.vin.as_deref().unwrap_or("(не отдан)")
                    ));
                    ui.label(format!(
                        "CVN:    {}",
                        info.cvn
                            .map(|c| format!("{:02X} {:02X} {:02X} {:02X}", c[0], c[1], c[2], c[3]))
                            .unwrap_or_else(|| "(не отдан)".into())
                    ));
                    ui.label(format!("ROM ID: {rom_id_hex}"));
                    ui.label(format!(
                        "Supported PIDs (01-20): 0x{:08X} ({} PIDs)",
                        info.mode01_supported_bitmap,
                        info.mode01_supported_bitmap.count_ones()
                    ));
                }
                ViewInfoState::Error(msg) => {
                    ui.colored_label(egui::Color32::RED, "Ошибка:");
                    ui.label(msg);
                    if msg.contains("Access") || msg.contains("denied") || msg.contains("permission") {
                        ui.add_space(6.0);
                        ui.colored_label(
                            egui::Color32::YELLOW,
                            "Tactrix требует root для USB-bulk. Запусти GUI с sudo:",
                        );
                        ui.code("sudo cargo run -p romraider-gui --features ecu-tools");
                    }
                }
            });
        self.show_view_info = open;
        if !self.show_view_info {
            if let Some(w) = self.worker.as_mut() {
                w.shutdown();
            }
            self.worker = None;
        }
    }

    fn render_read_rom_window(&mut self, ctx: &egui::Context) {
        if !self.show_read_rom {
            return;
        }
        let mut open = self.show_read_rom;
        egui::Window::new("Read ROM from ECU")
            .open(&mut open)
            .resizable(true)
            .default_width(520.0)
            .collapsible(false)
            .show(ctx, |ui| self.render_read_rom_content(ui));
        self.show_read_rom = open;
        if !self.show_read_rom {
            if let Some(w) = self.worker.as_mut() {
                w.shutdown();
            }
            self.worker = None;
        }
    }

    fn render_read_rom_content(&mut self, ui: &mut egui::Ui) {
        // Note: `take` чтобы избежать двойного borrow при переходах состояний.
        let state = std::mem::take(&mut self.read_rom_state);
        let next = match state {
            ReadRomState::Prep => self.render_prep(ui),
            ReadRomState::Running(ui_state) => self.render_running(ui, ui_state),
            ReadRomState::Done { bytes, ui_state } => self.render_done(ui, bytes, ui_state),
            ReadRomState::Error(msg) => self.render_error(ui, msg),
        };
        self.read_rom_state = next;
    }

    fn render_prep(&mut self, ui: &mut egui::Ui) -> ReadRomState {
        ui.heading("Read ROM via UDS-over-CAN");
        ui.separator();
        ui.label("Перед стартом проверь:");
        ui.label("  1. Tactrix Openport 2.0 подключён к OBD-II");
        ui.label("  2. Зажигание в положении ON (мотор НЕ заводить)");
        ui.label("  3. Машина не движется, нагрузка минимальная");
        ui.label("  4. GUI запущен с sudo (USB-bulk требует root на macOS)");
        ui.add_space(6.0);
        ui.label("Процесс займёт ~45 секунд. Во время дампа:");
        ui.label("  • ECU перейдёт в programming-mode (двигатель завести нельзя)");
        ui.label("  • После окончания — выключи зажигание, подожди 10с, снова ON");
        ui.add_space(10.0);
        if ui.button("▶ Start").clicked() {
            self.start_read_rom_worker();
            return ReadRomState::Running(DumpUiState::default());
        }
        ReadRomState::Prep
    }

    fn render_running(&mut self, ui: &mut egui::Ui, mut state: DumpUiState) -> ReadRomState {
        ui.heading("Dumping…");
        ui.separator();

        // Phase indicators
        ui.label(format!(
            "Phase A (OBD-II identify): {}",
            if state.obdii_done { "✓" } else { "running…" }
        ));
        ui.label(format!(
            "Phase B (SecurityAccess): {}",
            if state.sec_access_done {
                "✓"
            } else if state.obdii_done {
                "running…"
            } else {
                "—"
            }
        ));

        ui.label(format!(
            "Phase C (kernel upload): {}%",
            (state.kernel_upload_pct * 100.0) as i32
        ));
        if state.sec_access_done {
            ui.add(
                egui::ProgressBar::new(state.kernel_upload_pct)
                    .show_percentage()
                    .animate(state.kernel_upload_pct < 1.0),
            );
        }

        let pct = state.dump_pct();
        ui.label(format!(
            "Phase E (ROM dump): {}/{} bytes — {:.0} B/s",
            state.dump_done, state.dump_total, state.dump_rate_bps
        ));
        if state.dump_total > 0 {
            ui.add(
                egui::ProgressBar::new(pct)
                    .show_percentage()
                    .animate(pct < 1.0),
            );
        }

        ui.add_space(8.0);
        ui.collapsing("Log", |ui| {
            egui::ScrollArea::vertical()
                .max_height(180.0)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for line in &state.log_lines {
                        ui.monospace(line);
                    }
                });
        });

        if state.log_lines.len() > 500 {
            // bound memory if dump runs long
            state.log_lines.drain(..state.log_lines.len() - 500);
        }
        ReadRomState::Running(state)
    }

    fn render_done(
        &mut self,
        ui: &mut egui::Ui,
        bytes: Vec<u8>,
        ui_state: DumpUiState,
    ) -> ReadRomState {
        ui.heading("✅ Done");
        ui.separator();
        ui.label(format!("Dumped {} bytes from ECU.", bytes.len()));
        if let Some(vin) = &ui_state.vin {
            ui.label(format!("VIN: {vin}"));
        }
        if let Some(cvn) = &ui_state.cvn {
            ui.label(format!("CVN: {cvn}"));
        }
        ui.add_space(10.0);
        ui.label("⚠️  Не забудь:");
        ui.label("  • Выключить зажигание");
        ui.label("  • Подождать 10 секунд (ECU outputs reset)");
        ui.label("  • Снова повернуть в ON чтобы выйти из programming-mode");
        ui.add_space(8.0);
        if ui.button("💾 Save As…").clicked() {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("ROM image", &["bin", "rom"])
                .set_file_name("ecu-dump.bin")
                .save_file()
            {
                if let Err(e) = std::fs::write(&path, &bytes) {
                    return ReadRomState::Error(format!("Save failed: {e}"));
                }
                tracing::info!(
                    bytes = bytes.len(),
                    path = %path.display(),
                    "ECU dump saved"
                );
                return ReadRomState::Done { bytes, ui_state };
            }
        }
        if ui.button("Close").clicked() {
            self.show_read_rom = false;
            return ReadRomState::Prep;
        }
        ReadRomState::Done { bytes, ui_state }
    }

    fn render_error(&mut self, ui: &mut egui::Ui, msg: String) -> ReadRomState {
        ui.heading("❌ Failed");
        ui.separator();
        ui.colored_label(egui::Color32::RED, &msg);

        let is_permission =
            msg.contains("Access") || msg.contains("permission") || msg.contains("denied");
        if is_permission {
            ui.add_space(6.0);
            ui.colored_label(
                egui::Color32::YELLOW,
                "USB-bulk доступ к Tactrix требует root на macOS.",
            );
            ui.label("Запусти GUI так:");
            ui.code("sudo cargo run -p romraider-gui --features ecu-tools");
        }

        ui.add_space(10.0);
        if ui.button("Retry").clicked() {
            return ReadRomState::Prep;
        }
        if ui.button("Close").clicked() {
            self.show_read_rom = false;
            return ReadRomState::Prep;
        }
        ReadRomState::Error(msg)
    }

    fn start_view_info_worker(&mut self) {
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_clone = Arc::clone(&cancel);
        let (tx, rx) = mpsc::channel::<WorkerEvent>();
        let handle = std::thread::Builder::new()
            .name("ecu-view-info-worker".into())
            .spawn(move || view_info_worker(tx, cancel_clone))
            .expect("spawn view-info worker");
        self.worker = Some(Worker {
            rx,
            handle: Some(handle),
            cancel,
        });
        self.view_info_state = ViewInfoState::Running;
    }

    fn start_read_rom_worker(&mut self) {
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_clone = Arc::clone(&cancel);
        let (tx, rx) = mpsc::channel::<WorkerEvent>();
        let handle = std::thread::Builder::new()
            .name("ecu-read-rom-worker".into())
            .spawn(move || read_rom_worker(tx, cancel_clone))
            .expect("spawn read-rom worker");
        self.worker = Some(Worker {
            rx,
            handle: Some(handle),
            cancel,
        });
    }
}

fn view_info_worker(tx: mpsc::Sender<WorkerEvent>, _cancel: Arc<AtomicBool>) {
    let timeout = Duration::from_millis(3000);
    let mut tr = match open_tactrix_can() {
        Ok(t) => t,
        Err(e) => {
            let _ = tx.send(WorkerEvent::Failed(format!("Tactrix open: {e}")));
            return;
        }
    };
    let info = match peek_ecu_info(&mut tr, timeout) {
        Ok(i) => i,
        Err(e) => {
            let _ = tx.send(WorkerEvent::Failed(format!("peek_ecu_info: {e}")));
            return;
        }
    };
    // ROM-ID получим попозже — для MVP пустой
    let _ = tx.send(WorkerEvent::Info(info, [0; 5]));
}

fn read_rom_worker(tx: mpsc::Sender<WorkerEvent>, _cancel: Arc<AtomicBool>) {
    let timeout = Duration::from_millis(5000);
    let mut tr = match open_tactrix_can() {
        Ok(t) => t,
        Err(e) => {
            let _ = tx.send(WorkerEvent::Failed(format!("Tactrix open: {e}")));
            return;
        }
    };
    let tx_for_cb = tx.clone();
    let result = dump_rom_via_can(
        &mut tr,
        0x000000,
        1024 * 1024, // SH7058 full 1 MiB
        2048,
        timeout,
        move |ev| {
            let _ = tx_for_cb.send(WorkerEvent::Progress(ev));
        },
    );
    match result {
        Ok(bytes) => {
            let _ = tx.send(WorkerEvent::Dumped(bytes));
        }
        Err(e) => {
            let _ = tx.send(WorkerEvent::Failed(format!("Dump failed: {e}")));
        }
    }
}

fn open_tactrix_can() -> anyhow::Result<romraider_io::tactrix::TactrixTransport> {
    use romraider_io::tactrix::{TactrixConfig, TactrixTransport};
    let cfg = TactrixConfig::iso15765_500k();
    let mut tr = TactrixTransport::open(&cfg)?;
    tr.set_can_flow_control_filter(0x7E8, 0x7E0)?;
    Ok(tr)
}
