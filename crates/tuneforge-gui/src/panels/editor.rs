//! ROM editor panel — load a ROM + defs, view and edit table values, save.
//!
//! Workflow:
//! 1. File → Open ROM…       (через rfd)
//! 2. File → Open Def…       (XML с `<roms>`)
//! 3. Левая колонка — picker ROM-ID + дерево таблиц по `category`.
//! 4. Центр — сетка значений выбранной таблицы со scaling и осями (для 3D).

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;

use tracing::{info, warn};
use tuneforge_core::{Address, Endian};
use tuneforge_defs::{
    parse_file, resolve, CompiledScaling, ResolvedRom, ResolvedTable, StorageType, TableKind,
};
use tuneforge_rom::{encode_cells, subaru_classic, RomImage};

use crate::panels::surface;

#[derive(Default)]
pub struct EditorPanel {
    rom: Option<RomState>,
    /// Опциональный второй ROM (base) для compare-режима. Read-only.
    compare_rom: Option<RomState>,
    def: Option<DefState>,
    selected_rom_id: Option<String>,
    selected_table_name: Option<String>,
    error: Option<String>,
    /// Кратковременное сообщение в статусной строке (например, «Saved to …»).
    notice: Option<String>,
    /// Что показывать в ячейках, когда есть `compare_rom`.
    display_mode: DisplayMode,
    /// Подсвечивать ли ячейки cool→warm градиентом по значению (когда нет compare).
    heatmap_enabled: bool,
    /// Журнал изменений для Undo/Redo.
    undo_log: UndoLog,
    /// Открыто ли окно «Changes since open».
    changes_window_open: bool,
    /// Открыто ли окно 3D-поверхности выбранной карты.
    surface_window_open: bool,
    /// Ракурс 3D-поверхности (yaw/pitch).
    surface_view: surface::SurfaceView,
}

const MAX_UNDO_HISTORY: usize = 100;

const SHORTCUT_UNDO: egui::KeyboardShortcut =
    egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Z);
const SHORTCUT_REDO_Y: egui::KeyboardShortcut =
    egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Y);
const SHORTCUT_REDO_Z: egui::KeyboardShortcut = egui::KeyboardShortcut::new(
    egui::Modifiers {
        shift: true,
        command: true,
        ..egui::Modifiers::NONE
    },
    egui::Key::Z,
);
const SHORTCUT_SAVE: egui::KeyboardShortcut =
    egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::S);

#[derive(Debug, Default)]
struct UndoLog {
    undo: VecDeque<EditAction>,
    redo: VecDeque<EditAction>,
}

#[derive(Debug, Clone)]
struct EditAction {
    address: Address,
    before: Vec<u8>,
    after: Vec<u8>,
}

impl UndoLog {
    /// Зарегистрировать изменение. Если `can_merge_with_prev` = `true`
    /// **и** последняя запись имеет тот же address + её `after` совпадает с
    /// текущим `before` (т.е. это продолжение того же редактирования), то
    /// мы **сливаем** их в одну: `last.after = new.after`. Это даёт «один
    /// undo step на drag-сессию» даже если drag-event-ы прилетели за
    /// несколько frame-ов с промежуточными значениями.
    ///
    /// Caller-ы:
    ///   - `write_back` (DragValue-cells): передаёт `can_merge=true` когда
    ///     drag всё ещё продолжается (frame N>1 одного drag-а), иначе
    ///     `false` (первый frame drag-а / typed edit).
    ///   - switch/bitwise (дискретные клики): всегда `false`.
    fn record(
        &mut self,
        address: Address,
        before: Vec<u8>,
        after: Vec<u8>,
        can_merge_with_prev: bool,
    ) {
        if before == after {
            return; // no-op
        }
        if can_merge_with_prev {
            if let Some(last) = self.undo.back_mut() {
                if last.address == address && last.after == before {
                    last.after = after;
                    self.redo.clear();
                    return;
                }
            }
        }
        self.undo.push_back(EditAction {
            address,
            before,
            after,
        });
        if self.undo.len() > MAX_UNDO_HISTORY {
            self.undo.pop_front();
        }
        self.redo.clear();
    }

    /// Применить откат к ROM. `true` если что-то было откачено.
    fn undo(&mut self, rom: &mut RomImage) -> bool {
        let Some(action) = self.undo.pop_back() else {
            return false;
        };
        if rom.write(action.address, &action.before).is_err() {
            // Не удалось — возвращаем запись на место, чтобы не потерять.
            self.undo.push_back(action);
            return false;
        }
        self.redo.push_back(action);
        true
    }

    fn redo(&mut self, rom: &mut RomImage) -> bool {
        let Some(action) = self.redo.pop_back() else {
            return false;
        };
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

    fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
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
    rom: RomImage,
    /// Снапшот байтов на момент загрузки (или последнего успешного сохранения).
    /// Используется для:
    ///  - подсветки изменённых ячеек жёлтым в `cell_bg`
    ///  - сводки «Changes since open» (что и где было поправлено)
    original_bytes: Vec<u8>,
}

impl RomState {
    fn new(path: PathBuf, rom: RomImage) -> Self {
        let original_bytes = rom.raw().to_vec();
        Self {
            path,
            rom,
            original_bytes,
        }
    }

    /// Зафиксировать «текущее состояние» как новый baseline (после `Save`).
    fn snapshot_as_baseline(&mut self) {
        self.original_bytes = self.rom.raw().to_vec();
    }

