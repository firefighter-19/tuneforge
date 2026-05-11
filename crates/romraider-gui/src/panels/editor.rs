//! ROM editor panel — read-only MVP.
//!
//! Workflow:
//! 1. File → Open ROM…       (через rfd)
//! 2. File → Open Def…       (XML с `<roms>`)
//! 3. Левая колонка — picker ROM-ID + дерево таблиц по `category`.
//! 4. Центр — сетка значений выбранной таблицы со scaling и осями (для 3D).

use std::collections::BTreeMap;
use std::path::PathBuf;

use romraider_core::Endian;
use romraider_defs::{
    parse_file, resolve, CompiledScaling, ResolvedRom, ResolvedTable, StorageType, TableKind,
};
use romraider_rom::{subaru_classic, RomImage};
use tracing::{info, warn};

#[derive(Default)]
pub struct EditorPanel {
    rom:                 Option<RomState>,
    /// Опциональный второй ROM (base) для compare-режима. Read-only.
    compare_rom:         Option<RomState>,
    def:                 Option<DefState>,
    selected_rom_id:     Option<String>,
    selected_table_name: Option<String>,
    error:               Option<String>,
    /// Кратковременное сообщение в статусной строке (например, «Saved to …»).
    notice:              Option<String>,
    /// Что показывать в ячейках, когда есть `compare_rom`.
    display_mode:        DisplayMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DisplayMode {
    /// Показывать текущие значения; раскраска фона по diff vs base.
    #[default]
    Values,
    /// Показывать `current - base`; read-only.
    Diff,
}

#[derive(Debug, Clone, Copy)]
struct ChecksumSummary {
    valid: usize,
    total: usize,
}

impl ChecksumSummary {
    fn all_valid(self) -> bool {
        self.valid == self.total
    }
}

struct RomState {
    path: PathBuf,
    rom:  RomImage,
}

struct DefState {
    path: PathBuf,
    roms: Vec<ResolvedRom>,
}

impl EditorPanel {
    pub fn load_rom(&mut self, path: PathBuf) {
        match RomImage::open(&path) {
            Ok(rom) => {
                self.rom    = Some(RomState { path, rom });
                self.error  = None;
                self.notice = None;
            }
            Err(e) => {
                warn!(?e, "rom open failed");
                self.error = Some(format!("Failed to open ROM: {e}"));
            }
        }
    }

    pub fn load_compare_rom(&mut self, path: PathBuf) {
        match RomImage::open(&path) {
            Ok(rom) => {
                self.compare_rom = Some(RomState { path, rom });
                self.error       = None;
                self.notice      = None;
            }
            Err(e) => {
                warn!(?e, "compare rom open failed");
                self.error = Some(format!("Failed to open compare ROM: {e}"));
            }
        }
    }

    pub fn clear_compare_rom(&mut self) {
        self.compare_rom = None;
        // Сбрасываем режим, чтобы Diff не висел без base.
        self.display_mode = DisplayMode::Values;
    }

