use std::path::PathBuf;

use romraider_rom::RomImage;
use tracing::warn;

#[derive(Default)]
pub struct EditorPanel {
    rom:  Option<(PathBuf, RomImage)>,
    open_requested: bool,
    error: Option<String>,
}

impl EditorPanel {
    pub fn request_open(&mut self) {
        self.open_requested = true;
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        if self.open_requested {
            self.open_requested = false;
            // TODO: подключить rfd::FileDialog. Пока — placeholder, чтобы не тянуть
            // лишнюю крейт-зависимость до того, как UI реально нужен.
            warn!("file picker not wired yet");
            self.error = Some("File picker is not implemented in the skeleton".into());
        }

        ui.heading("ROM Editor");
        ui.separator();

        match &self.rom {
            Some((path, rom)) => {
                ui.label(format!("File:  {}", path.display()));
                ui.label(format!("Size:  {} KiB", rom.size() / 1024));
                ui.label(format!("Dirty: {}", rom.is_dirty()));
            }
            None => {
                ui.label("No ROM loaded. Use File → Open ROM…");
            }
        }

        if let Some(err) = &self.error {
            ui.colored_label(egui::Color32::LIGHT_RED, err);
        }
    }
}
