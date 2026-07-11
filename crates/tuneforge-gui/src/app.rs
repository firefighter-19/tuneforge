use std::path::PathBuf;

use eframe::CreationContext;

#[cfg(feature = "ecu-tools")]
use crate::panels::ecu_tools::EcuToolsPanel;
use crate::panels::{editor::EditorPanel, logger::LoggerPanel};

pub struct App {
    active: Tab,
    editor: EditorPanel,
    logger: LoggerPanel,
    #[cfg(feature = "ecu-tools")]
    ecu_tools: EcuToolsPanel,
    /// Отложенное разрушающее действие при несохранённых правках.
    guard: UnsavedGuard,
    /// Разрешить фактическое закрытие окна (после подтверждения Quit).
    allow_close: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Editor,
    Logger,
}

/// Действие, отложенное до разрешения «unsaved changes».
#[derive(Clone, PartialEq, Eq, Debug)]
enum PendingAction {
    Quit,
    OpenRom(PathBuf),
}

/// Guard против потери несохранённых правок: если ROM «грязный», разрушающее
/// действие откладывается до подтверждения (Save / Discard / Cancel).
#[derive(Default)]
struct UnsavedGuard {
    pending: Option<PendingAction>,
}

impl UnsavedGuard {
    /// Запросить `action`. `true` — можно выполнять сразу (ROM не изменён);
    /// `false` — действие отложено, нужно показать модалку подтверждения.
    fn guard(&mut self, action: PendingAction, dirty: bool) -> bool {
        if dirty {
            self.pending = Some(action);
            false
        } else {
            true
        }
    }

    fn is_active(&self) -> bool {
        self.pending.is_some()
    }

    /// Забрать отложенное действие (после Save/Discard) — выполнить его.
    fn take(&mut self) -> Option<PendingAction> {
        self.pending.take()
    }

    /// Отменить (Cancel).
    fn cancel(&mut self) {
        self.pending = None;
    }
}

impl App {
    pub fn new(_cc: &CreationContext<'_>) -> Self {
        Self {
            active: Tab::Editor,
            editor: EditorPanel::default(),
            logger: LoggerPanel::default(),
            #[cfg(feature = "ecu-tools")]
            ecu_tools: EcuToolsPanel::default(),
            guard: UnsavedGuard::default(),
            allow_close: false,
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Перехват закрытия окна (крестик) при несохранённых правках: отменяем
        // закрытие и показываем модалку Save/Discard/Cancel.
        if ctx.input(|i| i.viewport().close_requested())
            && !self.allow_close
            && self.editor.is_dirty()
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.guard.guard(PendingAction::Quit, true);
        }

        egui::TopBottomPanel::top("topbar").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Open ROM…").clicked() {
                        ui.close_menu();
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("ROM image", &["bin", "rom", "hex"])
                            .add_filter("Any", &["*"])
                            .pick_file()
                        {
                            if self
                                .guard
                                .guard(PendingAction::OpenRom(path.clone()), self.editor.is_dirty())
                            {
                                self.editor.load_rom(path);
                            }
                        }
                    }
                    if ui.button("Open Def…").clicked() {
                        ui.close_menu();
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("ECU definitions XML", &["xml"])
                            .pick_file()
                        {
                            self.editor.load_def(path);
                        }
                    }
                    ui.separator();
                    if ui.button("Open Compare ROM…").clicked() {
                        ui.close_menu();
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("ROM image", &["bin", "rom", "hex"])
                            .add_filter("Any", &["*"])
                            .pick_file()
                        {
                            self.editor.load_compare_rom(path);
                        }
                    }
                    if ui.button("Close Compare ROM").clicked() {
                        ui.close_menu();
                        self.editor.clear_compare_rom();
                    }
                    ui.separator();
                    if ui
                        .add_enabled(
                            self.editor.has_rom(),
                            egui::Button::new("Save ROM  (Ctrl+S)"),
                        )
                        .clicked()
                    {
                        ui.close_menu();
                        self.editor.save_rom();
                    }
                    if ui.button("Save ROM As…").clicked() {
                        ui.close_menu();
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("ROM image", &["bin", "rom"])
                            .save_file()
                        {
                            self.editor.save_rom_as(path);
                        }
                    }
                    ui.separator();
                    if ui.button("Quit").clicked() {
                        ui.close_menu();
                        if self
                            .guard
                            .guard(PendingAction::Quit, self.editor.is_dirty())
                        {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    }
                });
                ui.menu_button("Edit", |ui| {
                    let can_undo = self.editor.can_undo();
                    let can_redo = self.editor.can_redo();
                    if ui
                        .add_enabled(can_undo, egui::Button::new("Undo  (Ctrl+Z)"))
                        .clicked()
                    {
                        ui.close_menu();
                        self.editor.undo_action();
                    }
                    if ui
                        .add_enabled(can_redo, egui::Button::new("Redo  (Ctrl+Y)"))
                        .clicked()
                    {
                        ui.close_menu();
                        self.editor.redo_action();
                    }
                });
                #[cfg(feature = "ecu-tools")]
                ui.menu_button("ECU", |ui| {
                    if ui.button("Read ROM from ECU…").clicked() {
                        ui.close_menu();
                        self.ecu_tools.open_read_rom();
                    }
                    if ui.button("View ECU Info…").clicked() {
                        ui.close_menu();
                        self.ecu_tools.open_view_info();
                    }
                    if ui.button("Read DTCs…").clicked() {
                        ui.close_menu();
                        self.ecu_tools.open_dtc();
                    }
                    if ui.button("Read Freeze Frame…").clicked() {
                        ui.close_menu();
                        self.ecu_tools.open_freeze();
                    }
                    ui.separator();
                    // Placeholder-пункты для flash-операций, disabled
                    // намеренно (нет donor-ECU = нет safe тестов).
                    let write_btn = egui::Button::new("Write ROM to ECU…");
                    let erase_btn = egui::Button::new("Erase ROM");
                    ui.add_enabled(false, write_btn).on_disabled_hover_text(
                        "Flash-write is not implemented: safe development requires\n\
                             a donor ECU. Project strategy is read-only for now.",
                    );
                    ui.add_enabled(false, erase_btn).on_disabled_hover_text(
                        "Erase is not implemented — see tooltip above about donor ECU.",
                    );
                });
                ui.separator();
                ui.selectable_value(&mut self.active, Tab::Editor, "Editor");
                ui.selectable_value(&mut self.active, Tab::Logger, "Logger");
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| match self.active {
            Tab::Editor => self.editor.ui(ui),
            Tab::Logger => self.logger.ui(ui),
        });

        #[cfg(feature = "ecu-tools")]
        self.ecu_tools.ui(ctx);

        self.render_unsaved_modal(ctx);
    }
}

