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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Editor,
    Logger,
}

impl App {
    pub fn new(_cc: &CreationContext<'_>) -> Self {
        Self {
            active: Tab::Editor,
            editor: EditorPanel::default(),
            logger: LoggerPanel::default(),
            #[cfg(feature = "ecu-tools")]
            ecu_tools: EcuToolsPanel::default(),
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
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
                            self.editor.load_rom(path);
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
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
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
                        "Flash-write не реализован: для безопасной разработки\n\
                             требуется donor-ECU. Текущая стратегия проекта — read-only.",
                    );
                    ui.add_enabled(false, erase_btn).on_disabled_hover_text(
                        "Erase не реализован — см. tooltip выше про donor-ECU.",
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
    }
}