    /// Изменён ли байтовый диапазон `[addr, addr+len)` относительно baseline.
    /// Используется для diff-highlight в Compare-ROM-режиме; clippy предлагает
    /// удалить как «dead», но это часть public API и могут понадобиться плагины.
    #[allow(dead_code)]
    fn is_range_modified(&self, addr: u32, len: usize) -> bool {
        let a = addr as usize;
        let Some(end) = a.checked_add(len) else {
            return false;
        };
        if end > self.original_bytes.len() {
            return false;
        }
        let cur = match self.rom.read(Address::new(addr), len) {
            Ok(b) => b,
            Err(_) => return false,
        };
        cur != &self.original_bytes[a..end]
    }
}

struct DefState {
    path: PathBuf,
    roms: Vec<ResolvedRom>,
}

impl EditorPanel {
    pub fn load_rom(&mut self, path: PathBuf) {
        match RomImage::open(&path) {
            Ok(rom) => {
                self.rom = Some(RomState::new(path, rom));
                self.error = None;
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

    pub fn can_undo(&self) -> bool {
        self.undo_log.can_undo() && self.rom.is_some()
    }
    pub fn can_redo(&self) -> bool {
        self.undo_log.can_redo() && self.rom.is_some()
    }

    pub fn undo_action(&mut self) {
        let Some(state) = self.rom.as_mut() else {
            return;
        };
        if !self.undo_log.undo(&mut state.rom) {
            self.notice = Some("Nothing to undo".into());
        }
    }

    pub fn redo_action(&mut self) {
        let Some(state) = self.rom.as_mut() else {
            return;
        };
        if !self.undo_log.redo(&mut state.rom) {
            self.notice = Some("Nothing to redo".into());
        }
    }

    pub fn load_compare_rom(&mut self, path: PathBuf) {
        match RomImage::open(&path) {
            Ok(rom) => {
                self.compare_rom = Some(RomState::new(path, rom));
                self.error = None;
                self.notice = None;
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
                self.def = Some(DefState { path, roms });
                self.selected_rom_id = None;
                self.selected_table_name = None;
                self.error = None;
                self.notice = None;
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
        self.render_changes_window(ui.ctx());
        self.render_surface_window(ui.ctx());

        egui::SidePanel::left("editor-sidebar")
            .min_width(220.0)
            .resizable(true)
            .show_inside(ui, |ui| self.render_sidebar(ui));

        egui::CentralPanel::default().show_inside(ui, |ui| self.render_content(ui));
    }

    fn render_changes_window(&mut self, ctx: &egui::Context) {
        if !self.changes_window_open {
            return;
        }
        let mut open = self.changes_window_open;
        egui::Window::new("Changes since open / last save")
            .open(&mut open)
            .default_size([720.0, 480.0])
            .show(ctx, |ui| {
                let Some(rom_state) = self.rom.as_ref() else {
                    ui.label("No ROM loaded.");
                    return;
                };
                let Some(def) = self.def.as_ref() else {
                    ui.label(
                        "ROM is dirty, but no definitions are loaded — cannot map changes \
                         back to named tables. Load defs first.",
                    );
                    return;
                };
                let Some(rom_id) = self.selected_rom_id.as_ref() else {
                    ui.label("No ROM definition selected from defs.");
                    return;
                };
                let Some(rom_def) = def.roms.iter().find(|r| &r.xml_id == rom_id) else {
                    ui.label(format!("ROM `{rom_id}` not found in defs."));
                    return;
                };
                let summary = collect_changes(rom_state, rom_def);
                if summary.tables.is_empty() {
                    ui.label("No cell changes detected (file write-back may have been a no-op).");
                    return;
                }
                ui.heading(format!(
                    "{} cell{} modified across {} table{}",
                    summary.total_cells,
                    if summary.total_cells == 1 { "" } else { "s" },
                    summary.tables.len(),
                    if summary.tables.len() == 1 { "" } else { "s" },
                ));
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for entry in &summary.tables {
                        egui::CollapsingHeader::new(format!(
                            "{}  —  {} cell{}",
                            entry.name,
                            entry.cells.len(),
                            if entry.cells.len() == 1 { "" } else { "s" },
                        ))
                        .default_open(summary.tables.len() <= 3)
                        .show(ui, |ui| {
                            egui::Grid::new(format!("changes-grid-{}", entry.name))
                                .striped(true)
                                .num_columns(4)
                                .show(ui, |ui| {
                                    ui.label(egui::RichText::new("Cell").strong());
                                    ui.label(egui::RichText::new("Before").strong());
                                    ui.label(egui::RichText::new("After").strong());
                                    ui.label(egui::RichText::new("Δ").strong());
                                    ui.end_row();
                                    for c in &entry.cells {
                                        ui.label(c.label.as_str());
                                        ui.label(format!("{:.4}", c.before_real));
                                        ui.label(format!("{:.4}", c.after_real));
                                        let delta = c.after_real - c.before_real;
                                        ui.colored_label(
                                            if delta > 0.0 {
                                                egui::Color32::LIGHT_GREEN
                                            } else {
                                                egui::Color32::LIGHT_RED
                                            },
                                            format!("{:+.4}", delta),
                                        );
                                        ui.end_row();
                                    }
                                });
                        });
                    }
                });
            });
        self.changes_window_open = open;
    }

    fn handle_shortcuts(&mut self, ui: &mut egui::Ui) {
        let undo = ui.input_mut(|i| i.consume_shortcut(&SHORTCUT_UNDO));
        let redo_y = ui.input_mut(|i| i.consume_shortcut(&SHORTCUT_REDO_Y));
        let redo_z = ui.input_mut(|i| i.consume_shortcut(&SHORTCUT_REDO_Z));
        let save = ui.input_mut(|i| i.consume_shortcut(&SHORTCUT_SAVE));
        if undo {
            self.undo_action();
        }
        if redo_y || redo_z {
            self.redo_action();
        }
        if save {
            self.save_rom();
        }
    }

    fn render_surface_window(&mut self, ctx: &egui::Context) {
        if !self.surface_window_open {
            return;
        }
        let mut open = self.surface_window_open;
        egui::Window::new("3D surface")
            .open(&mut open)
            .default_size([560.0, 480.0])
            .show(ctx, |ui| self.render_surface_body(ui));
        self.surface_window_open = open;
    }

    fn render_surface_body(&mut self, ui: &mut egui::Ui) {
        let Some((name, z, sx, sy)) = self.selected_surface_data() else {
            ui.label("Select a 3D table (with axes) to see its surface.");
            return;
        };
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(name).strong());
            ui.label(egui::RichText::new("— drag to rotate").weak());
            if ui.button("Reset view").clicked() {
                self.surface_view = surface::SurfaceView::default();
            }
        });
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), egui::Sense::drag());
        if response.dragged() {
            let d = response.drag_delta();
            self.surface_view.yaw += d.x * 0.01;
            self.surface_view.pitch = (self.surface_view.pitch + d.y * 0.01).clamp(0.15, 1.55);
        }
        draw_surface(ui.painter(), rect, &z, sx, sy, self.surface_view);
    }

    /// Собрать данные выбранной 3D-таблицы: (имя, real-значения Z по строкам,
    /// `size_x`, `size_y`). `None`, если подходящей 3D-таблицы нет.
    fn selected_surface_data(&self) -> Option<(String, Vec<f64>, usize, usize)> {
        let rom_state = self.rom.as_ref()?;
        let def = self.def.as_ref()?;
        let rom_id = self.selected_rom_id.as_ref()?;
        let table_name = self.selected_table_name.as_ref()?;
        let rom_def = def.roms.iter().find(|r| &r.xml_id == rom_id)?;
        let table = rom_def.tables.iter().find(|t| &t.name == table_name)?;
        let sx = table.size_x? as usize;
        let sy = table.size_y? as usize;
        if sx < 2 || sy < 2 {
            return None;
        }
        let raw = rom_state.rom.read_table(table).ok()?;
        if raw.len() != sx * sy {
            return None;
        }
        let scaling = compile_first_scaling(table);
        let z = raw.iter().map(|&x| to_real(scaling.as_ref(), x)).collect();
        Some((table_name.clone(), z, sx, sy))
    }

    /// Загружен ли ROM (для enable/disable пунктов меню и guard'а).
    pub fn has_rom(&self) -> bool {
        self.rom.is_some()
    }

    /// Есть ли несохранённые правки.
    pub fn is_dirty(&self) -> bool {
        self.rom.as_ref().is_some_and(|s| s.rom.is_dirty())
    }

    /// Сохранить на месте (в текущий путь) — File → Save / Ctrl+S. Возвращает
    /// `false`, если ROM не загружен (тогда вызывающий откроет диалог Save As…).
    pub fn save_rom(&mut self) -> bool {
        let Some(path) = self.rom.as_ref().map(|s| s.path.clone()) else {
            return false;
        };
        self.save_to(path);
        true
    }

    pub fn save_rom_as(&mut self, path: PathBuf) {
        self.save_to(path);
    }

    fn save_to(&mut self, path: PathBuf) {
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
                    Ok(0) => {} // нет таблиц или нечего обновлять
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
                // После успешного save текущее состояние становится новым baseline —
                // «модифицированные» ячейки очищаются.
                state.snapshot_as_baseline();
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
        let rom = self.rom.as_ref()?;
        let def = self.def.as_ref()?;
        let rom_id = self.selected_rom_id.as_deref()?;
        let rom_def = def.roms.iter().find(|r| r.xml_id == rom_id)?;

        let results = subaru_classic::verify(&rom.rom, rom_def).ok()?;
        if results.is_empty() {
            return None;
        }
        let valid = results.iter().filter(|r| r.valid).count();
        Some(ChecksumSummary {
            valid,
            total: results.len(),
        })
    }

    /// Пересчитать checksum-fix-таблицы прямо сейчас, не сохраняя файл.
    fn fix_checksums_now(&mut self) {
        let Some(state) = self.rom.as_mut() else {
            return;
        };
        let Some(def) = self.def.as_ref() else { return };
        let Some(rom_id) = self.selected_rom_id.as_deref() else {
            return;
        };
        let Some(rom_def) = def.roms.iter().find(|r| r.xml_id == rom_id) else {
            return;
        };

        match subaru_classic::fix(&mut state.rom, rom_def) {
            Ok(n) => {
                self.error = None;
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
        let mut fix_clicked = false;
        let mut clear_compare = false;

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("ROM:").strong());
            match &self.rom {
                Some(r) => {
                    ui.label(r.path.display().to_string());
                    if r.rom.is_dirty() {
                        let modified_btn = egui::Button::new(
                            egui::RichText::new("● modified").color(egui::Color32::YELLOW),
                        )
                        .small()
                        .fill(egui::Color32::TRANSPARENT);
                        if ui
                            .add(modified_btn)
                            .on_hover_text("Show what changed since open / last save")
                            .clicked()
                        {
                            self.changes_window_open = true;
                        }
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
                    .map_or_else(|| "(not loaded)".into(), |d| d.path.display().to_string()),
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
                .on_hover_text("Color cells cool→warm by value (Values mode only, no compare ROM)");
            ui.checkbox(&mut self.surface_window_open, "3D surface")
                .on_hover_text("Show the selected 3D map as a rotatable colored surface");
        });

        // Вторая строка: compare-ROM + display-mode toggle.
        if let Some(c) = &self.compare_rom {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Base:").strong());
                ui.label(c.path.display().to_string());
                if ui
                    .small_button("✕")
                    .on_hover_text("Close compare ROM")
                    .clicked()
                {
                    clear_compare = true;
                }
                ui.separator();
                ui.label("Show:");
                ui.selectable_value(&mut self.display_mode, DisplayMode::Values, "Values");
                ui.selectable_value(&mut self.display_mode, DisplayMode::Diff, "Diff");
            });
        }

        if fix_clicked {
            self.fix_checksums_now();
        }
        if clear_compare {
            self.clear_compare_rom();
        }
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
                    ui.selectable_value(&mut self.selected_rom_id, Some(rom.xml_id.clone()), label);
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
        let rom_state = self.rom.as_mut();
        let def_state = self.def.as_ref();
        let compare_rom = self.compare_rom.as_ref().map(|r| &r.rom);
        let rom_id = self.selected_rom_id.as_deref();
        let table_name = self.selected_table_name.as_deref();
        let mode = self.display_mode;

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
        // Подсветка изменённых ячеек жёлтым делается только когда у нас нет
        // явного compare_rom — иначе режим Compare красит red/green-ом.
        let baseline_bytes: Option<&[u8]> = if compare_rom.is_none() {
            Some(rom_state.original_bytes.as_slice())
        } else {
            None
        };
        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| match table.kind {
                Some(TableKind::ThreeD) => render_3d(
                    ui,
                    &mut rom_state.rom,
                    undo_log,
                    compare_rom,
                    baseline_bytes,
                    table,
                    mode,
                    heatmap,
                ),
                Some(TableKind::TwoD | TableKind::OneD | TableKind::XAxis | TableKind::YAxis) => {
                    render_flat(
                        ui,
                        &mut rom_state.rom,
                        undo_log,
                        compare_rom,
                        baseline_bytes,
                        table,
                        mode,
                        heatmap,
                    );
                }
                Some(TableKind::StaticXAxis | TableKind::StaticYAxis) => {
                    render_static_axis(ui, table);
                }
                Some(TableKind::Switch) => {
                    render_switch(ui, &mut rom_state.rom, undo_log, table);
                }
                Some(TableKind::BitwiseSwitch) => {
                    render_bitwise_switch(ui, &mut rom_state.rom, undo_log, table);
                }
                None => {
                    ui.colored_label(
                        egui::Color32::YELLOW,
                        "Table kind unknown after resolution.",
                    );
                }
            });
    }
}