    pub fn load_def(&mut self, path: PathBuf) {
        let result = parse_file(&path).and_then(|doc| resolve(&doc));
        match result {
            Ok(roms) => {
                self.def                 = Some(DefState { path, roms });
                self.selected_rom_id     = None;
                self.selected_table_name = None;
                self.error               = None;
                self.notice              = None;
            }
            Err(e) => {
                warn!(?e, "def load failed");
                self.error = Some(format!("Failed to load definitions: {e}"));
            }
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        self.render_status(ui);

        egui::SidePanel::left("editor-sidebar")
            .min_width(220.0)
            .resizable(true)
            .show_inside(ui, |ui| self.render_sidebar(ui));

        egui::CentralPanel::default().show_inside(ui, |ui| self.render_content(ui));
    }

    pub fn save_rom_as(&mut self, path: PathBuf) {
        let Some(state) = self.rom.as_mut() else {
            self.error = Some("No ROM loaded.".into());
            return;
        };

        // Авто-пересчёт Subaru-classic checksum, если у выбранного ROM-а в def
        // есть «checksum fix»-таблицы. Без этого ECU отвергнет файл.
        let mut fixed_msg = String::new();
        if let (Some(def), Some(rom_id)) = (&self.def, &self.selected_rom_id) {
            if let Some(rom_def) = def.roms.iter().find(|r| &r.xml_id == rom_id) {
                match subaru_classic::fix(&mut state.rom, rom_def) {
                    Ok(0) => {}                                                                  // нет таблиц или нечего обновлять
                    Ok(n) => fixed_msg = format!(" (auto-fixed {n} checksum entries)"),
                    Err(e) => {
                        warn!(?e, "checksum fix failed");
                        self.error = Some(format!("Checksum auto-fix failed: {e}"));
                        return;
                    }
                }
            }
        }

        match state.rom.save_as(&path) {
            Ok(()) => {
                info!(?path, "ROM saved");
                let display = path.display().to_string();
                state.path = path;
                self.error = None;
                self.notice = Some(format!("Saved to {display}{fixed_msg}"));
            }
            Err(e) => {
                warn!(?e, "save_as failed");
                self.error = Some(format!("Save failed: {e}"));
            }
        }
    }

    /// Вычислить сводку по checksum-fix-таблицам выбранного ROM. `None`, если
    /// невозможно (нет ROM/def/выбранного rom_id) или в def нет таких таблиц.
    fn checksum_summary(&self) -> Option<ChecksumSummary> {
        let rom    = self.rom.as_ref()?;
        let def    = self.def.as_ref()?;
        let rom_id = self.selected_rom_id.as_deref()?;
        let rom_def = def.roms.iter().find(|r| r.xml_id == rom_id)?;

        let results = subaru_classic::verify(&rom.rom, rom_def).ok()?;
        if results.is_empty() {
            return None;
        }
        let valid = results.iter().filter(|r| r.valid).count();
        Some(ChecksumSummary { valid, total: results.len() })
    }

    /// Пересчитать checksum-fix-таблицы прямо сейчас, не сохраняя файл.
    fn fix_checksums_now(&mut self) {
        let Some(state)  = self.rom.as_mut() else { return };
        let Some(def)    = self.def.as_ref() else { return };
        let Some(rom_id) = self.selected_rom_id.as_deref() else { return };
        let Some(rom_def) = def.roms.iter().find(|r| r.xml_id == rom_id) else { return };

        match subaru_classic::fix(&mut state.rom, rom_def) {
            Ok(n) => {
                self.error  = None;
                self.notice = Some(format!("Fixed {n} checksum entries (not yet saved)"));
            }
            Err(e) => {
                warn!(?e, "manual checksum fix failed");
                self.error = Some(format!("Checksum fix failed: {e}"));
            }
        }
    }

    fn render_status(&mut self, ui: &mut egui::Ui) {
        let summary = self.checksum_summary();
        let mut fix_clicked    = false;
        let mut clear_compare  = false;

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("ROM:").strong());
            match &self.rom {
                Some(r) => {
                    ui.label(r.path.display().to_string());
                    if r.rom.is_dirty() {
                        ui.colored_label(egui::Color32::YELLOW, "● modified");
                    }
                }
                None => {
                    ui.label("(not loaded)");
                }
            }
            ui.separator();
            ui.label(egui::RichText::new("Def:").strong());
            ui.label(
                self.def
                    .as_ref()
                    .map(|d| d.path.display().to_string())
                    .unwrap_or_else(|| "(not loaded)".into()),
            );
            if let Some(s) = summary {
                ui.separator();
                if s.all_valid() {
                    ui.colored_label(
                        egui::Color32::LIGHT_GREEN,
                        format!("Checksums: {}/{} ✓", s.valid, s.total),
                    );
                } else {
                    ui.colored_label(
                        egui::Color32::LIGHT_RED,
                        format!("Checksums: {}/{} ✗", s.valid, s.total),
                    );
                    if ui.button("Fix now").clicked() {
                        fix_clicked = true;
                    }
                }
            }
        });

