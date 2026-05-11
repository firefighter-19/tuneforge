//! Резолвинг сырых определений: ROM- и table-наследование, scaling-refs,
//! материализация типизированных полей.
//!
//! Алгоритм:
//! 1. Для каждого ROM строим цепочку наследования через `base="…"`, ищем
//!    предков по индексу `xml_id`.
//! 2. Сливаем таблицы из всех предков (root первым, child последним) — child
//!    переопределяет одноимённые поля; nested-оси мерджатся по `name`/`kind`;
//!    непустые scalings ребёнка полностью заменяют родительские.
//! 3. Внутри собранного множества разрешаем `<table base="…">`-ссылки.
//! 4. Парсим строковые поля в типы: адреса в [`Address`], размеры в `u32`,
//!    `kind` в [`TableKind`], `storagetype` в [`StorageType`], `endian` в
//!    [`Endian`].

use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use romraider_core::{Address, Endian};
use serde::{Deserialize, Serialize};

use crate::ecu::{RomDefinition, RomId, RomsDocument, ScalingBase, ScalingRef, TableDef};
use crate::error::{DefError, DefResult};
use crate::typed::{parse_endian, StorageType, TableKind};

/// Резолв всего документа в коллекцию готовых ROM.
pub fn resolve(doc: &RomsDocument) -> DefResult<Vec<ResolvedRom>> {
    let resolver = Resolver::new(doc);
    doc.roms.iter().map(|r| resolver.resolve_rom(r)).collect()
}

/// Полностью разрешённый ROM.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedRom {
    pub xml_id: String,
    pub romid:  RomId,
    pub tables: Vec<ResolvedTable>,
}

/// Полностью разрешённая таблица. Все ссылки на base развёрнуты,
/// scaling-refs разрешены. Незаполненные поля остаются `None` —
/// downstream решает, является ли это ошибкой для конкретного use-case.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedTable {
    pub name:            String,
    pub kind:            Option<TableKind>,
    pub category:        Option<String>,
    pub storage_type:    Option<StorageType>,
    pub endian:          Option<Endian>,
    pub storage_address: Option<Address>,
    pub size_x:          Option<u32>,
    pub size_y:          Option<u32>,
    pub user_level:      Option<u8>,
    pub log_param:       Option<String>,
    pub scalings:        Vec<ResolvedScaling>,
    pub axes:            Vec<ResolvedTable>,
    pub data:            Vec<String>,
    pub description:     Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResolvedScaling {
    pub name:             Option<String>,
    pub category:         Option<String>,
    pub units:            Option<String>,
    pub expression:       Option<String>,
    pub to_byte:          Option<String>,
    pub format:           Option<String>,
    pub fine_increment:   Option<f64>,
    pub coarse_increment: Option<f64>,
    pub min:              Option<f64>,
    pub max:              Option<f64>,
}

// ------------------------------------------------------------- internals ---

struct Resolver<'a> {
    rom_index:     HashMap<&'a str, &'a RomDefinition>,
    scaling_index: HashMap<&'a str, &'a ScalingBase>,
}

impl<'a> Resolver<'a> {
    fn new(doc: &'a RomsDocument) -> Self {
        let rom_index = doc
            .roms
            .iter()
            .filter_map(|r| {
                r.romid
                    .as_ref()
                    .and_then(|id| id.xml_id.as_deref())
                    .map(|id| (id, r))
            })
            .collect();
        let scaling_index = doc
            .scaling_bases
            .iter()
            .map(|s| (s.name.as_str(), s))
            .collect();
        Self { rom_index, scaling_index }
    }

