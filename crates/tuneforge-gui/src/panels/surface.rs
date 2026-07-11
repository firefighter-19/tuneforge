//! 3D-surface: чистая математика проекции для просмотрщика карт.
//!
//! Рендер (egui `Painter` + заливка `heat_color`) живёт в `editor.rs`; здесь
//! только геометрия, чтобы её можно было юнит-тестировать без UI-контекста.

/// Углы обзора поверхности (радианы): `yaw` — поворот вокруг вертикали,
/// `pitch` — наклон к зрителю.
#[derive(Debug, Clone, Copy)]
pub struct SurfaceView {
    pub yaw: f32,
    pub pitch: f32,
}

impl Default for SurfaceView {
    fn default() -> Self {
        // Слегка сбоку и сверху — типовой стартовый ракурс.
        Self {
            yaw: 0.6,
            pitch: 0.9,
        }
    }
}

/// Ортографическая проекция точки `(gx, gy, gz)` (где `gy` — высота/значение)
/// при углах `yaw`/`pitch`. Возвращает `(screen_x, screen_y, depth)`: экранные
/// координаты (экранный `y` растёт вниз, поэтому высота уходит в минус) и
/// `depth`, где БО́ЛЬШИЙ `depth` = БЛИЖЕ к зрителю. Для back-to-front рисуйте
/// по ВОЗРАСТАНИЮ `depth` (дальние — меньший depth — первыми).
///
/// Порядок: поворот плоскости вокруг вертикали (`yaw`), затем наклон к зрителю
/// (`pitch` вокруг горизонтальной оси), затем ортографический сброс глубины.
#[must_use]
pub fn project(gx: f32, gy: f32, gz: f32, yaw: f32, pitch: f32) -> (f32, f32, f32) {
    let (sa, ca) = yaw.sin_cos();
    let x1 = gx * ca + gz * sa;
    let z1 = -gx * sa + gz * ca;
    let y1 = gy;

    let (sb, cb) = pitch.sin_cos();
    let y2 = y1 * cb - z1 * sb;
    let z2 = y1 * sb + z1 * cb;

    (x1, -y2, z2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::FRAC_PI_2;

    fn approx(a: f32, b: f32) {
        assert!((a - b).abs() < 1e-5, "{a} != {b}");
    }

    #[test]
    fn identity_projection_passes_through_with_flipped_height() {
        // yaw=0, pitch=0: (gx, gy=height, gz) → (gx, -gy, gz).
        let (sx, sy, d) = project(0.5, 0.25, -0.5, 0.0, 0.0);
        approx(sx, 0.5);
        approx(sy, -0.25); // высота вверх = экранный y вниз
        approx(d, -0.5);
    }

    #[test]
    fn yaw_90_swaps_plane_axes() {
        // gx уходит в глубину, gz — на экран: project(1,0,0, 90°,0) ≈ (0,0,-1).
        let (sx, sy, d) = project(1.0, 0.0, 0.0, FRAC_PI_2, 0.0);
        approx(sx, 0.0);
        approx(sy, 0.0);
        approx(d, -1.0);
    }

    #[test]
    fn pitch_90_tilts_height_fully_into_depth() {
        // При наклоне 90° высота полностью уходит в глубину: (0,1,0) → depth=1.
        let (sx, sy, d) = project(0.0, 1.0, 0.0, 0.0, FRAC_PI_2);
        approx(sx, 0.0);
        approx(sy, 0.0);
        approx(d, 1.0);
    }

    #[test]
    fn taller_points_have_larger_depth_ie_nearer() {
        // Depth-семантика, на которую опирается back-to-front сортировка в
        // draw_surface: бо́льшая высота (gy) → бо́льший depth (ближе к зрителю).
        // Если знак глубины в project поменяют — сортировка молча сломается.
        let v = SurfaceView::default();
        let low = project(0.0, 0.0, 0.0, v.yaw, v.pitch).2;
        let high = project(0.0, 1.0, 0.0, v.yaw, v.pitch).2;
        assert!(high > low, "higher point must be nearer: {high} !> {low}");
    }
}
