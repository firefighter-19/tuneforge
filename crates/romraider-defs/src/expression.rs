//! Компиляция scaling-формул в считающие функции.
//!
//! XML хранит `expression="x*.001333224"` и обратное `to_byte="x/.001333224"`
//! как строки. Здесь они парсятся через [`meval`] в `meval::Expr` один раз и
//! потом вычисляются при каждом запросе байт→real или real→байт.
//!
//! Для всех формул из апстрим-фикстур (rpm, BoostTarget_*, WastegateDutyCycle_%,
//! `(x*1.8)+32`, `(-x*.000224304213)+7.350001`) meval работает напрямую без
//! препроцессинга.

use crate::error::{DefError, DefResult};
use crate::resolve::ResolvedScaling;

/// Скомпилированная пара формул для одного scaling-а.
#[derive(Debug, Clone)]
pub struct CompiledScaling {
    /// Исходная сырая запись — удобно для UI и логов.
    pub source: ResolvedScaling,
    to_real_expr: meval::Expr,
    to_byte_expr: meval::Expr,
}

impl CompiledScaling {
    /// Преобразовать сырой байт-параметр в реальное значение.
    ///
    /// Возвращает `NaN`, если при вычислении произошла ошибка (например,
    /// деление на ноль в формуле обратного преобразования).
    #[must_use]
    pub fn to_real(&self, byte: f64) -> f64 {
        eval_with_x(&self.to_real_expr, byte)
    }

    /// Обратное преобразование: реальное значение → байт-репрезентация.
    #[must_use]
    pub fn to_byte(&self, real: f64) -> f64 {
        eval_with_x(&self.to_byte_expr, real)
    }
}

impl ResolvedScaling {
    /// Скомпилировать `expression` и `to_byte` в [`CompiledScaling`].
    ///
    /// Парсит обе формулы и прогоняет sanity-eval с `x=0.0`, чтобы выловить
    /// ошибки в фикстуре на этапе загрузки, а не при первом отображении ячейки.
    pub fn compile(&self) -> DefResult<CompiledScaling> {
        let expr_str = self
            .expression
            .as_deref()
            .ok_or(DefError::MissingRequiredField("scaling.expression"))?;
        let to_byte_str = self
            .to_byte
            .as_deref()
            .ok_or(DefError::MissingRequiredField("scaling.to_byte"))?;

        let to_real_expr = parse(expr_str)?;
        let to_byte_expr = parse(to_byte_str)?;

        // Sanity-проверка: формула не падает при подстановке известного x.
        // (Если падает — это валидная ошибка определения, но мы хотим её сейчас.)
        eval(&to_real_expr, 0.0).map_err(|m| DefError::InvalidScaling {
            expr:    expr_str.to_string(),
            message: m,
        })?;
        eval(&to_byte_expr, 0.0).map_err(|m| DefError::InvalidScaling {
            expr:    to_byte_str.to_string(),
            message: m,
        })?;

        Ok(CompiledScaling {
            source:       self.clone(),
            to_real_expr,
            to_byte_expr,
        })
    }
}

// ----------------------------------------------------------- internals ---

/// Вставляет `0` перед точкой в `.84` → `0.84`. Апстрим-определения часто
/// пишут константы без ведущего нуля (`x*.001333224`), но meval парсит
/// только канонические числа.
fn normalize_decimal(s: &str) -> String {
    let mut out  = String::with_capacity(s.len() + 4);
    let mut prev: Option<char> = None;
    let chars: Vec<char> = s.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c == '.' {
            let next_is_digit = chars.get(i + 1).is_some_and(|c| c.is_ascii_digit());
            let prev_is_digit = prev.is_some_and(|c: char| c.is_ascii_digit());
            if next_is_digit && !prev_is_digit {
                out.push('0');
            }
        }
        out.push(c);
        prev = Some(c);
    }
    out
}

fn parse(s: &str) -> DefResult<meval::Expr> {
    let normalized = normalize_decimal(s);
    normalized
        .parse::<meval::Expr>()
        .map_err(|e| DefError::InvalidScaling {
            expr:    s.to_string(),
            message: e.to_string(),
        })
}

fn eval(expr: &meval::Expr, x: f64) -> Result<f64, String> {
    let mut ctx = meval::Context::new();
    ctx.var("x", x);
    expr.eval_with_context(ctx).map_err(|e| e.to_string())
}

fn eval_with_x(expr: &meval::Expr, x: f64) -> f64 {
    eval(expr, x).unwrap_or(f64::NAN)
}

/// Скомпилировать выражение из `log_defs.xml`-формата. В отличие от
/// scaling-формул в ROM-определениях, логгер использует `[value]` как
/// плейсхолдер сырого значения. Здесь мы подменяем `[value]` на `(x)`
/// и дальше используем общий [`parse`] / [`eval_with_x`] pipeline.
///
/// Поддерживается без переменных, кроме `[value]` (= `x`). `GetLogParam("…")`
/// и другие cross-references пока не поддерживаются — соответствующие формулы
/// упадут при компиляции, и параметр придётся пропустить.
pub fn compile_log_expr(expr_str: &str) -> DefResult<meval::Expr> {
    let substituted = expr_str.replace("[value]", "(x)");
    parse(&substituted)
}

