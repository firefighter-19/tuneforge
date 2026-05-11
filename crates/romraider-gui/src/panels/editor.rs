//! ROM editor panel — read-only MVP.
//!
//! Workflow:
//! 1. File → Open ROM…       (через rfd)
//! 2. File → Open Def…       (XML с `<roms>`)
//! 3. Левая колонка — picker ROM-ID + дерево таблиц по `category`.
//! 4. Центр — сетка значений выбранной таблицы со scaling и осями (для 3D).

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;

use romraider_core::{Address, Endian};
use romraider_defs::{
    parse_file, resolve, CompiledScaling, ResolvedRom, ResolvedTable, StorageType, TableKind,
};
use romraider_rom::{encode_cells, subaru_classic, RomImage};
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
    /// Подсвечивать ли ячейки cool→warm градиентом по значению (когда нет compare).
    heatmap_enabled:     bool,
    /// Журнал изменений для Undo/Redo.
    undo_log:            UndoLog,
}

const MAX_UNDO_HISTORY: usize = 100;

const SHORTCUT_UNDO:   egui::KeyboardShortcut =
    egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Z);
const SHORTCUT_REDO_Y: egui::KeyboardShortcut =
    egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Y);
const SHORTCUT_REDO_Z: egui::KeyboardShortcut = egui::KeyboardShortcut::new(
    egui::Modifiers { shift: true, command: true, ..egui::Modifiers::NONE },
    egui::Key::Z,
);

#[derive(Debug, Default)]
struct UndoLog {
    undo: VecDeque<EditAction>,
    redo: VecDeque<EditAction>,
}

#[derive(Debug, Clone)]
struct EditAction {
    address: Address,
    before:  Vec<u8>,
    after:   Vec<u8>,
}

impl UndoLog {
    fn record(&mut self, address: Address, before: Vec<u8>, after: Vec<u8>) {
        if before == after {
            return; // no-op
        }
        self.undo.push_back(EditAction { address, before, after });
        if self.undo.len() > MAX_UNDO_HISTORY {
            self.undo.pop_front();
        }
        self.redo.clear();
    }

    /// Применить откат к ROM. `true` если что-то было откачено.
    fn undo(&mut self, rom: &mut RomImage) -> bool {
        let Some(action) = self.undo.pop_back() else { return false };
        if rom.write(action.address, &action.before).is_err() {
            // Не удалось — возвращаем запись на место, чтобы не потерять.
            self.undo.push_back(action);
            return false;
        }
        self.redo.push_back(action);
        true
    }

    fn redo(&mut self, rom: &mut RomImage) -> bool {
        let Some(action) = self.redo.pop_back() else { return false };
        if rom.write(action.address, &action.after).is_err() {
            self.redo.push_back(action);
            return false;
        }
        self.undo.push_back(action);
        true
    }

    fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    fn can_undo(&self) -> bool { !self.undo.is_empty() }
    fn can_redo(&self) -> bool { !self.redo.is_empty() }
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
                // Новый ROM = чистая история (старые undo-байты относятся к другому файлу).
                self.undo_log.clear();
            }
            Err(e) => {
                warn!(?e, "rom open failed");
                self.error = Some(format!("Failed to open ROM: {e}"));
            }
        }
    }

    pub fn can_undo(&self) -> bool { self.undo_log.can_undo() && self.rom.is_some() }
    pub fn can_redo(&self) -> bool { self.undo_log.can_redo() && self.rom.is_some() }

    pub fn undo_action(&mut self) {
        let Some(state) = self.rom.as_mut() else { return };
        if !self.undo_log.undo(&mut state.rom) {
            self.notice = Some("Nothing to undo".into());
        }
    }

    pub fn redo_action(&mut self) {
        let Some(state) = self.rom.as_mut() else { return };
        if !self.undo_log.redo(&mut state.rom) {
            self.notice = Some("Nothing to redo".into());
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
        self.handle_shortcuts(ui);
        self.render_status(ui);

        egui::SidePanel::left("editor-sidebar")
            .min_width(220.0)
            .resizable(true)
            .show_inside(ui, |ui| self.render_sidebar(ui));

        egui::CentralPanel::default().show_inside(ui, |ui| self.render_content(ui));
    }

    fn handle_shortcuts(&mut self, ui: &mut egui::Ui) {
        let undo   = ui.input_mut(|i| i.consume_shortcut(&SHORTCUT_UNDO));
        let redo_y = ui.input_mut(|i| i.consume_shortcut(&SHORTCUT_REDO_Y));
        let redo_z = ui.input_mut(|i| i.consume_shortcut(&SHORTCUT_REDO_Z));
        if undo   { self.undo_action(); }
        if redo_y || redo_z { self.redo_action(); }
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
            ui.separator();
            ui.checkbox(&mut self.heatmap_enabled, "Heatmap")
                .on_hover_text("Раскрасить ячейки cool→warm по значению (только в режиме Values без compare ROM)");
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

        let heatmap = self.heatmap_enabled;
        let undo_log = &mut self.undo_log;
        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| match table.kind {
                Some(TableKind::ThreeD) => render_3d(
                    ui, &mut rom_state.rom, undo_log, compare_rom, table, mode, heatmap,
                ),
                Some(TableKind::TwoD)
                | Some(TableKind::OneD)
                | Some(TableKind::XAxis)
                | Some(TableKind::YAxis) => render_flat(
                    ui, &mut rom_state.rom, undo_log, compare_rom, table, mode, heatmap,
                ),
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
    undo_log:    &mut UndoLog,
    compare_rom: Option<&RomImage>,
    table:       &ResolvedTable,
    mode:        DisplayMode,
    heatmap:     bool,
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

            let heat_range = if heatmap && base_real.is_none() {
                heatmap_range(scaling.as_ref(), &display)
            } else {
                None
            };
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
                    let bg   = cell_bg(display[idx], base, heat_range);
                    if render_cell(ui, &mut display[idx], base, bg, mode, precision, speed) {
                        changed = true;
                    }
                }
                ui.end_row();
            }
        });

    if changed {
        write_back(rom, undo_log, table, &display, scaling.as_ref());
    }
}

fn render_flat(
    ui:          &mut egui::Ui,
    rom:         &mut RomImage,
    undo_log:    &mut UndoLog,
    compare_rom: Option<&RomImage>,
    table:       &ResolvedTable,
    mode:        DisplayMode,
    heatmap:     bool,
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

    let heat_range = if heatmap && base_real.is_none() {
        heatmap_range(scaling.as_ref(), &display)
    } else {
        None
    };

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
                let bg   = cell_bg(display[i], base, heat_range);
                if render_cell(ui, &mut display[i], base, bg, mode, precision, speed) {
                    changed = true;
                }
            }
            ui.end_row();
        });

    if changed {
        write_back(rom, undo_log, table, &display, scaling.as_ref());
    }
}

/// Цвет фона ячейки. Приоритет: compare-diff > heatmap > прозрачный.
fn cell_bg(value: f64, base: Option<f64>, heat_range: Option<(f64, f64)>) -> egui::Color32 {
    if let Some(b) = base {
        diff_bg(value - b, true)
    } else if let Some((mn, mx)) = heat_range {
        heat_color(value, mn, mx)
    } else {
        egui::Color32::TRANSPARENT
    }
}

