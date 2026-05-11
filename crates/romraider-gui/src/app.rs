use eframe::CreationContext;

use crate::panels::{editor::EditorPanel, logger::LoggerPanel};

pub struct App {
    active: Tab,
    editor: EditorPanel,
    logger: LoggerPanel,
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
                ui.separator();
                ui.selectable_value(&mut self.active, Tab::Editor, "Editor");
                ui.selectable_value(&mut self.active, Tab::Logger, "Logger");
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| match self.active {
            Tab::Editor => self.editor.ui(ui),
            Tab::Logger => self.logger.ui(ui),
        });
    }
}