    fn resolve_rom(&self, rom: &'a RomDefinition) -> DefResult<ResolvedRom> {
        let chain = self.rom_base_chain(rom)?;

        // Сливаем таблицы — root первым (его перекрывают потомки).
        let mut name_order:    Vec<String>             = Vec::new();
        let mut tables_by_name: HashMap<String, TableDef> = HashMap::new();
        for ancestor in chain.iter().rev() {
            for raw in &ancestor.tables {
                let Some(name) = raw.name.clone() else {
                    continue; // топ-левел без имени игнорируем
                };
                if !tables_by_name.contains_key(&name) {
                    name_order.push(name.clone());
                }
                let merged = match tables_by_name.remove(&name) {
                    Some(existing) => merge_table(&existing, raw),
                    None           => raw.clone(),
                };
                tables_by_name.insert(name, merged);
            }
        }

        // Разрешаем table-level base= внутри собранного множества.
        let mut tables = Vec::with_capacity(name_order.len());
        for n in &name_order {
            let raw = &tables_by_name[n];
            let mut visiting = HashSet::new();
            let resolved_raw = self.resolve_table_base(raw, &tables_by_name, &mut visiting)?;
            tables.push(self.materialize_table(&resolved_raw)?);
        }

        let xml_id = rom
            .romid
            .as_ref()
            .and_then(|r| r.xml_id.clone())
            .unwrap_or_else(|| "<unnamed>".into());
        Ok(ResolvedRom {
            xml_id,
            romid: rom.romid.clone().unwrap_or_default(),
            tables,
        })
    }