fn rom_label(rom: &ResolvedRom) -> String {
    let make = rom.romid.make.as_deref().unwrap_or("");
    let model = rom.romid.model.as_deref().unwrap_or("");
    let sub = rom.romid.submodel.as_deref().unwrap_or("");
    let bits: Vec<&str> = [make, model, sub]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect();
    if bits.is_empty() {
        rom.xml_id.clone()
    } else {
        format!("{}  ({})", rom.xml_id, bits.join(" "))
    }
}

fn render_table_tree(ui: &mut egui::Ui, rom_def: &ResolvedRom, selection: &mut Option<String>) {
    // Subaru-defs наследуют тысячи таблиц от base/template-ROM-ов, но в
    // конкретной прошивке физически присутствуют только те, у которых
    // resolver проставил `storage_address`. Остальные — варианты «для MT»,
    // «для другого региона» и т.п. — должны быть скрыты, иначе пользователь
    // увидит дубли с одинаковыми именами и Read failed на отсутствующем адресе.
    // Дополнительно дедуплицируем по имени, оставляя первую запись с адресом.
    let mut seen_names: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut by_cat: BTreeMap<String, Vec<&ResolvedTable>> = BTreeMap::new();
    for t in &rom_def.tables {
        if t.storage_address.is_none() {
            continue;
        }
        if !seen_names.insert(t.name.as_str()) {
            continue;
        }
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
            ui.label(format!(
                "units: {}",
                scaling.units.as_deref().unwrap_or("-")
            ));
        }
    });
    if let Some(desc) = &t.description {
        ui.label(egui::RichText::new(desc).italics().weak());
    }
}

