//! Transfer functions — the **Transfer** stage of `Data → Transfer → Blend`.
//!
//! A [`SurfaceField`](crate::field::SurfaceField) produces a scalar; a [`TransferFn`]
//! maps that scalar to an RGBA colour. It is deliberately **pure, headless and
//! deterministic** so the SAME colour comes out everywhere the field is shown:
//!
//! - the terrain shader samples the field texture then runs this same math (its
//!   parameters are shader **uniforms**, so re-tuning a critical angle is a uniform
//!   write — no rebuild, no pipeline permutation);
//! - a **legend** builds its swatches by sampling this at the tick values;
//! - a **headless PNG / GeoTIFF export** colours a materialised field raster with it.
//!
//! Because the parameters are a few floats + colours, the whole function travels as
//! uniforms; only the field *data* is a texture. See
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
/// **red** (impassable). Shared by the shader default and the legend so they match.
pub const HAZARD_SAFE: Rgba = [0.15, 0.75, 0.20, 1.0];
pub const HAZARD_WARN: Rgba = [0.95, 0.85, 0.10, 1.0];
pub const HAZARD_CLIFF: Rgba = [0.90, 0.15, 0.10, 1.0];

/// Map a hazard weight `t ∈ [0,1]` through the green→amber→red ramp.
pub fn hazard_color(t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        lerp4(HAZARD_SAFE, HAZARD_WARN, t * 2.0)
    } else {
        lerp4(HAZARD_WARN, HAZARD_CLIFF, (t - 0.5) * 2.0)
    }
}

/// A value→colour mapping. Every variant is closed over a handful of floats/colours
/// so it round-trips through shader uniforms unchanged.
pub enum TransferFn {
    /// Linear ramp: `v` in `[lo, hi]` → `t` in `[0,1]` → `lerp(c_lo, c_hi)`, clamped
    /// outside. The generic scalar colouriser (elevation hypsometry, AO, …).
    Ramp { lo: f32, hi: f32, c_lo: Rgba, c_hi: Rgba },
    /// Hard cut at `at`: `v < at` → `below`, else `above`. A binary go/no-go mask.
    Threshold { at: f32, below: Rgba, above: Rgba },
    /// Slope traversability keyed on the two **critical angles** (radians): green
    /// below `safe_rad`, ramping to red at/above `cliff_rad`, via
    /// [`hazard_from_slope`](crate::derive::hazard_from_slope). THE live-tunable
    /// overlay — dropping the cliff angle re-reds steeper ground with no re-bake.
    SlopeHazard { safe_rad: f32, cliff_rad: f32 },
    /// Piecewise ramp over sorted `(stop, colour)` knots — lerp between neighbours,
    /// clamp past the ends. For arbitrary colormaps (a mineral palette, hypsometry).
    /// `stops` MUST be ascending by `stop`; an empty list yields transparent black.
    Palette { stops: Vec<(f32, Rgba)> },
}

impl TransferFn {
    /// Colour for a field value. Pure; identical on every platform.
    pub fn sample(&self, v: f32) -> Rgba {
        match self {
            TransferFn::Ramp { lo, hi, c_lo, c_hi } => {
                let t = if (hi - lo).abs() < 1e-9 {
                    if v >= *hi { 1.0 } else { 0.0 }
                } else {
                    (v - lo) / (hi - lo)
                };
                lerp4(*c_lo, *c_hi, t)
            }
            TransferFn::Threshold { at, below, above } => {
                if v < *at {
                    *below
                } else {
                    *above
                }
            }
            TransferFn::SlopeHazard { safe_rad, cliff_rad } => {
                hazard_color(crate::derive::hazard_from_slope(v, *safe_rad, *cliff_rad))
            }
            TransferFn::Palette { stops } => sample_palette(stops, v),
        }
    }
}