/// Eval скомпилированного log-выражения. Возвращает `NaN` при ошибке.
#[must_use]
pub fn eval_log_expr(expr: &meval::Expr, value: f64) -> f64 {
    eval_with_x(expr, value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scaling(expression: &str, to_byte: &str) -> ResolvedScaling {
        ResolvedScaling {
            name:             None,
            category:         None,
            units:            None,
            expression:       Some(expression.into()),
            to_byte:          Some(to_byte.into()),
            format:           None,
            fine_increment:   None,
            coarse_increment: None,
            min:              None,
            max:              None,
        }
    }

    #[test]
    fn identity_round_trip() {
        let c = scaling("x", "x").compile().unwrap();
        assert_eq!(c.to_real(42.0), 42.0);
        assert_eq!(c.to_byte(42.0), 42.0);
    }

    #[test]
    fn fahrenheit_celsius_round_trip() {
        // (x*1.8)+32 — F из C. Обратно: (x-32)/1.8.
        let c = scaling("(x*1.8)+32", "(x-32)/1.8").compile().unwrap();
        let r = c.to_real(100.0);            // 100°C → 212°F
        let b = c.to_byte(r);                // 212 → 100
        assert!((r - 212.0).abs() < 1e-9);
        assert!((b - 100.0).abs() < 1e-9);
    }

    #[test]
    fn boost_target_bar_absolute_round_trip() {
        // Из апстрим-фикстуры: BoostTarget_barabsolute.
        let c = scaling("x*.001333224", "x/.001333224").compile().unwrap();
        let raw  = 1500.0_f64;
        let real = c.to_real(raw);
        let back = c.to_byte(real);
        assert!((real - 1500.0 * 0.001333224).abs() < 1e-9);
        assert!((back - raw).abs() < 1e-6);
    }

    #[test]
    fn formula_with_leading_negative_decimal_parses() {
        // Из апстрим-фикстуры CL Fueling: (-x*.000224304213)+7.350001
        let c = scaling("(-x*.000224304213)+7.350001", "(x-7.350001)/-.000224304213")
            .compile()
            .unwrap();
        let real = c.to_real(1000.0);
        assert!((real - ((-1000.0 * 0.000224304213) + 7.350001)).abs() < 1e-12);
        // Round-trip с допуском (формула содержит деление на маленькое число).
        let back = c.to_byte(real);
        assert!((back - 1000.0).abs() < 1e-3);
    }

    #[test]
    fn psi_relative_sea_level_formula_works() {
        // (x-760)*.01933677 — давление относительно стандартной атмосферы.
        let c = scaling("(x-760)*.01933677", "(x/.01933677)+760").compile().unwrap();
        let real = c.to_real(800.0);
        assert!((real - ((800.0 - 760.0) * 0.01933677)).abs() < 1e-9);
    }

    #[test]
    fn unparseable_expression_returns_invalid_scaling() {
        let err = scaling("x +& 2", "x").compile().unwrap_err();
        assert!(matches!(err, DefError::InvalidScaling { .. }));
    }

    #[test]
    fn missing_expression_returns_missing_field() {
        let mut s = scaling("x", "x");
        s.expression = None;
        let err = s.compile().unwrap_err();
        assert!(matches!(err, DefError::MissingRequiredField("scaling.expression")));
    }

    #[test]
    fn compile_log_expr_handles_value_placeholder() {
        let e = compile_log_expr("[value]/4").unwrap();
        assert!((eval_log_expr(&e, 100.0) - 25.0).abs() < 1e-9);

        let e = compile_log_expr("([value]-128)*100/128").unwrap();
        assert!((eval_log_expr(&e, 0.0)   - (-100.0)).abs() < 1e-9);
        assert!((eval_log_expr(&e, 128.0) -    0.0  ).abs() < 1e-9);
        assert!((eval_log_expr(&e, 256.0) -  100.0  ).abs() < 1e-9);
    }

    #[test]
    fn compile_log_expr_supports_leading_dot_decimals() {
        // Такие же formula-quirks как у ROM-defs.
        let e = compile_log_expr("[value]*.06894757").unwrap();
        assert!((eval_log_expr(&e, 100.0) - 6.894757).abs() < 1e-6);
    }

    #[test]
    fn compile_log_expr_invalid_returns_error() {
        let err = compile_log_expr("[value] +& 1").unwrap_err();
        assert!(matches!(err, DefError::InvalidScaling { .. }));
    }

    #[test]
    fn normalize_decimal_inserts_leading_zero() {
        assert_eq!(normalize_decimal("x*.84"),       "x*0.84");
        assert_eq!(normalize_decimal(".84"),         "0.84");
        assert_eq!(normalize_decimal("0.84"),        "0.84");      // не трогаем
        assert_eq!(normalize_decimal("1.5+.5"),      "1.5+0.5");
        assert_eq!(normalize_decimal("/-.0002"),     "/-0.0002");
        assert_eq!(normalize_decimal("x"),           "x");
    }
}