fn render_3d(
    ui: &mut egui::Ui,
    rom: &mut RomImage,
    undo_log: &mut UndoLog,
    compare_rom: Option<&RomImage>,
    baseline_bytes: Option<&[u8]>,
    table: &ResolvedTable,
    mode: DisplayMode,
    heatmap: bool,
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
    let scaling = compile_first_scaling(table);
    let precision = precision_from_format(table.scalings.first().and_then(|s| s.format.as_deref()));
    let speed = cell_speed(scaling.as_ref(), precision);

    // Real-values из current ROM для отображения/редактирования.
    let mut display: Vec<f64> = raw.iter().map(|&x| to_real(scaling.as_ref(), x)).collect();
    // Real-values из base ROM (если задан) — для diff-раскраски.
    let base_real: Option<Vec<f64>> =
        compare_rom
            .and_then(|cr| cr.read_table(table).ok())
            .map(|raw_b| {
                raw_b
                    .into_iter()
                    .map(|x| to_real(scaling.as_ref(), x))
                    .collect()
            });

    let x_axis = table.axes.iter().find(|a| a.kind == Some(TableKind::XAxis));
    let y_axis = table.axes.iter().find(|a| a.kind == Some(TableKind::YAxis));
    let x_values = x_axis.and_then(|a| rom.read_cells(a, size_x).ok());
    let y_values = y_axis.and_then(|a| rom.read_cells(a, size_y).ok());
    let x_scaling = x_axis.and_then(compile_first_scaling);
    let y_scaling = y_axis.and_then(compile_first_scaling);
    let x_precision = precision_from_format(
        x_axis.and_then(|a| a.scalings.first().and_then(|s| s.format.as_deref())),
    );
    let y_precision = precision_from_format(
        y_axis.and_then(|a| a.scalings.first().and_then(|s| s.format.as_deref())),
    );

    let mut changed = false;
    let mut drag_continuation = false;
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
            let stride = table
                .storage_type
                .map_or(1, tuneforge_defs::StorageType::byte_size);
            let base_addr_raw = table
                .storage_address
                .map_or(0, tuneforge_core::Address::raw);
            let units = table.scalings.first().and_then(|s| s.units.as_deref());
            let expression = scaling
                .as_ref()
                .and_then(|c| c.source.expression.as_deref());
            let to_byte_expr = scaling.as_ref().and_then(|c| c.source.to_byte.as_deref());
            let description = table.description.as_deref();

            for y in 0..size_y {
                if let Some(ys) = &y_values {
                    let v = to_real(y_scaling.as_ref(), ys[y]);
                    ui.label(egui::RichText::new(format!("{v:.*}", y_precision)).strong());
                } else {
                    ui.label(egui::RichText::new(y.to_string()).strong());
                }
                for x in 0..size_x {
                    let idx = y * size_x + x;
                    let base = base_real.as_ref().and_then(|b| b.get(idx).copied());
                    let cell_addr = base_addr_raw + (idx * stride) as u32;
                    let modified = is_cell_modified(rom, baseline_bytes, cell_addr, stride);
                    let bg = cell_bg(display[idx], base, heat_range, modified);
                    let tooltip = CellTooltip {
                        addr: Address::new(cell_addr),
                        raw_byte: raw[idx],
                        position: format!("y={y}, x={x}"),
                        units,
                        expression,
                        to_byte: to_byte_expr,
                        description,
                    };
                    let cell = render_cell(
                        ui,
                        &mut display[idx],
                        base,
                        bg,
                        mode,
                        precision,
                        speed,
                        &tooltip,
                    );
                    if cell.changed {
                        changed = true;
                        if cell.is_drag_continuation {
                            drag_continuation = true;
                        }
                    }
                }
                ui.end_row();
            }
        });

    if changed {
        write_back(
            rom,
            undo_log,
            table,
            &display,
            scaling.as_ref(),
            drag_continuation,
        );
    }
}

fn render_flat(
    ui: &mut egui::Ui,
    rom: &mut RomImage,
    undo_log: &mut UndoLog,
    compare_rom: Option<&RomImage>,
    baseline_bytes: Option<&[u8]>,
    table: &ResolvedTable,
    mode: DisplayMode,
    heatmap: bool,
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
    let scaling = compile_first_scaling(table);
    let precision = precision_from_format(table.scalings.first().and_then(|s| s.format.as_deref()));
    let speed = cell_speed(scaling.as_ref(), precision);

    let mut display: Vec<f64> = raw.iter().map(|&x| to_real(scaling.as_ref(), x)).collect();
    let base_real: Option<Vec<f64>> = compare_rom
        .and_then(|cr| cr.read_cells(table, count).ok())
        .map(|raw_b| {
            raw_b
                .into_iter()
                .map(|x| to_real(scaling.as_ref(), x))
                .collect()
        });

    let heat_range = if heatmap && base_real.is_none() {
        heatmap_range(scaling.as_ref(), &display)
    } else {
        None
    };
    let stride = table
        .storage_type
        .map_or(1, tuneforge_defs::StorageType::byte_size);
    let base_addr_raw = table
        .storage_address
        .map_or(0, tuneforge_core::Address::raw);
    let units = table.scalings.first().and_then(|s| s.units.as_deref());
    let expression = scaling
        .as_ref()
        .and_then(|c| c.source.expression.as_deref());
    let to_byte_expr = scaling.as_ref().and_then(|c| c.source.to_byte.as_deref());
    let description = table.description.as_deref();

    let mut changed = false;
    let mut drag_continuation = false;
    egui::Grid::new("table-flat-grid")
        .striped(true)
        .spacing([6.0, 2.0])
        .show(ui, |ui| {
            for i in 0..display.len() {
                if i > 0 && i % 8 == 0 {
                    ui.end_row();
                }
                let base = base_real.as_ref().and_then(|b| b.get(i).copied());
                let cell_addr = base_addr_raw + (i * stride) as u32;
                let modified = is_cell_modified(rom, baseline_bytes, cell_addr, stride);
                let bg = cell_bg(display[i], base, heat_range, modified);
                let tooltip = CellTooltip {
                    addr: Address::new(base_addr_raw + (i * stride) as u32),
                    raw_byte: raw[i],
                    position: format!("i={i}"),
                    units,
                    expression,
                    to_byte: to_byte_expr,
                    description,
                };
                let cell = render_cell(
                    ui,
                    &mut display[i],
                    base,
                    bg,
                    mode,
                    precision,
                    speed,
                    &tooltip,
                );
                if cell.changed {
                    changed = true;
                    if cell.is_drag_continuation {
                        drag_continuation = true;
                    }
                }
            }
            ui.end_row();
        });

    if changed {
        write_back(
            rom,
            undo_log,
            table,
            &display,
            scaling.as_ref(),
            drag_continuation,
        );
    }
}

/// Цвет фона ячейки. Приоритет: compare-diff > heatmap > прозрачный.
/// Был ли байтовый диапазон ячейки изменён относительно baseline-снапшота
/// (открытие ROM или последний `Save`).
fn is_cell_modified(rom: &RomImage, baseline: Option<&[u8]>, addr: u32, len: usize) -> bool {
    let Some(orig) = baseline else { return false };
    let a = addr as usize;
    let Some(end) = a.checked_add(len) else {
        return false;
    };
    if end > orig.len() {
        return false;
    }
    rom.read(Address::new(addr), len)
        .is_ok_and(|cur| cur != &orig[a..end])
}

fn cell_bg(
    value: f64,
    base: Option<f64>,
    heat_range: Option<(f64, f64)>,
    modified: bool,
) -> egui::Color32 {
    if let Some(b) = base {
        diff_bg(value - b, true)
    } else if let Some((mn, mx)) = heat_range {
        heat_color(value, mn, mx)
    } else if modified {
        // Жёлто-янтарный тинт для ячеек, изменённых с момента открытия ROM.
        // Гасим прозрачностью, чтобы текст оставался читаемым.
        egui::Color32::from_rgba_unmultiplied(0xFF, 0xD0, 0x40, 70)
    } else {
        egui::Color32::TRANSPARENT
    }
}

/// Информация для tooltip-а одной ячейки (то, что должно быть видно при hover).
struct CellTooltip<'a> {
    addr: Address,
    raw_byte: f64,
    position: String,
    units: Option<&'a str>,
    expression: Option<&'a str>,
    to_byte: Option<&'a str>,
    /// Описание таблицы из `ecu_defs.xml` (атрибут `<table description="…">`).
    /// Отображается на самом верху tooltip-а — частая причина hover-а:
    /// «что вообще делает эта ячейка?». См. Slice 14 critpath item #9.
    description: Option<&'a str>,
}

