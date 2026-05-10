//! Модель табличных данных в ROM. 1D / 2D / 3D — с применённым scaling.

use romraider_core::Address;
use romraider_defs::TableDef;

use crate::error::RomResult;
use crate::image::RomImage;

#[derive(Debug, Clone)]
pub struct Table<'def> {
    pub definition: &'def TableDef,
    pub address:    Address,
    pub values:     TableValues,
}

#[derive(Debug, Clone)]
pub enum TableValues {
    OneD(Vec<f64>),
    TwoD { x: Vec<f64>, data: Vec<f64> },
    ThreeD { x: Vec<f64>, y: Vec<f64>, data: Vec<f64> },
    Constant(f64),
    Switch(Vec<u8>),
}

impl<'def> Table<'def> {
    /// Прочитать таблицу из ROM по её определению.
    ///
    /// TODO: применить scaling (`def.scaling.to_real`) и осевые
    /// преобразования. Сейчас — каркас, фиксирующий публичный контракт.
    pub fn load(_def: &'def TableDef, _rom: &RomImage) -> RomResult<Self> {
        unimplemented!("table loading wires up to scaling + axes")
    }
}