        // Вторая строка: compare-ROM + display-mode toggle.
        if let Some(c) = &self.compare_rom {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Base:").strong());
                ui.label(c.path.display().to_string());
                if ui.small_button("✕").on_hover_text("Close compare ROM").clicked() {
                    clear_compare = true;
                }
                ui.separator();
                ui.label("Show:");
                ui.selectable_value(&mut self.display_mode, DisplayMode::Values, "Values");
                ui.selectable_value(&mut self.display_mode, DisplayMode::Diff,   "Diff");
            });
        }

        if fix_clicked   { self.fix_checksums_now(); }
        if clear_compare { self.clear_compare_rom(); }
        if let Some(msg) = self.notice.clone() {
            ui.horizontal(|ui| {
                ui.colored_label(egui::Color32::LIGHT_BLUE, msg);
                if ui.button("✕").clicked() {
                    self.notice = None;
                }
            });
        }
        if let Some(err) = self.error.clone() {
            ui.horizontal(|ui| {
                ui.colored_label(egui::Color32::LIGHT_RED, err);
                if ui.button("✕").clicked() {
                    self.error = None;
                }
            });
        }
        ui.separator();
    }

    fn render_sidebar(&mut self, ui: &mut egui::Ui) {
        let Some(def) = self.def.as_ref() else {
            ui.label("Use File → Open Def… to load XML definitions.");
            return;
        };

        let current = self.selected_rom_id.clone().unwrap_or_default();
        egui::ComboBox::from_label("ROM")
            .selected_text(&current)
            .width(180.0)
            .show_ui(ui, |ui| {
                for rom in &def.roms {
                    let label = rom_label(rom);
                    ui.selectable_value(
                        &mut self.selected_rom_id,
                        Some(rom.xml_id.clone()),
                        label,
                    );
                }
            });

        ui.separator();

        let Some(rom_id) = self.selected_rom_id.clone() else {
            return;
        };
        let Some(rom_def) = def.roms.iter().find(|r| r.xml_id == rom_id) else {
            ui.colored_label(egui::Color32::YELLOW, "ROM ID not found in defs");
            return;
        };
        render_table_tree(ui, rom_def, &mut self.selected_table_name);
    }

    fn render_content(&mut self, ui: &mut egui::Ui) {
        // Disjoint-borrow: rom mut, def imm, compare imm, selection imm — все разные поля self.
        let rom_state   = self.rom.as_mut();
        let def_state   = self.def.as_ref();
        let compare_rom = self.compare_rom.as_ref().map(|r| &r.rom);
        let rom_id      = self.selected_rom_id.as_deref();
        let table_name  = self.selected_table_name.as_deref();
        let mode        = self.display_mode;

        let (Some(rom_state), Some(def_state), Some(rom_id), Some(table_name)) =
            (rom_state, def_state, rom_id, table_name)
        else {
            ui.label("Load a ROM and a definition, then pick ROM ID + table in the sidebar.");
            return;
        };

        let Some(rom_def) = def_state.roms.iter().find(|r| r.xml_id == rom_id) else {
            ui.colored_label(egui::Color32::LIGHT_RED, "Selected ROM ID not found.");
            return;
        };
        let Some(table) = rom_def.tables.iter().find(|t| t.name == table_name) else {
            ui.colored_label(egui::Color32::LIGHT_RED, "Selected table not found.");
            return;
        };

        render_table_header(ui, table);
        ui.separator();

        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| match table.kind {
                Some(TableKind::ThreeD) => render_3d(ui, &mut rom_state.rom, compare_rom, table, mode),
                Some(TableKind::TwoD)
                | Some(TableKind::OneD)
                | Some(TableKind::XAxis)
                | Some(TableKind::YAxis) => {
                    render_flat(ui, &mut rom_state.rom, compare_rom, table, mode)
                }
                Some(TableKind::StaticXAxis) | Some(TableKind::StaticYAxis) => {
                    render_static_axis(ui, table);
                }
                None => {
                    ui.colored_label(egui::Color32::YELLOW, "Table kind unknown after resolution.");
                }
            });
    }
}

fn rom_label(rom: &ResolvedRom) -> String {
    let make  = rom.romid.make.as_deref().unwrap_or("");
    let model = rom.romid.model.as_deref().unwrap_or("");
    let sub   = rom.romid.submodel.as_deref().unwrap_or("");
    let bits: Vec<&str> = [make, model, sub].into_iter().filter(|s| !s.is_empty()).collect();
    if bits.is_empty() {
        rom.xml_id.clone()
    } else {
        format!("{}  ({})", rom.xml_id, bits.join(" "))
    }
}