/// Универсальная отрисовка одной ячейки: DragValue в Values-режиме, Label
/// со значением Δ (или сырого value) в Diff-режиме. Цвет фона `bg` уже
/// вычислен снаружи (compare-diff приоритетнее heatmap).
///
/// Возвращает `true` если значение было изменено (только в Values-режиме).
fn render_cell(
    ui:        &mut egui::Ui,
    value:     &mut f64,
    base:      Option<f64>,
    bg:        egui::Color32,
    mode:      DisplayMode,
    precision: usize,
    speed:     f64,
) -> bool {
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
                let label = match base {
                    Some(b) => format!("{:+.*}", precision, *value - b),
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

/// Диапазон для heatmap: scaling.min/max если оба заданы, иначе автоматический
/// min/max из самих данных. Возвращает `None` если диапазон вырожденный
/// (все ячейки равны или невалидны) — в этом случае heatmap не рисуется.
fn heatmap_range(scaling: Option<&CompiledScaling>, data: &[f64]) -> Option<(f64, f64)> {
    if let Some(c) = scaling {
        if let (Some(mn), Some(mx)) = (c.source.min, c.source.max) {
            if mn < mx {
                return Some((mn, mx));
            }
        }
    }
    let (mn, mx) = data.iter().copied().filter(|x| x.is_finite()).fold(
        (f64::INFINITY, f64::NEG_INFINITY),
        |(a, b), v| (a.min(v), b.max(v)),
    );
    if mn.is_finite() && mx.is_finite() && mn < mx {
        Some((mn, mx))
    } else {
        None
    }
}

/// 3-точечный градиент cool blue → mid neutral → hot red. Цвета подобраны так,
/// чтобы текст DragValue поверх оставался читаемым на тёмной теме.
fn heat_color(value: f64, min: f64, max: f64) -> egui::Color32 {
    let t = ((value - min) / (max - min)).clamp(0.0, 1.0);
    let (r0, g0, b0) = (28u8,  50,  89);   // холодный, тёмно-синий
    let (r1, g1, b1) = (75u8,  75,  60);   // нейтральный, тёмно-оливковый
    let (r2, g2, b2) = (110u8, 35,  35);   // горячий, тёмно-красный
    let (r, g, b) = if t < 0.5 {
        let u = t * 2.0;
        (lerp_u8(r0, r1, u), lerp_u8(g0, g1, u), lerp_u8(b0, b1, u))
    } else {
        let u = (t - 0.5) * 2.0;
        (lerp_u8(r1, r2, u), lerp_u8(g1, g2, u), lerp_u8(b1, b2, u))
    };
    egui::Color32::from_rgb(r, g, b)
}

fn lerp_u8(a: u8, b: u8, t: f64) -> u8 {
    let a = f64::from(a);
    let b = f64::from(b);
    (a + (b - a) * t).clamp(0.0, 255.0).round() as u8
}

/// Сконвертировать «real» значения обратно в байт-репрезентацию, записать в ROM
/// и зарегистрировать изменение в `undo_log` (для Ctrl+Z).
fn write_back(
    rom:      &mut RomImage,
    undo_log: &mut UndoLog,
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

    // Кодируем заранее, чтобы получить «after»-байты для undo.
    let (Some(addr), Some(st), Some(end)) =
        (table.storage_address, table.storage_type, table.endian)
    else {
        return;
    };
    let after  = encode_cells(&raw_back, st, end);
    let before = match rom.read(addr, after.len()) {
        Ok(b)  => b.to_vec(),
        Err(e) => {
            warn!(?e, "write-back: read failed");
            return;
        }
    };
    if let Err(e) = rom.write(addr, &after) {
        warn!(?e, "write-back failed");
        return;
    }
    undo_log.record(addr, before, after);
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

    #[test]
    fn heatmap_range_uses_data_when_scaling_missing() {
        let data = [1.0, 5.0, 3.0, 10.0, 2.0];
        let (mn, mx) = heatmap_range(None, &data).unwrap();
        assert_eq!(mn,  1.0);
        assert_eq!(mx, 10.0);
    }

    #[test]
    fn heatmap_range_degenerate_returns_none() {
        let constant = [4.0, 4.0, 4.0];
        assert!(heatmap_range(None, &constant).is_none());
        let empty: [f64; 0] = [];
        assert!(heatmap_range(None, &empty).is_none());
    }

    #[test]
    fn heat_color_clamps_outside_range() {
        let cold     = heat_color(-10.0, 0.0, 100.0);
        let cold_in  = heat_color(0.0,   0.0, 100.0);
        assert_eq!(cold, cold_in, "out-of-range clamps to cold edge");
        let hot      = heat_color(200.0, 0.0, 100.0);
        let hot_in   = heat_color(100.0, 0.0, 100.0);
        assert_eq!(hot, hot_in, "out-of-range clamps to hot edge");
    }

    #[test]
    fn heat_color_endpoints_differ() {
        let cold = heat_color(0.0, 0.0, 1.0);
        let hot  = heat_color(1.0, 0.0, 1.0);
        assert_ne!(cold, hot);
    }

    #[test]
    fn undo_log_records_and_undoes() {
        let mut log = UndoLog::default();
        let mut rom = RomImage::from_bytes(vec![0u8; 16]);
        let addr    = Address::new(0);
        let before  = vec![0, 0, 0, 0];
        let after   = vec![1, 2, 3, 4];

        rom.write(addr, &after).unwrap();
        log.record(addr, before.clone(), after.clone());

        assert!(log.can_undo());
        assert!(!log.can_redo());

        assert!(log.undo(&mut rom));
        assert_eq!(&rom.raw()[0..4], &before[..]);
        assert!(log.can_redo());
        assert!(!log.can_undo());

        assert!(log.redo(&mut rom));
        assert_eq!(&rom.raw()[0..4], &after[..]);
    }

    #[test]
    fn undo_log_clears_redo_on_new_action() {
        let mut log = UndoLog::default();
        let mut rom = RomImage::from_bytes(vec![0u8; 4]);
        let addr    = Address::new(0);

        log.record(addr, vec![0], vec![1]);
        log.undo(&mut rom);
        assert!(log.can_redo());

        log.record(addr, vec![0], vec![2]);
        assert!(!log.can_redo(), "redo stack must clear after new record");
    }

    #[test]
    fn undo_log_caps_at_max_history() {
        let mut log = UndoLog::default();
        for i in 0..MAX_UNDO_HISTORY + 50 {
            log.record(Address::new(0), vec![i as u8], vec![(i + 1) as u8]);
        }
        assert_eq!(log.undo.len(), MAX_UNDO_HISTORY);
    }

    #[test]
    fn undo_log_ignores_no_op() {
        let mut log = UndoLog::default();
        log.record(Address::new(0), vec![1, 2], vec![1, 2]);
        assert!(!log.can_undo());
    }

    #[test]
    fn multiple_undo_redo_steps() {
        let mut log = UndoLog::default();
        let mut rom = RomImage::from_bytes(vec![0u8; 4]);
        let addr    = Address::new(0);

        rom.write(addr, &[1, 0, 0, 0]).unwrap();
        log.record(addr, vec![0, 0, 0, 0], vec![1, 0, 0, 0]);
        rom.write(addr, &[1, 2, 0, 0]).unwrap();
        log.record(addr, vec![1, 0, 0, 0], vec![1, 2, 0, 0]);
        rom.write(addr, &[1, 2, 3, 0]).unwrap();
        log.record(addr, vec![1, 2, 0, 0], vec![1, 2, 3, 0]);

        log.undo(&mut rom);
        assert_eq!(rom.raw(), &[1, 2, 0, 0]);
        log.undo(&mut rom);
        assert_eq!(rom.raw(), &[1, 0, 0, 0]);
        log.undo(&mut rom);
        assert_eq!(rom.raw(), &[0, 0, 0, 0]);
        assert!(!log.can_undo());

        log.redo(&mut rom);
        log.redo(&mut rom);
        assert_eq!(rom.raw(), &[1, 2, 0, 0]);
    }
}
