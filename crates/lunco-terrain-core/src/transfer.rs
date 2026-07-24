//! Transfer functions — the **Transfer** stage of `Data → Transfer → Blend`.
//!
//! A [`SurfaceField`](crate::field::SurfaceField) produces a scalar; a [`TransferFn`]
//! maps that scalar to an RGBA colour. It is deliberately **pure, headless and
//! deterministic** so the SAME colour comes out everywhere the field is shown:
//!
//! - the terrain shader samples the field texture then runs this same math (its
//!   parameters are shader **uniforms**, so re-tuning a critical angle is a uniform
//!   write — no rebuild, no pipeline permutation) — `assets/shaders/transfer.wgsl`;
//! - the Inspector **legend** builds its swatches by sampling [`TransferFn::sample`]
//!   at the tick values (`lunco-sandbox-edit::ui::inspector::draw_slope_legend`);
//! - a **headless PNG / GeoTIFF export** colours a materialised field raster with it.
//!
//! Because the parameters are a few floats + colours, the whole function travels as
//! uniforms; only the field *data* is a texture. The three [`HAZARD_SAFE`] /
//! [`HAZARD_WARN`] / [`HAZARD_CLIFF`] swatches here are THE definition — the WGSL
//! module mirrors them and must be edited in lockstep. See
//! `docs/architecture/terrain-layered-rendering.md`.

/// Straight (non-premultiplied) linear RGBA in `0..1`.
pub type Rgba = [f32; 4];

fn lerp4(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
        a[3] + (b[3] - a[3]) * t,
    ]
}

/// Canonical slope-hazard swatches: **green** (traversable) → **amber** (caution) →
/// **red** (impassable).
///
/// **THIS IS THE SINGLE SOURCE OF TRUTH** for the hazard palette. `assets/shaders/
/// transfer.wgsl` mirrors these three constants (and [`hazard_color`] /
/// [`crate::derive::hazard_from_slope`]) so the terrain pixel, the Inspector legend
/// swatch, and a headless export all produce the SAME colour. Change a swatch here
/// and you MUST change it there — WGSL has no way to import a Rust const.
pub const HAZARD_SAFE: Rgba = [0.15, 0.75, 0.20, 1.0];
pub const HAZARD_WARN: Rgba = [0.95, 0.85, 0.10, 1.0];
pub const HAZARD_CLIFF: Rgba = [0.90, 0.15, 0.10, 1.0];

/// Map a hazard weight `t ∈ [0,1]` through the green→amber→red ramp. Mirrored by
/// `hazard_color` in `assets/shaders/transfer.wgsl`.
pub fn hazard_color(t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        lerp4(HAZARD_SAFE, HAZARD_WARN, t * 2.0)
    } else {
        lerp4(HAZARD_WARN, HAZARD_CLIFF, (t - 0.5) * 2.0)
    }
}

/// A value→colour mapping, closed over a handful of floats so it round-trips through
/// shader uniforms unchanged.
///
/// ONE variant, because there is one consumer: the slope-hazard overlay. Generic
/// `Ramp` / `Threshold` / `Palette` variants existed here with no consumer at all
/// (the shader's is `slope_hazard_color`, the legend's is `SlopeHazard`) — an unused
/// parallel path is worse than no path; add a variant when its consumer lands.
pub enum TransferFn {
    /// Slope traversability keyed on the two **critical angles** (radians): green
    /// below `safe_rad`, ramping to red at/above `cliff_rad`, via
    /// [`hazard_from_slope`](crate::derive::hazard_from_slope). THE live-tunable
    /// overlay — dropping the cliff angle re-reds steeper ground with no re-bake.
    SlopeHazard { safe_rad: f32, cliff_rad: f32 },
}

impl TransferFn {
    /// Colour for a field value. Pure; identical on every platform — and identical to
    /// `slope_hazard_color` in `assets/shaders/transfer.wgsl`, which is what makes the
    /// legend swatch and the terrain pixel agree by construction.
    pub fn sample(&self, v: f32) -> Rgba {
        match self {
            TransferFn::SlopeHazard {
                safe_rad,
                cliff_rad,
            } => hazard_color(crate::derive::hazard_from_slope(v, *safe_rad, *cliff_rad)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Rgba, b: Rgba) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5)
    }

    #[test]
    fn slope_hazard_greens_flat_reds_cliff() {
        let safe = 15f32.to_radians();
        let cliff = 30f32.to_radians();
        let t = TransferFn::SlopeHazard {
            safe_rad: safe,
            cliff_rad: cliff,
        };
        assert!(close(t.sample(0.0), HAZARD_SAFE)); // flat → green
        assert!(close(t.sample(cliff), HAZARD_CLIFF)); // at cliff → red
        assert!(close(t.sample(45f32.to_radians()), HAZARD_CLIFF)); // beyond → red
                                                                    // Mid-band is neither pure green nor pure red.
        let mid = t.sample(22.5f32.to_radians());
        assert!(!close(mid, HAZARD_SAFE) && !close(mid, HAZARD_CLIFF));
    }

    #[test]
    fn slope_hazard_tightening_cliff_reddens_a_fixed_slope() {
        // A 25° slope: safe under a 40° cliff, hazardous under a 26° cliff — the
        // live-tuning property (same data, new uniform → new colour).
        let s = 25f32.to_radians();
        let loose = TransferFn::SlopeHazard {
            safe_rad: 15f32.to_radians(),
            cliff_rad: 40f32.to_radians(),
        };
        let tight = TransferFn::SlopeHazard {
            safe_rad: 15f32.to_radians(),
            cliff_rad: 26f32.to_radians(),
        };
        // redder = higher R, lower G
        assert!(tight.sample(s)[0] > loose.sample(s)[0]);
        assert!(tight.sample(s)[1] < loose.sample(s)[1]);
    }

    #[test]
    fn hazard_color_ramps_green_amber_red() {
        assert!(close(hazard_color(0.0), HAZARD_SAFE));
        assert!(close(hazard_color(0.5), HAZARD_WARN));
        assert!(close(hazard_color(1.0), HAZARD_CLIFF));
    }
}