/// Результат отрисовки одной ячейки в Values-режиме. Нужен для coalescing-а
/// undo: за один drag-сессию (нажал → потащил → отпустил) DragValue может
/// прилететь с изменениями в ~60 frame-ов подряд, и без объединения в undo
/// получалось ~60 шагов Ctrl+Z вместо одного.
#[derive(Default, Clone, Copy)]
struct CellEdit {
    /// Значение этой ячейки изменилось в этом frame.
    changed: bool,
    /// Это **продолжение** активного drag-а (frame N>1 одной drag-сессии),
    /// **не** первый frame и **не** typed-edit. Используется как сигнал
    /// undo-логу что новую запись можно merge-нуть с предыдущей.
    is_drag_continuation: bool,
}

/// Универсальная отрисовка одной ячейки: DragValue в Values-режиме, Label
/// со значением Δ (или сырого value) в Diff-режиме. Цвет фона `bg` уже
/// вычислен снаружи (compare-diff приоритетнее heatmap). При hover показывает
/// `tooltip` — адрес, raw/real, формула, diff vs base (если задан).
///
/// Возвращает [`CellEdit`] — `changed` + флаг «это продолжение drag-а».
/// Caller (`render_3d`/`render_flat`) агрегирует флаги по всему grid-у и
/// передаёт результат в `write_back` для coalescing-а undo.
fn render_cell(
    ui: &mut egui::Ui,
    value: &mut f64,
    base: Option<f64>,
    bg: egui::Color32,
    mode: DisplayMode,
    precision: usize,
    speed: f64,
    tooltip: &CellTooltip<'_>,
) -> CellEdit {
    let mut edit = CellEdit::default();
    let response = egui::Frame::none()
        .fill(bg)
        .inner_margin(egui::Margin::same(1.0))
        .show(ui, |ui| match mode {
            DisplayMode::Values => {
                let resp = ui.add(
                    egui::DragValue::new(value)
                        .speed(speed)
                        .fixed_decimals(precision),
                );
                if resp.changed() {
                    edit.changed = true;
                    // Drag-continuation = mid-drag (frame N>1). Первый frame
                    // drag-а (`drag_started`) и typed-edit (no `dragged`) не
                    // считаются — каждый из них даёт свежий undo-step.
                    edit.is_drag_continuation = resp.dragged() && !resp.drag_started();
                }
            }
            DisplayMode::Diff => {
                let label = match base {
                    Some(b) => format!("{:+.*}", precision, *value - b),
                    None => format!("{:.*}", precision, value),
                };
                ui.label(label);
            }
        })
        .response;

    let snapshot_value = *value;
    response.on_hover_ui(|ui| {
        cell_tooltip_ui(ui, tooltip, snapshot_value, base, precision);
    });
    edit
}

fn cell_tooltip_ui(
    ui: &mut egui::Ui,
    t: &CellTooltip<'_>,
    real_value: f64,
    base: Option<f64>,
    precision: usize,
) {
    let units = t.units.unwrap_or("");
    if let Some(desc) = t.description {
        // Описание таблицы — самая полезная инфа при hover-е («что эта
        // ячейка вообще делает»). Курсивом + italics чтобы не сливалось
        // с monospace-телом tooltip-а.
        ui.label(egui::RichText::new(desc).italics());
        ui.separator();
    }
    ui.label(
        egui::RichText::new(format!("@ {}  ({})", t.addr, t.position))
            .strong()
            .monospace(),
    );
    ui.label(format!("Real:  {:.*} {}", precision, real_value, units));
    ui.label(format!("Byte:  {}", format_byte(t.raw_byte)));
    if let Some(b) = base {
        ui.separator();
        ui.label(format!("Base:  {:.*} {}", precision, b, units));
        ui.label(format!("Δ:     {:+.*}", precision, real_value - b));
    }
    if t.expression.is_some() || t.to_byte.is_some() {
        ui.separator();
        if let Some(e) = t.expression {
            ui.label(format!("byte → real:  {e}"));
        }
        if let Some(b) = t.to_byte {
            ui.label(format!("real → byte:  {b}"));
        }
    }
}

/// Форматирование «raw byte»-значения: целочисленные показываем без точки,
/// дробные (float storage) — с 4 знаками.
fn format_byte(v: f64) -> String {
    if v.fract().abs() < 1e-9 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v:.4}")
    }
}

fn diff_bg(diff: f64, have_base: bool) -> egui::Color32 {
    if !have_base {
        return egui::Color32::TRANSPARENT;
    }
    // EPSILON-чувствительность чтобы не подсвечивать совсем мелкие float-расхождения.
    const EPSILON: f64 = 1e-9;
    if diff > EPSILON {
        egui::Color32::from_rgb(80, 35, 35) // current выше base — красноватый
    } else if diff < -EPSILON {
        egui::Color32::from_rgb(35, 65, 35) // current ниже base — зеленоватый
    } else {
        egui::Color32::TRANSPARENT
    }
}

/// Диапазон для heatmap: scaling.min/max если оба заданы, иначе автоматический
/// min/max из самих данных. Возвращает `None` если диапазон вырожденный
/// (все ячейки равны или невалидны) — в этом случае heatmap не рисуется.
/// Нарисовать 3D-поверхность карты: проекция каждой вершины, заливка квадратов
/// цветом по нормированной высоте (`heat_color`), отрисовка far→near
/// (painter's algorithm). Порядок — по средней глубине квадрата; для гладких
/// карт достаточно, сильно взаимопроникающие квадраты он не разрешает (нужен
/// был бы z-buffer). Таблица перечитывается каждый кадр — для 15×18 дёшево;
/// при желании можно кэшировать по `(rom_id, table_name)`.
fn draw_surface(
    painter: &egui::Painter,
    rect: egui::Rect,
    z: &[f64],
    sx: usize,
    sy: usize,
    view: surface::SurfaceView,
) {
    if sx < 2 || sy < 2 || z.len() != sx * sy {
        return;
    }
    let pad = 12.0;
    if rect.width() < 2.0 * pad || rect.height() < 2.0 * pad {
        return; // окно слишком маленькое — рисовать негде
    }

    // Диапазон высот берём ТОЛЬКО по конечным значениям: scaling может дать
    // inf/NaN (напр. `k/x` при нулевой ячейке), и один inf иначе «схлопнул» бы
    // всю поверхность. Пустой/вырожденный диапазон → рисуем плоскость по центру.
    let (zmin, zmax) = z
        .iter()
        .copied()
        .filter(|v| v.is_finite())
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| {
            (lo.min(v), hi.max(v))
        });
    let degenerate = !zmin.is_finite() || (zmax - zmin).abs() < f64::EPSILON;

    // Нормированная высота [0,1] на вершину (0.5 для не-конечных/вырожденных) —
    // используется и для высоты, и для цвета, поэтому inf/NaN и плоские карты
    // не портят ни геометрию, ни `heat_color`.
    let norm: Vec<f32> = z
        .iter()
        .map(|&v| {
            if degenerate || !v.is_finite() {
                0.5
            } else {
                (((v - zmin) / (zmax - zmin)) as f32).clamp(0.0, 1.0)
            }
        })
        .collect();

    const HEIGHT: f32 = 0.55;
    let mut pts: Vec<(egui::Pos2, f32)> = Vec::with_capacity(sx * sy);
    for j in 0..sy {
        for i in 0..sx {
            let gx = i as f32 / (sx - 1) as f32 - 0.5;
            let gz = j as f32 / (sy - 1) as f32 - 0.5;
            let gy = (norm[j * sx + i] - 0.5) * HEIGHT;
            let (px, py, depth) = surface::project(gx, gy, gz, view.yaw, view.pitch);
            pts.push((egui::pos2(px, py), depth));
        }
    }

    // Вписать проекцию в rect (равномерный масштаб, по центру).
    let (mut minx, mut miny, mut maxx, mut maxy) = (
        f32::INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
    );
    for (p, _) in &pts {
        minx = minx.min(p.x);
        maxx = maxx.max(p.x);
        miny = miny.min(p.y);
        maxy = maxy.max(p.y);
    }
    let scale = ((rect.width() - 2.0 * pad) / (maxx - minx).max(1e-3))
        .min((rect.height() - 2.0 * pad) / (maxy - miny).max(1e-3));
    let cx = rect.center().x - (minx + maxx) * 0.5 * scale;
    let cy = rect.center().y - (miny + maxy) * 0.5 * scale;
    let to_screen = |p: egui::Pos2| egui::pos2(p.x * scale + cx, p.y * scale + cy);

    // Квадраты со средней глубиной и цветом по средней нормированной высоте
    // (navg ∈ [0,1] всегда конечен → `heat_color` не получает вырожденный range).
    let mut quads: Vec<([egui::Pos2; 4], f32, egui::Color32)> =
        Vec::with_capacity((sx - 1) * (sy - 1));
    for j in 0..sy - 1 {
        for i in 0..sx - 1 {
            let idx = [
                j * sx + i,
                j * sx + i + 1,
                (j + 1) * sx + i + 1,
                (j + 1) * sx + i,
            ];
            let corners = [
                to_screen(pts[idx[0]].0),
                to_screen(pts[idx[1]].0),
                to_screen(pts[idx[2]].0),
                to_screen(pts[idx[3]].0),
            ];
            let depth = idx.iter().map(|&k| pts[k].1).sum::<f32>() / 4.0;
            let navg = f64::from(idx.iter().map(|&k| norm[k]).sum::<f32>() / 4.0);
            quads.push((corners, depth, heat_color(navg, 0.0, 1.0)));
        }
    }
    // Painter's algorithm: дальние (МЕНЬШИЙ depth) рисуем ПЕРВЫМИ, ближние
    // (больший depth) — поверх. Depth-семантику см. в `surface::project`
    // (выше высота → больше depth → ближе к зрителю).
    quads.sort_by(|a, b| a.1.total_cmp(&b.1));

    let stroke = egui::Stroke::new(0.5, egui::Color32::from_black_alpha(60));
    for (corners, _, color) in quads {
        painter.add(egui::Shape::convex_polygon(corners.to_vec(), color, stroke));
    }
}

