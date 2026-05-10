use egui_plot::{Line, Plot, PlotPoints};

#[derive(Default)]
pub struct LoggerPanel {
    running: bool,
    samples: Vec<[f64; 2]>, // (t, value) — заглушка для одной серии
}

impl LoggerPanel {
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Logger");
        ui.separator();

        ui.horizontal(|ui| {
            if ui.button(if self.running { "Stop" } else { "Start" }).clicked() {
                self.running = !self.running;
                if self.running {
                    self.samples.clear();
                }
            }
            ui.label(format!("Samples: {}", self.samples.len()));
        });

        // TODO: подключить broadcast::Receiver<Sample> от LoggerSession.
        if self.running {
            let t = self.samples.len() as f64 * 0.1;
            let v = (t * 0.5).sin();
            self.samples.push([t, v]);
            ui.ctx().request_repaint();
        }

        let line = Line::new(PlotPoints::from(self.samples.clone()));
        Plot::new("logger_plot")
            .height(400.0)
            .show(ui, |plot_ui| plot_ui.line(line));
    }
}