fn render_table_tree(
    ui:        &mut egui::Ui,
    rom_def:   &ResolvedRom,
    selection: &mut Option<String>,
) {
    let mut by_cat: BTreeMap<String, Vec<&ResolvedTable>> = BTreeMap::new();
    for t in &rom_def.tables {
        let cat = t.category.clone().unwrap_or_else(|| "Uncategorized".into());
        by_cat.entry(cat).or_default().push(t);
    }
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (cat, tables) in &by_cat {
                egui::CollapsingHeader::new(cat)
                    .default_open(true)
                    .show(ui, |ui| {
                        for t in tables {
                            let selected = selection.as_deref() == Some(t.name.as_str());
                            if ui.selectable_label(selected, &t.name).clicked() {
                                *selection = Some(t.name.clone());
                            }
                        }
                    });
            }
        });
}

fn render_table_header(ui: &mut egui::Ui, t: &ResolvedTable) {
    ui.heading(&t.name);
    ui.horizontal_wrapped(|ui| {
        ui.label(format!("kind: {}", debug_kind(t.kind)));
        ui.separator();
        ui.label(format!("storage: {}", debug_storage(t.storage_type)));
        ui.separator();
        ui.label(format!("endian: {}", debug_endian(t.endian)));
        ui.separator();
        ui.label(format!(
            "address: {}",
            t.storage_address
                .map_or_else(|| "?".into(), |a| format!("{a}"))
        ));
        ui.separator();
        ui.label(format!(
            "dims: {}×{}",
            t.size_x.map_or("?".into(), |x| x.to_string()),
            t.size_y.map_or("?".into(), |y| y.to_string()),
        ));
        if let Some(scaling) = t.scalings.first() {
            ui.separator();
            ui.label(format!("units: {}", scaling.units.as_deref().unwrap_or("-")));
        }
    });
    if let Some(desc) = &t.description {
        ui.label(egui::RichText::new(desc).italics().weak());
    }
}

fn render_3d(
    ui:          &mut egui::Ui,
    rom:         &mut RomImage,
    compare_rom: Option<&RomImage>,
    table:       &ResolvedTable,
    mode:        DisplayMode,
) {
    let (Some(size_x), Some(size_y)) = (table.size_x, table.size_y) else {
        ui.colored_label(egui::Color32::YELLOW, "3D table is missing sizex/sizey.");
        return;
    };
    let size_x = size_x as usize;
    let size_y = size_y as usize;

    let raw = match rom.read_table(table) {
        Ok(d) => d,
        Err(e) => {
            ui.colored_label(egui::Color32::LIGHT_RED, format!("Read failed: {e}"));
            return;
        }
    };
    let scaling   = compile_first_scaling(table);
    let precision = precision_from_format(table.scalings.first().and_then(|s| s.format.as_deref()));
    let speed     = cell_speed(scaling.as_ref(), precision);

    // Real-values из current ROM для отображения/редактирования.
    let mut display: Vec<f64> = raw.iter().map(|&x| to_real(scaling.as_ref(), x)).collect();
    // Real-values из base ROM (если задан) — для diff-раскраски.
    let base_real: Option<Vec<f64>> = compare_rom
        .and_then(|cr| cr.read_table(table).ok())
        .map(|raw_b| raw_b.into_iter().map(|x| to_real(scaling.as_ref(), x)).collect());

    let x_axis = table.axes.iter().find(|a| a.kind == Some(TableKind::XAxis));
    let y_axis = table.axes.iter().find(|a| a.kind == Some(TableKind::YAxis));
    let x_values = x_axis.and_then(|a| rom.read_cells(a, size_x).ok());
    let y_values = y_axis.and_then(|a| rom.read_cells(a, size_y).ok());
    let x_scaling   = x_axis.and_then(compile_first_scaling);
    let y_scaling   = y_axis.and_then(compile_first_scaling);
    let x_precision = precision_from_format(
        x_axis.and_then(|a| a.scalings.first().and_then(|s| s.format.as_deref())),
    );
    let y_precision = precision_from_format(
        y_axis.and_then(|a| a.scalings.first().and_then(|s| s.format.as_deref())),
    );

    let mut changed = false;
    egui::Grid::new("table-3d-grid")
        .striped(true)
        .spacing([4.0, 2.0])
        .show(ui, |ui| {
            // Top-left corner + X axis header
            ui.label("");
            if let Some(xs) = &x_values {
                for x in xs {
                    let v = to_real(x_scaling.as_ref(), *x);
                    ui.label(egui::RichText::new(format!("{v:.*}", x_precision)).strong());
                }
            } else {
                for i in 0..size_x {
                    ui.label(egui::RichText::new(i.to_string()).strong());
                }
            }
            ui.end_row();

            for y in 0..size_y {
                if let Some(ys) = &y_values {
                    let v = to_real(y_scaling.as_ref(), ys[y]);
                    ui.label(egui::RichText::new(format!("{v:.*}", y_precision)).strong());
                } else {
                    ui.label(egui::RichText::new(y.to_string()).strong());
                }
                for x in 0..size_x {
                    let idx  = y * size_x + x;
                    let base = base_real.as_ref().and_then(|b| b.get(idx).copied());
                    if render_cell(ui, &mut display[idx], base, mode, precision, speed) {
                        changed = true;
                    }
                }
                ui.end_row();
            }
        });

    if changed {
        write_back(rom, table, &display, scaling.as_ref());
    }
}