fn heatmap_range(scaling: Option<&CompiledScaling>, data: &[f64]) -> Option<(f64, f64)> {
    if let Some(c) = scaling {
        if let (Some(mn), Some(mx)) = (c.source.min, c.source.max) {
            if mn < mx {
                return Some((mn, mx));
            }
        }
    }
    let (mn, mx) = data
        .iter()
        .copied()
        .filter(|x| x.is_finite())
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| {
            (a.min(v), b.max(v))
        });
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
    let (r0, g0, b0) = (28u8, 50, 89); // холодный, тёмно-синий
    let (r1, g1, b1) = (75u8, 75, 60); // нейтральный, тёмно-оливковый
    let (r2, g2, b2) = (110u8, 35, 35); // горячий, тёмно-красный
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
///
/// `can_merge_with_prev` = `true` если это продолжение того же drag-а — тогда
/// undo-лог сольёт новую запись с предыдущей в один шаг (см. [`UndoLog::record`]).
fn write_back(
    rom: &mut RomImage,
    undo_log: &mut UndoLog,
    table: &ResolvedTable,
    display: &[f64],
    scaling: Option<&CompiledScaling>,
    can_merge_with_prev: bool,
) {
    let raw_back: Vec<f64> = display
        .iter()
        .map(|&v| match scaling {
            Some(s) => s.to_byte(v),
            None => v,
        })
        .collect();

    // Кодируем заранее, чтобы получить «after»-байты для undo.
    let (Some(addr), Some(st), Some(end)) =
        (table.storage_address, table.storage_type, table.endian)
    else {
        return;
    };
    let after = encode_cells(&raw_back, st, end);
    let before = match rom.read(addr, after.len()) {
        Ok(b) => b.to_vec(),
        Err(e) => {
            warn!(?e, "write-back: read failed");
            return;
        }
    };
    if let Err(e) = rom.write(addr, &after) {
        warn!(?e, "write-back failed");
        return;
    }
    undo_log.record(addr, before, after, can_merge_with_prev);
}

fn cell_speed(scaling: Option<&CompiledScaling>, precision: usize) -> f64 {
    scaling
        .and_then(|c| c.source.fine_increment)
        .unwrap_or(if precision == 0 {
            1.0
        } else {
            10f64.powi(-(precision as i32))
        })
}

fn render_switch(
    ui: &mut egui::Ui,
    rom: &mut RomImage,
    undo_log: &mut UndoLog,
    table: &ResolvedTable,
) {
    let Some(addr) = table.storage_address else {
        ui.colored_label(
            egui::Color32::YELLOW,
            "Switch table is missing storageaddress.",
        );
        return;
    };
    if table.states.is_empty() {
        ui.colored_label(
            egui::Color32::YELLOW,
            "Switch table has no <state> entries.",
        );
        return;
    }
    // Размер берём из первой state.data (после parse в байты).
    let first_size = match table.states[0].data_bytes() {
        Ok(b) if !b.is_empty() => b.len(),
        _ => {
            ui.colored_label(egui::Color32::LIGHT_RED, "Switch state[0].data malformed.");
            return;
        }
    };
    let current = match rom.read(addr, first_size) {
        Ok(b) => b.to_vec(),
        Err(e) => {
            ui.colored_label(egui::Color32::LIGHT_RED, format!("Read failed: {e}"));
            return;
        }
    };

    let mut new_data: Option<Vec<u8>> = None;
    let mut matched = false;
    ui.vertical(|ui| {
        for state in &table.states {
            let Ok(bytes) = state.data_bytes() else {
                continue;
            };
            let selected = bytes == current;
            if selected {
                matched = true;
            }
            let response = ui.radio(selected, &state.name);
            if response.clicked() && !selected {
                new_data = Some(bytes);
            }
        }
        if !matched {
            ui.colored_label(
                egui::Color32::YELLOW,
                format!(
                    "Current bytes {} don't match any state",
                    tuneforge_core::bytes::hex_dump(&current)
                ),
            );
        }
    });

    if let Some(after) = new_data {
        if rom.write(addr, &after).is_ok() {
            // Switch — discrete click, не drag. Никогда не merge с prev.
            undo_log.record(addr, current, after, false);
        }
    }
}