impl App {
    /// Модалка «Unsaved changes» — показывается, когда guard держит отложенное
    /// действие. Save сохраняет на месте и продолжает; Discard продолжает без
    /// сохранения; Cancel — остаётся в редакторе.
    fn render_unsaved_modal(&mut self, ctx: &egui::Context) {
        if !self.guard.is_active() {
            return;
        }
        enum Choice {
            Save,
            Discard,
            Cancel,
        }
        let mut choice = None;
        egui::Window::new("Unsaved changes")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label("The ROM has unsaved changes. Save before continuing?");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        choice = Some(Choice::Save);
                    }
                    if ui.button("Discard").clicked() {
                        choice = Some(Choice::Discard);
                    }
                    if ui.button("Cancel").clicked() {
                        choice = Some(Choice::Cancel);
                    }
                });
            });
        match choice {
            Some(Choice::Save) => {
                self.editor.save_rom();
                if let Some(action) = self.guard.take() {
                    self.execute_pending(ctx, action);
                }
            }
            Some(Choice::Discard) => {
                if let Some(action) = self.guard.take() {
                    self.execute_pending(ctx, action);
                }
            }
            Some(Choice::Cancel) => self.guard.cancel(),
            None => {}
        }
    }

    fn execute_pending(&mut self, ctx: &egui::Context, action: PendingAction) {
        match action {
            PendingAction::Quit => {
                self.allow_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            PendingAction::OpenRom(path) => self.editor.load_rom(path),
        }
    }
}

#[cfg(test)]
mod guard_tests {
    use super::{PendingAction, UnsavedGuard};
    use std::path::PathBuf;

    #[test]
    fn clean_rom_proceeds_without_confirmation() {
        let mut g = UnsavedGuard::default();
        assert!(
            g.guard(PendingAction::Quit, false),
            "not dirty → proceed now"
        );
        assert!(!g.is_active());
    }

    #[test]
    fn dirty_rom_stashes_action_and_requires_confirmation() {
        let mut g = UnsavedGuard::default();
        assert!(!g.guard(PendingAction::Quit, true), "dirty → needs confirm");
        assert!(g.is_active());
        assert_eq!(g.take(), Some(PendingAction::Quit));
        assert!(!g.is_active(), "take() consumes the pending action");
    }

    #[test]
    fn cancel_clears_pending_action() {
        let mut g = UnsavedGuard::default();
        g.guard(PendingAction::OpenRom(PathBuf::from("/tmp/x.bin")), true);
        assert!(g.is_active());
        g.cancel();
        assert!(!g.is_active());
        assert_eq!(g.take(), None);
    }

    #[test]
    fn open_rom_action_round_trips_the_path() {
        let mut g = UnsavedGuard::default();
        let p = PathBuf::from("/tmp/tune.bin");
        assert!(!g.guard(PendingAction::OpenRom(p.clone()), true));
        assert_eq!(g.take(), Some(PendingAction::OpenRom(p)));
    }
}