fn render_flat(
    ui:          &mut egui::Ui,
    rom:         &mut RomImage,
    compare_rom: Option<&RomImage>,
    table:       &ResolvedTable,
    mode:        DisplayMode,
) {
    let count = table
        .size_x
        .or(table.size_y)
        .map(|v| v as usize)
        .or(Some(1));
    let count = match count {
        Some(0) | None => {
            ui.label("Empty table.");
            return;
        }
        Some(n) => n,
    };

    let raw = match rom.read_cells(table, count) {
        Ok(d) => d,
        Err(e) => {
            ui.colored_label(egui::Color32::LIGHT_RED, format!("Read failed: {e}"));
            return;
        }
    };
    let scaling   = compile_first_scaling(table);
    let precision = precision_from_format(table.scalings.first().and_then(|s| s.format.as_deref()));
    let speed     = cell_speed(scaling.as_ref(), precision);

    let mut display: Vec<f64> = raw.iter().map(|&x| to_real(scaling.as_ref(), x)).collect();
    let base_real: Option<Vec<f64>> = compare_rom
        .and_then(|cr| cr.read_cells(table, count).ok())
        .map(|raw_b| raw_b.into_iter().map(|x| to_real(scaling.as_ref(), x)).collect());

    let mut changed = false;
    egui::Grid::new("table-flat-grid")
        .striped(true)
        .spacing([6.0, 2.0])
        .show(ui, |ui| {
            for i in 0..display.len() {
                if i > 0 && i % 8 == 0 {
                    ui.end_row();
                }
                let base = base_real.as_ref().and_then(|b| b.get(i).copied());
                if render_cell(ui, &mut display[i], base, mode, precision, speed) {
                    changed = true;
                }
            }
            ui.end_row();
        });

    if changed {
        write_back(rom, table, &display, scaling.as_ref());
    }
}

/// Универсальная отрисовка одной ячейки: DragValue в Values-режиме, Label
/// со значением Δ в Diff-режиме. Раскраска фона по знаку diff (только когда
/// base задан).
///
/// Возвращает `true` если значение было изменено (только в Values-режиме).
fn render_cell(
    ui:        &mut egui::Ui,
    value:     &mut f64,
    base:      Option<f64>,
    mode:      DisplayMode,
    precision: usize,
    speed:     f64,
) -> bool {
    let diff       = base.map_or(0.0, |b| *value - b);
    let bg         = diff_bg(diff, base.is_some());
    let mut changed = false;

    egui::Frame::none()
        .fill(bg)
        .inner_margin(egui::Margin::same(1.0))
        .show(ui, |ui| match mode {
            DisplayMode::Values => {
                if ui
                    .add(
                        egui::DragValue::new(value)
                            .speed(speed)
                            .fixed_decimals(precision),
                    )
                    .changed()
                {
                    changed = true;
                }
            }
            DisplayMode::Diff => {
                // В Diff-режиме показываем Δ или сам value (если base нет).
                let label = match base {
                    Some(_) => format!("{:+.*}", precision, diff),
                    None    => format!("{:.*}",  precision, value),
                };
                ui.label(label);
            }
        });
    changed
}