fn render_bitwise_switch(
    ui: &mut egui::Ui,
    rom: &mut RomImage,
    undo_log: &mut UndoLog,
    table: &ResolvedTable,
) {
    let Some(addr) = table.storage_address else {
        ui.colored_label(
            egui::Color32::YELLOW,
            "BitwiseSwitch table is missing storageaddress.",
        );
        return;
    };
    if table.bits.is_empty() {
        ui.colored_label(
            egui::Color32::YELLOW,
            "BitwiseSwitch table has no <bit> entries.",
        );
        return;
    }
    let current = match rom.read(addr, 1) {
        Ok(b) => b[0],
        Err(e) => {
            ui.colored_label(egui::Color32::LIGHT_RED, format!("Read failed: {e}"));
            return;
        }
    };

    let mut new_value = current;
    ui.vertical(|ui| {
        for bit in &table.bits {
            let Ok(pos) = bit.bit_position() else {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    format!("Invalid bit position: {}", bit.position),
                );
                continue;
            };
            if pos >= 8 {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    format!(
                        "{} — bit position {} > 7 (multi-byte bitwise not supported)",
                        bit.name, pos
                    ),
                );
                continue;
            }
            let mask = 1u8 << pos;
            let mut on = (new_value & mask) != 0;
            if ui
                .checkbox(&mut on, format!("bit {} — {}", pos, bit.name))
                .changed()
            {
                if on {
                    new_value |= mask;
                } else {
                    new_value &= !mask;
                }
            }
        }
    });

    if new_value != current && rom.write(addr, &[new_value]).is_ok() {
        // BitwiseSwitch — checkbox click, не drag. Не merge.
        undo_log.record(addr, vec![current], vec![new_value], false);
    }
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
    f[idx + 1..]
        .chars()
        .filter(|c| *c == '0' || *c == '#')
        .count()
}

fn debug_kind(k: Option<TableKind>) -> &'static str {
    match k {
        Some(TableKind::OneD) => "1D",
        Some(TableKind::TwoD) => "2D",
        Some(TableKind::ThreeD) => "3D",
        Some(TableKind::XAxis) => "X Axis",
        Some(TableKind::YAxis) => "Y Axis",
        Some(TableKind::StaticXAxis) => "Static X Axis",
        Some(TableKind::StaticYAxis) => "Static Y Axis",
        Some(TableKind::Switch) => "Switch",
        Some(TableKind::BitwiseSwitch) => "BitwiseSwitch",
        None => "?",
    }
}

fn debug_storage(s: Option<StorageType>) -> &'static str {
    match s {
        Some(StorageType::UInt8) => "uint8",
        Some(StorageType::Int8) => "int8",
        Some(StorageType::UInt16) => "uint16",
        Some(StorageType::Int16) => "int16",
        Some(StorageType::UInt32) => "uint32",
        Some(StorageType::Int32) => "int32",
        Some(StorageType::Float) => "float",
        Some(StorageType::Hex) => "hex",
        Some(StorageType::Char) => "char",
        None => "?",
    }
}

fn debug_endian(e: Option<Endian>) -> &'static str {
    match e {
        Some(Endian::Big) => "big",
        Some(Endian::Little) => "little",
        None => "?",
    }
}

/// Описание одной изменённой ячейки для окна «Changes since open».
struct ChangedCell {
    /// Подпись ячейки (`"x=3, y=5"` для 3D, `"#7"` для плоских).
    label: String,
    before_real: f64,
    after_real: f64,
}

struct ChangedTable {
    name: String,
    cells: Vec<ChangedCell>,
}

struct ChangesSummary {
    total_cells: usize,
    tables: Vec<ChangedTable>,
}

/// Найти все ячейки, отличающиеся от baseline-снапшота, и сгруппировать
/// по таблицам. Возвращает порядок таблиц — как в defs (стабильный).
fn collect_changes(rom_state: &RomState, rom_def: &ResolvedRom) -> ChangesSummary {
    let mut tables = Vec::new();
    let mut total = 0usize;
    let baseline = rom_state.original_bytes.as_slice();

    for t in &rom_def.tables {
        let Some(addr) = t.storage_address else {
            continue;
        };
        let Some(stype) = t.storage_type else {
            continue;
        };
        let stride = stype.byte_size();
        // Сколько ячеек: для 3D — size_x*size_y, иначе одна из размерностей или 1.
        let count: usize = match (t.size_x, t.size_y, t.kind) {
            (Some(x), Some(y), Some(TableKind::ThreeD)) => x as usize * y as usize,
            (Some(x), _, _) => x as usize,
            (_, Some(y), _) => y as usize,
            _ => 1,
        };
        if count == 0 {
            continue;
        }

        let scaling = compile_first_scaling(t);
        let mut cells = Vec::new();
        for idx in 0..count {
            let cell_addr = addr.raw() + (idx * stride) as u32;
            if !is_cell_modified(&rom_state.rom, Some(baseline), cell_addr, stride) {
                continue;
            }
            // Сырые байтовые значения → f64-ячейки → scaling в real units.
            let cur_raw = decode_one_at(
                rom_state.rom.raw(),
                cell_addr as usize,
                stride,
                stype,
                t.endian,
            );
            let orig_raw = decode_one_at(baseline, cell_addr as usize, stride, stype, t.endian);
            let (Some(cur), Some(orig)) = (cur_raw, orig_raw) else {
                continue;
            };
            let before_real = scaling.as_ref().map_or(orig, |c| c.to_real(orig));
            let after_real = scaling.as_ref().map_or(cur, |c| c.to_real(cur));
            let label = match (t.size_x, t.size_y, t.kind) {
                (Some(x), Some(_), Some(TableKind::ThreeD)) => {
                    let cols = x as usize;
                    format!("x={}, y={}", idx % cols, idx / cols)
                }
                _ => format!("#{idx}"),
            };
            cells.push(ChangedCell {
                label,
                before_real,
                after_real,
            });
        }
        if !cells.is_empty() {
            total += cells.len();
            tables.push(ChangedTable {
                name: t.name.clone(),
                cells,
            });
        }
    }
    ChangesSummary {
        total_cells: total,
        tables,
    }
}

