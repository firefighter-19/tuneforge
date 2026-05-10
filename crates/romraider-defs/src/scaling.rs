//! Scaling — преобразование сырых байт ECU в человеко-читаемые единицы.
//!
//! В оригинале (Java) для произвольных выражений используется JEP. Здесь
//! начинаем с простого аффинного преобразования `y = a*x + b`, которое
//! покрывает 90% таблиц. Остальное в TODO до подключения парсера выражений.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scaling {
    pub name:     String,
    pub units:    String,
    pub format:   String,
    pub to_real:  Expression,
    pub to_byte:  Expression,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Expression {
    /// `y = a*x + b`
    Linear { a: f64, b: f64 },
    /// Произвольное выражение с переменной `x`. Парсится при загрузке
    /// определения; вычисляется через `meval` или `evalexpr` (TODO).
    Raw(String),
}

impl Expression {
    #[must_use]
    pub fn evaluate(&self, x: f64) -> f64 {
        match self {
            Expression::Linear { a, b } => a * x + b,
            Expression::Raw(_) => f64::NAN, // TODO: подключить evalexpr
        }
    }
}