fn diff_bg(diff: f64, have_base: bool) -> egui::Color32 {
    if !have_base {
        return egui::Color32::TRANSPARENT;
    }
    // EPSILON-чувствительность чтобы не подсвечивать совсем мелкие float-расхождения.
    const EPSILON: f64 = 1e-9;
    if diff > EPSILON {
        egui::Color32::from_rgb(80, 35, 35)  // current выше base — красноватый
    } else if diff < -EPSILON {
        egui::Color32::from_rgb(35, 65, 35)  // current ниже base — зеленоватый
    } else {
        egui::Color32::TRANSPARENT
    }
}

/// Сконвертировать «real» значения обратно в байт-репрезентацию и записать.
fn write_back(
    rom:      &mut RomImage,
    table:    &ResolvedTable,
    display:  &[f64],
    scaling:  Option<&CompiledScaling>,
) {
    let raw_back: Vec<f64> = display
        .iter()
        .map(|&v| match scaling {
            Some(s) => s.to_byte(v),
            None    => v,
        })
        .collect();
    if let Err(e) = rom.write_cells(table, &raw_back) {
        warn!(?e, "write-back failed");
    }
}

fn cell_speed(scaling: Option<&CompiledScaling>, precision: usize) -> f64 {
    scaling
        .and_then(|c| c.source.fine_increment)
        .unwrap_or(if precision == 0 { 1.0 } else { 10f64.powi(-(precision as i32)) })
}

fn render_static_axis(ui: &mut egui::Ui, table: &ResolvedTable) {
    if table.data.is_empty() {
        ui.label("Static axis has no labels.");
        return;
    }
    egui::Grid::new("table-static-grid").show(ui, |ui| {
        for (i, label) in table.data.iter().enumerate() {
            ui.label(egui::RichText::new(i.to_string()).strong());
            ui.label(label);
            ui.end_row();
        }
    });
}

fn compile_first_scaling(table: &ResolvedTable) -> Option<CompiledScaling> {
    table.scalings.first().and_then(|s| s.compile().ok())
}

fn to_real(scaling: Option<&CompiledScaling>, value: f64) -> f64 {
    scaling.map_or(value, |c| c.to_real(value))
}

fn precision_from_format(format: Option<&str>) -> usize {
    let Some(f) = format else { return 2 };
    let Some(idx) = f.find('.') else { return 0 };
    f[idx + 1..].chars().filter(|c| *c == '0' || *c == '#').count()
}

fn debug_kind(k: Option<TableKind>) -> &'static str {
    match k {
        Some(TableKind::OneD)        => "1D",
        Some(TableKind::TwoD)        => "2D",
        Some(TableKind::ThreeD)      => "3D",
        Some(TableKind::XAxis)       => "X Axis",
        Some(TableKind::YAxis)       => "Y Axis",
        Some(TableKind::StaticXAxis) => "Static X Axis",
        Some(TableKind::StaticYAxis) => "Static Y Axis",
        None => "?",
    }
}

fn debug_storage(s: Option<StorageType>) -> &'static str {
    match s {
        Some(StorageType::UInt8)  => "uint8",
        Some(StorageType::Int8)   => "int8",
        Some(StorageType::UInt16) => "uint16",
        Some(StorageType::Int16)  => "int16",
        Some(StorageType::UInt32) => "uint32",
        Some(StorageType::Int32)  => "int32",
        Some(StorageType::Float)  => "float",
        Some(StorageType::Hex)    => "hex",
        Some(StorageType::Char)   => "char",
        None => "?",
    }
}

fn debug_endian(e: Option<Endian>) -> &'static str {
    match e {
        Some(Endian::Big)    => "big",
        Some(Endian::Little) => "little",
        None => "?",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precision_picks_zeros_and_hashes_after_decimal() {
        assert_eq!(precision_from_format(Some("0.00")),   2);
        assert_eq!(precision_from_format(Some("#0.000")), 3);
        assert_eq!(precision_from_format(Some("0")),      0);
        assert_eq!(precision_from_format(Some("0.0##")),  3);
        assert_eq!(precision_from_format(None),           2);
    }
}