/// Декодировать одну ячейку из произвольного байтового буфера по заданному
/// смещению. Возвращает `None` если буфер короче.
fn decode_one_at(
    buf: &[u8],
    offset: usize,
    stride: usize,
    stype: StorageType,
    endian: Option<Endian>,
) -> Option<f64> {
    if offset.checked_add(stride)? > buf.len() {
        return None;
    }
    let chunk = &buf[offset..offset + stride];
    let endian = endian.unwrap_or(Endian::Big);
    Some(match stype {
        StorageType::UInt8 | StorageType::Hex | StorageType::Char => f64::from(chunk[0]),
        StorageType::Int8 => f64::from(chunk[0] as i8),
        StorageType::UInt16 => {
            let arr: [u8; 2] = chunk.try_into().ok()?;
            f64::from(endian.read_u16(&arr))
        }
        StorageType::Int16 => {
            let arr: [u8; 2] = chunk.try_into().ok()?;
            f64::from(endian.read_u16(&arr) as i16)
        }
        StorageType::UInt32 => {
            let arr: [u8; 4] = chunk.try_into().ok()?;
            f64::from(endian.read_u32(&arr))
        }
        StorageType::Int32 => {
            let arr: [u8; 4] = chunk.try_into().ok()?;
            f64::from(endian.read_u32(&arr) as i32)
        }
        // Float — defs часто врут с endian; читаем BE как и `decode_one` в tuneforge-rom.
        StorageType::Float => {
            let arr: [u8; 4] = chunk.try_into().ok()?;
            f64::from(f32::from_bits(Endian::Big.read_u32(&arr)))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precision_picks_zeros_and_hashes_after_decimal() {
        assert_eq!(precision_from_format(Some("0.00")), 2);
        assert_eq!(precision_from_format(Some("#0.000")), 3);
        assert_eq!(precision_from_format(Some("0")), 0);
        assert_eq!(precision_from_format(Some("0.0##")), 3);
        assert_eq!(precision_from_format(None), 2);
    }

    #[test]
    fn heatmap_range_uses_data_when_scaling_missing() {
        let data = [1.0, 5.0, 3.0, 10.0, 2.0];
        let (mn, mx) = heatmap_range(None, &data).unwrap();
        assert_eq!(mn, 1.0);
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
        let cold = heat_color(-10.0, 0.0, 100.0);
        let cold_in = heat_color(0.0, 0.0, 100.0);
        assert_eq!(cold, cold_in, "out-of-range clamps to cold edge");
        let hot = heat_color(200.0, 0.0, 100.0);
        let hot_in = heat_color(100.0, 0.0, 100.0);
        assert_eq!(hot, hot_in, "out-of-range clamps to hot edge");
    }

    #[test]
    fn heat_color_endpoints_differ() {
        let cold = heat_color(0.0, 0.0, 1.0);
        let hot = heat_color(1.0, 0.0, 1.0);
        assert_ne!(cold, hot);
    }

    #[test]
    fn undo_log_records_and_undoes() {
        let mut log = UndoLog::default();
        let mut rom = RomImage::from_bytes(vec![0u8; 16]);
        let addr = Address::new(0);
        let before = vec![0, 0, 0, 0];
        let after = vec![1, 2, 3, 4];

        rom.write(addr, &after).unwrap();
        log.record(addr, before.clone(), after.clone(), false);

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
        let addr = Address::new(0);

        log.record(addr, vec![0], vec![1], false);
        log.undo(&mut rom);
        assert!(log.can_redo());

        log.record(addr, vec![0], vec![2], false);
        assert!(!log.can_redo(), "redo stack must clear after new record");
    }

    #[test]
    fn undo_log_caps_at_max_history() {
        let mut log = UndoLog::default();
        for i in 0..MAX_UNDO_HISTORY + 50 {
            log.record(Address::new(0), vec![i as u8], vec![(i + 1) as u8], false);
        }
        assert_eq!(log.undo.len(), MAX_UNDO_HISTORY);
    }

    #[test]
    fn undo_log_ignores_no_op() {
        let mut log = UndoLog::default();
        log.record(Address::new(0), vec![1, 2], vec![1, 2], false);
        assert!(!log.can_undo());
    }

    #[test]
    fn format_byte_integer_when_whole_number() {
        assert_eq!(format_byte(128.0), "128");
        assert_eq!(format_byte(255.0), "255");
        assert_eq!(format_byte(0.0), "0");
        assert_eq!(format_byte(-1.0), "-1");
    }

    #[test]
    fn format_byte_fractional_when_float() {
        assert_eq!(format_byte(1.5), "1.5000");
        assert_eq!(format_byte(0.001333), "0.0013");
    }

    #[test]
    fn multiple_undo_redo_steps() {
        let mut log = UndoLog::default();
        let mut rom = RomImage::from_bytes(vec![0u8; 4]);
        let addr = Address::new(0);

        rom.write(addr, &[1, 0, 0, 0]).unwrap();
        log.record(addr, vec![0, 0, 0, 0], vec![1, 0, 0, 0], false);
        rom.write(addr, &[1, 2, 0, 0]).unwrap();
        log.record(addr, vec![1, 0, 0, 0], vec![1, 2, 0, 0], false);
        rom.write(addr, &[1, 2, 3, 0]).unwrap();
        log.record(addr, vec![1, 2, 0, 0], vec![1, 2, 3, 0], false);

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

    // ── Coalescing-undo (drag merging) ──────────────────────────────

    /// Симуляция drag-сессии: 5 frame-ов подряд меняют одну ячейку
    /// 10 → 11 → 12 → 13 → 14 → 15. С `can_merge=true` всё это должно
    /// схлопнуться в **один** undo-step с before=10, after=15.
    #[test]
    fn undo_log_merges_drag_frames_into_single_step() {
        let mut log = UndoLog::default();
        let addr = Address::new(0x100);

        // Первый frame drag-а — fresh entry (can_merge=false).
        log.record(addr, vec![10], vec![11], false);
        // Frame 2..5 — продолжение drag-а, must merge.
        log.record(addr, vec![11], vec![12], true);
        log.record(addr, vec![12], vec![13], true);
        log.record(addr, vec![13], vec![14], true);
        log.record(addr, vec![14], vec![15], true);

        assert_eq!(log.undo.len(), 1, "drag-frames должны быть merge-нуты");
        let action = log.undo.back().unwrap();
        assert_eq!(action.before, vec![10]);
        assert_eq!(action.after, vec![15]);
    }

    /// `can_merge=true` но prev запись имеет **другой** адрес — не merge
    /// (это другая ячейка/таблица).
    #[test]
    fn undo_log_does_not_merge_across_addresses() {
        let mut log = UndoLog::default();
        log.record(Address::new(0x100), vec![10], vec![11], false);
        log.record(Address::new(0x200), vec![20], vec![21], true);
        assert_eq!(log.undo.len(), 2);
    }

    /// `can_merge=true` но prev.after != new.before — chain прервалась
    /// (что-то между ними изменило ROM). Не merge.
    #[test]
    fn undo_log_does_not_merge_when_chain_broken() {
        let mut log = UndoLog::default();
        let addr = Address::new(0);
        log.record(addr, vec![10], vec![11], false);
        // Здесь prev.after=11, но мы шлём before=99 — chain broken.
        log.record(addr, vec![99], vec![100], true);
        assert_eq!(log.undo.len(), 2);
    }

    /// Typed-edit (с `can_merge=false`) после drag-а — новая запись,
    /// не сливается с merged-drag-entry.
    #[test]
    fn undo_log_typed_edit_creates_fresh_entry_after_drag() {
        let mut log = UndoLog::default();
        let addr = Address::new(0);
        // Drag 10 → 15 (один merged step).
        log.record(addr, vec![10], vec![11], false);
        log.record(addr, vec![11], vec![15], true);
        assert_eq!(log.undo.len(), 1);
        // Typed edit: 15 → 42. can_merge=false — fresh entry даже если addr тот же.
        log.record(addr, vec![15], vec![42], false);
        assert_eq!(log.undo.len(), 2);
    }

    /// Merge правильно очищает redo-stack — после merge нельзя redo-нуть
    /// промежуточные значения drag-а.
    #[test]
    fn undo_log_merge_clears_redo() {
        let mut log = UndoLog::default();
        let mut rom = RomImage::from_bytes(vec![0u8; 4]);
        let addr = Address::new(0);
        log.record(addr, vec![0], vec![1], false);
        log.undo(&mut rom);
        assert!(log.can_redo());
        // Новый drag-merge должен сбросить redo.
        log.record(addr, vec![0], vec![5], false);
        log.record(addr, vec![5], vec![10], true);
        assert!(!log.can_redo(), "merge must clear redo stack");
    }
}

#[cfg(test)]
mod save_tests {
    use super::EditorPanel;

    #[test]
    fn save_rom_reports_no_rom_then_saves_in_place() {
        let path = std::env::temp_dir().join("tuneforge_save_rom_test.bin");
        std::fs::write(&path, [0xAAu8; 16]).unwrap();

        let mut panel = EditorPanel::default();
        assert!(!panel.save_rom(), "no ROM loaded → save_rom() is false");
        assert!(!panel.has_rom());

        panel.load_rom(path.clone());
        assert!(panel.has_rom());
        assert!(!panel.is_dirty(), "a freshly loaded ROM is clean");
        assert!(
            panel.save_rom(),
            "ROM loaded → saves in place, returns true"
        );
        assert!(!panel.is_dirty(), "save clears the dirty flag");
        assert_eq!(std::fs::read(&path).unwrap().len(), 16);

        let _ = std::fs::remove_file(&path);
    }
}