    /// `[self, parent, grandparent, …]`.
    fn rom_base_chain(&self, rom: &'a RomDefinition) -> DefResult<Vec<&'a RomDefinition>> {
        let mut chain = vec![rom];
        let mut visited = HashSet::<String>::new();
        if let Some(id) = rom.romid.as_ref().and_then(|r| r.xml_id.clone()) {
            visited.insert(id);
        }

        let mut current = rom;
        while let Some(base_id) = current.base.as_deref() {
            if !visited.insert(base_id.to_string()) {
                return Err(DefError::Cycle {
                    kind: "rom",
                    name: base_id.to_string(),
                });
            }
            let parent = self.rom_index.get(base_id).ok_or_else(|| DefError::BaseNotFound {
                kind: "rom",
                name: base_id.to_string(),
            })?;
            chain.push(parent);
            current = parent;
        }
        Ok(chain)
    }

    fn resolve_table_base(
        &self,
        table:    &TableDef,
        ctx:      &HashMap<String, TableDef>,
        visiting: &mut HashSet<String>,
    ) -> DefResult<TableDef> {
        let Some(base_name) = table.base.as_deref() else {
            return Ok(table.clone());
        };
        if !visiting.insert(base_name.to_string()) {
            return Err(DefError::Cycle {
                kind: "table",
                name: base_name.to_string(),
            });
        }
        let base = ctx.get(base_name).ok_or_else(|| DefError::BaseNotFound {
            kind: "table",
            name: base_name.to_string(),
        })?;
        let resolved_base = self.resolve_table_base(base, ctx, visiting)?;
        visiting.remove(base_name);
        Ok(merge_table(&resolved_base, table))
    }

    fn materialize_table(&self, raw: &TableDef) -> DefResult<ResolvedTable> {
        // Имя обязательно для верхнеуровневых таблиц, но для безымянных
        // вложенных осей берём `kind` как дисплейный фолбэк — иначе резолв
        // легко падает на child-only определениях с `<table type="X Axis" .. />`.
        let name = raw
            .name
            .clone()
            .or_else(|| raw.kind.clone())
            .ok_or(DefError::MissingRequiredField("table.name"))?;

        let kind = raw.kind.as_deref().map(TableKind::from_str).transpose()?;
        let storage_type = raw
            .storage_type
            .as_deref()
            .map(StorageType::from_str)
            .transpose()?;
        let endian = raw.endian.as_deref().map(parse_endian).transpose()?;
        let storage_address = raw
            .storage_address
            .as_deref()
            .map(Address::from_str)
            .transpose()?;

        let size_x     = parse_opt_u32(raw.size_x.as_deref(),     "sizex")?;
        let size_y     = parse_opt_u32(raw.size_y.as_deref(),     "sizey")?;
        let user_level = parse_opt_u8(raw.user_level.as_deref(),  "userlevel")?;

        let scalings = raw
            .scalings
            .iter()
            .map(|s| self.materialize_scaling(s))
            .collect::<DefResult<Vec<_>>>()?;
        let axes = raw
            .nested
            .iter()
            .map(|t| self.materialize_table(t))
            .collect::<DefResult<Vec<_>>>()?;

        Ok(ResolvedTable {
            name,
            kind,
            category: raw.category.clone(),
            storage_type,
            endian,
            storage_address,
            size_x,
            size_y,
            user_level,
            log_param: raw.log_param.clone(),
            scalings,
            axes,
            data: raw.data.clone(),
            description: raw.description.clone(),
        })
    }

    fn materialize_scaling(&self, raw: &ScalingRef) -> DefResult<ResolvedScaling> {
        let mut out = ResolvedScaling::default();

        if let Some(base_name) = raw.base.as_deref() {
            let base = self.scaling_index.get(base_name).ok_or_else(|| {
                DefError::BaseNotFound { kind: "scaling", name: base_name.to_string() }
            })?;
            out.name             = Some(base.name.clone());
            out.category         = base.category.clone();
            out.units            = base.units.clone();
            out.expression       = Some(base.expression.clone());
            out.to_byte          = Some(base.to_byte.clone());
            out.format           = base.format.clone();
            out.fine_increment   = parse_opt_f64(base.fine_increment.as_deref(),   "fineincrement")?;
            out.coarse_increment = parse_opt_f64(base.coarse_increment.as_deref(), "coarseincrement")?;
            out.min              = parse_opt_f64(base.min.as_deref(),              "min")?;
            out.max              = parse_opt_f64(base.max.as_deref(),              "max")?;
        }

        // Inline-поля перекрывают то, что пришло из base.
        if raw.category.is_some()   { out.category   = raw.category.clone(); }
        if raw.units.is_some()      { out.units      = raw.units.clone(); }
        if raw.expression.is_some() { out.expression = raw.expression.clone(); }
        if raw.to_byte.is_some()    { out.to_byte    = raw.to_byte.clone(); }
        if raw.format.is_some()     { out.format     = raw.format.clone(); }
        if let Some(v) = parse_opt_f64(raw.fine_increment.as_deref(),   "fineincrement")?   { out.fine_increment   = Some(v); }
        if let Some(v) = parse_opt_f64(raw.coarse_increment.as_deref(), "coarseincrement")? { out.coarse_increment = Some(v); }
        if let Some(v) = parse_opt_f64(raw.min.as_deref(),              "min")?             { out.min              = Some(v); }
        if let Some(v) = parse_opt_f64(raw.max.as_deref(),              "max")?             { out.max              = Some(v); }

        Ok(out)
    }
}

// ------------------------------------------------------------- merging ---

fn merge_table(parent: &TableDef, child: &TableDef) -> TableDef {
    TableDef {
        kind:            or_clone(&child.kind,            &parent.kind),
        name:            or_clone(&child.name,            &parent.name),
        // base сохраняем из родителя, чтобы цепочка table→base→… не
        // обрывалась при ROM-наследовании (e.g. BASE.TBB.base="TBA", а
        // CHILD.TBB вообще без base — мы хотим всё ещё «увидеть» TBA).
        base:            or_clone(&child.base,            &parent.base),
        category:        or_clone(&child.category,        &parent.category),
        storage_type:    or_clone(&child.storage_type,    &parent.storage_type),
        endian:          or_clone(&child.endian,          &parent.endian),
        storage_address: or_clone(&child.storage_address, &parent.storage_address),
        size_x:          or_clone(&child.size_x,          &parent.size_x),
        size_y:          or_clone(&child.size_y,          &parent.size_y),
        user_level:      or_clone(&child.user_level,      &parent.user_level),
        log_param:       or_clone(&child.log_param,       &parent.log_param),
        scalings: if child.scalings.is_empty() {
            parent.scalings.clone()
        } else {
            child.scalings.clone()
        },
        nested: merge_nested(&parent.nested, &child.nested),
        data: if child.data.is_empty() {
            parent.data.clone()
        } else {
            child.data.clone()
        },
        description: or_clone(&child.description, &parent.description),
    }
}

fn merge_nested(parent: &[TableDef], child: &[TableDef]) -> Vec<TableDef> {
    let mut result: Vec<TableDef> = parent.to_vec();
    for ch in child {
        let p_idx = result.iter().position(|p| {
            match (p.name.as_deref(), ch.name.as_deref()) {
                (Some(pn), Some(cn)) => pn == cn,
                _ => p.kind.is_some() && p.kind == ch.kind,
            }
        });
        if let Some(idx) = p_idx {
            result[idx] = merge_table(&result[idx], ch);
        } else {
            result.push(ch.clone());
        }
    }
    result
}

fn or_clone<T: Clone>(a: &Option<T>, b: &Option<T>) -> Option<T> {
    a.clone().or_else(|| b.clone())
}

fn parse_opt_u32(s: Option<&str>, field: &'static str) -> DefResult<Option<u32>> {
    s.map(|v| {
        v.parse::<u32>().map_err(|e| DefError::InvalidValue {
            field,
            value:  v.to_string(),
            reason: e.to_string(),
        })
    })
    .transpose()
}

fn parse_opt_u8(s: Option<&str>, field: &'static str) -> DefResult<Option<u8>> {
    s.map(|v| {
        v.parse::<u8>().map_err(|e| DefError::InvalidValue {
            field,
            value:  v.to_string(),
            reason: e.to_string(),
        })
    })
    .transpose()
}

fn parse_opt_f64(s: Option<&str>, field: &'static str) -> DefResult<Option<f64>> {
    s.map(|v| {
        v.parse::<f64>().map_err(|e| DefError::InvalidValue {
            field,
            value:  v.to_string(),
            reason: e.to_string(),
        })
    })
    .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_str;

    #[test]
    fn child_inherits_address_from_parent_via_scaling_base() {
        let xml = r##"
            <roms>
              <scalingbase name="rpm" units="RPM" expression="x" to_byte="x" format="#" />
              <rom>
                <romid><xmlid>BASE</xmlid></romid>
                <table type="2D" name="rpm-table" storagetype="uint16" endian="big" sizex="8" storageaddress="0x1000">
                  <scaling base="rpm" />
                </table>
              </rom>
              <rom base="BASE">
                <romid><xmlid>CHILD</xmlid></romid>
                <table name="rpm-table" storageaddress="0xDEAD" />
              </rom>
            </roms>
        "##;
        let doc      = parse_str(xml).unwrap();
        let resolved = resolve(&doc).unwrap();
        let child    = resolved.iter().find(|r| r.xml_id == "CHILD").unwrap();
        assert_eq!(child.tables.len(), 1);
        let t = &child.tables[0];
        assert_eq!(t.kind,            Some(TableKind::TwoD));
        assert_eq!(t.storage_type,    Some(StorageType::UInt16));
        assert_eq!(t.endian,          Some(Endian::Big));
        assert_eq!(t.size_x,          Some(8));
        assert_eq!(t.storage_address, Some(Address::new(0xDEAD)));
        assert_eq!(t.scalings.len(),  1);
        assert_eq!(t.scalings[0].name.as_deref(),       Some("rpm"));
        assert_eq!(t.scalings[0].expression.as_deref(), Some("x"));
    }

    #[test]
    fn rom_base_chain_detects_cycle() {
        let xml = r##"
            <roms>
              <rom base="B">
                <romid><xmlid>A</xmlid></romid>
              </rom>
              <rom base="A">
                <romid><xmlid>B</xmlid></romid>
              </rom>
            </roms>
        "##;
        let doc = parse_str(xml).unwrap();
        let err = resolve(&doc).unwrap_err();
        assert!(matches!(err, DefError::Cycle { kind: "rom", .. }));
    }

    #[test]
    fn missing_scaling_base_is_reported() {
        let xml = r##"
            <roms>
              <rom>
                <romid><xmlid>R</xmlid></romid>
                <table type="2D" name="t" storageaddress="0x10">
                  <scaling base="nonexistent" />
                </table>
              </rom>
            </roms>
        "##;
        let doc = parse_str(xml).unwrap();
        let err = resolve(&doc).unwrap_err();
        assert!(matches!(err, DefError::BaseNotFound { kind: "scaling", .. }));
    }

    #[test]
    fn table_base_inherits_within_same_rom() {
        let xml = r##"
            <roms>
              <rom>
                <romid><xmlid>R</xmlid></romid>
                <table type="2D" name="A" storagetype="uint8" storageaddress="0x10">
                  <scaling units="raw" expression="x" to_byte="x" />
                </table>
                <table base="A" name="B" storageaddress="0x20" />
              </rom>
            </roms>
        "##;
        let doc  = parse_str(xml).unwrap();
        let res  = resolve(&doc).unwrap();
        let rom  = &res[0];
        let b    = rom.tables.iter().find(|t| t.name == "B").unwrap();
        assert_eq!(b.storage_type,    Some(StorageType::UInt8));
        assert_eq!(b.storage_address, Some(Address::new(0x20)));
        assert_eq!(b.scalings.len(),  1);
        assert_eq!(b.scalings[0].units.as_deref(), Some("raw"));
    }

    #[test]
    fn inline_scaling_overrides_base_fields() {
        let xml = r##"
            <roms>
              <scalingbase name="raw" units="default" expression="x" to_byte="x" />
              <rom>
                <romid><xmlid>R</xmlid></romid>
                <table type="2D" name="t" storageaddress="0x10">
                  <scaling base="raw" units="override" />
                </table>
              </rom>
            </roms>
        "##;
        let doc = parse_str(xml).unwrap();
        let res = resolve(&doc).unwrap();
        let s   = &res[0].tables[0].scalings[0];
        assert_eq!(s.units.as_deref(),      Some("override"));
        assert_eq!(s.expression.as_deref(), Some("x"));
    }

    #[test]
    fn nested_axes_merged_by_kind_when_unnamed_in_child() {
        let xml = r##"
            <roms>
              <rom>
                <romid><xmlid>BASE</xmlid></romid>
                <table type="3D" name="T" storagetype="uint16" storageaddress="0x10">
                  <table type="X Axis" name="x-named" storageaddress="0x20" storagetype="float" />
                  <table type="Y Axis" name="y-named" storageaddress="0x30" storagetype="float" />
                </table>
              </rom>
              <rom base="BASE">
                <romid><xmlid>CHILD</xmlid></romid>
                <table name="T" storageaddress="0xAAA">
                  <table type="X Axis" storageaddress="0xBBB" />
                  <table type="Y Axis" storageaddress="0xCCC" />
                </table>
              </rom>
            </roms>
        "##;
        let doc   = parse_str(xml).unwrap();
        let res   = resolve(&doc).unwrap();
        let child = res.iter().find(|r| r.xml_id == "CHILD").unwrap();
        let t     = &child.tables[0];
        assert_eq!(t.axes.len(), 2);

        let x = t.axes.iter().find(|a| a.kind == Some(TableKind::XAxis)).unwrap();
        assert_eq!(x.name, "x-named");
        assert_eq!(x.storage_address, Some(Address::new(0xBBB)));
        assert_eq!(x.storage_type, Some(StorageType::Float));

        let y = t.axes.iter().find(|a| a.kind == Some(TableKind::YAxis)).unwrap();
        assert_eq!(y.storage_address, Some(Address::new(0xCCC)));
    }
}