/// Piecewise-linear lookup over ascending `(stop, colour)` knots.
fn sample_palette(stops: &[(f32, Rgba)], v: f32) -> Rgba {
    match stops {
        [] => [0.0, 0.0, 0.0, 0.0],
        [(_, c)] => *c,
        _ => {
            if v <= stops[0].0 {
                return stops[0].1;
            }
            let last = stops[stops.len() - 1];
            if v >= last.0 {
                return last.1;
            }
            // Find the bracket `[i, i+1]` with stop_i <= v < stop_{i+1}.
            for w in stops.windows(2) {
                let (s0, c0) = w[0];
                let (s1, c1) = w[1];
                if v >= s0 && v <= s1 {
                    let t = if (s1 - s0).abs() < 1e-9 { 0.0 } else { (v - s0) / (s1 - s0) };
                    return lerp4(c0, c1, t);
                }
            }
            last.1 // unreachable given the ascending guarantee, but total
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLACK: Rgba = [0.0, 0.0, 0.0, 1.0];
    const WHITE: Rgba = [1.0, 1.0, 1.0, 1.0];

    fn close(a: Rgba, b: Rgba) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5)
    }

    #[test]
    fn ramp_endpoints_and_mid_and_clamp() {
        let t = TransferFn::Ramp { lo: 0.0, hi: 10.0, c_lo: BLACK, c_hi: WHITE };
        assert!(close(t.sample(0.0), BLACK));
        assert!(close(t.sample(10.0), WHITE));
        assert!(close(t.sample(5.0), [0.5, 0.5, 0.5, 1.0]));
        assert!(close(t.sample(-3.0), BLACK)); // clamp below
        assert!(close(t.sample(99.0), WHITE)); // clamp above
    }

    #[test]
    fn ramp_degenerate_range_is_a_step() {
        let t = TransferFn::Ramp { lo: 4.0, hi: 4.0, c_lo: BLACK, c_hi: WHITE };
        assert!(close(t.sample(3.9), BLACK));
        assert!(close(t.sample(4.0), WHITE));
    }

    #[test]
    fn threshold_splits_below_above() {
        let t = TransferFn::Threshold { at: 1.0, below: BLACK, above: WHITE };
        assert!(close(t.sample(0.99), BLACK));
        assert!(close(t.sample(1.0), WHITE));
    }

    #[test]
    fn slope_hazard_greens_flat_reds_cliff() {
        let safe = 15f32.to_radians();
        let cliff = 30f32.to_radians();
        let t = TransferFn::SlopeHazard { safe_rad: safe, cliff_rad: cliff };
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
        let loose = TransferFn::SlopeHazard { safe_rad: 15f32.to_radians(), cliff_rad: 40f32.to_radians() };
        let tight = TransferFn::SlopeHazard { safe_rad: 15f32.to_radians(), cliff_rad: 26f32.to_radians() };
        // redder = higher R, lower G
        assert!(tight.sample(s)[0] > loose.sample(s)[0]);
        assert!(tight.sample(s)[1] < loose.sample(s)[1]);
    }

    #[test]
    fn palette_interpolates_and_clamps() {
        let stops = vec![(0.0, BLACK), (1.0, [1.0, 0.0, 0.0, 1.0]), (2.0, WHITE)];
        let t = TransferFn::Palette { stops };
        assert!(close(t.sample(-1.0), BLACK)); // clamp low
        assert!(close(t.sample(0.5), [0.5, 0.0, 0.0, 1.0]));
        assert!(close(t.sample(1.0), [1.0, 0.0, 0.0, 1.0])); // exact knot
        assert!(close(t.sample(1.5), [1.0, 0.5, 0.5, 1.0]));
        assert!(close(t.sample(9.0), WHITE)); // clamp high
    }

    #[test]
    fn palette_edge_cases() {
        assert!(close(TransferFn::Palette { stops: vec![] }.sample(3.0), [0.0, 0.0, 0.0, 0.0]));
        let one = TransferFn::Palette { stops: vec![(5.0, WHITE)] };
        assert!(close(one.sample(-2.0), WHITE) && close(one.sample(9.0), WHITE));
    }

    #[test]
    fn hazard_color_ramps_green_amber_red() {
        assert!(close(hazard_color(0.0), HAZARD_SAFE));
        assert!(close(hazard_color(0.5), HAZARD_WARN));
        assert!(close(hazard_color(1.0), HAZARD_CLIFF));
    }
}
