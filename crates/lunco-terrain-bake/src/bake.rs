//! Carve a DEM `HeightGrid` into a drawable/collidable tile.
//!
//! The authoritative DEM keeps its **full native resolution** (Shackleton Site01
//! is 3200² at 5 m/px). Two operations produce a tile from it without throwing
//! that detail away:
//!
//! - [`crop_centered`] — extract a centred sub-region at **native resolution**
//!   (every sample preserved, exact 5 m fidelity). This is the default path: a
//!   single full-detail mesh of the whole 16 km would be ~10 M verts / ~560 MB,
//!   so M3 realizes a *region* at full res; tiled streaming (M7) covers the rest
//!   at full res too. The physics collider always uses native resolution.
//! - [`resample`] — downsample to a coarser square grid. **Lossy** — reserved
//!   for *far* visual LOD where 5 m detail is sub-pixel, never for physics or the
//!   near field.
//!
//! Both reuse `HeightGrid::to_mesh_data` / `to_avian_heights` to build the Bevy
//! mesh and avian collider. Pure, Bevy-free → unit-tested and wasm-safe; the
//! plugin runs them inside an `AsyncComputeTaskPool` task off the main thread.

use lunco_obstacle_field::field::HeightGrid;

use lunco_terrain_core::source::HeightSource;

/// Extract the centred `[-half_window, half_window]` sub-region of `grid` at its
/// **native resolution** — no decimation, every sample kept. A `half_window` ≥
/// the grid's own half-extent (or non-positive / non-finite) returns the whole
/// grid. The result is origin-centred, like all `HeightGrid`s.
pub fn crop_centered(grid: &HeightGrid, half_window: f64) -> HeightGrid {
    let n = grid.res;
    let s = grid.spacing() as f64;
    // `!half_window.is_finite() || half_window <= 0.0`, NOT `half_window <= 0.0`
    // alone: NaN compares false against everything, so the bare `<=` would let a
    // NaN window through to the `floor() as usize` below. This was previously
    // written `!(half_window > 0.0)`, which catches NaN by the same negation
    // trick but relies on the reader spotting it — and clippy's suggested
    // "simplification" of that form drops the NaN case outright.
    if !half_window.is_finite()
        || half_window <= 0.0
        || half_window >= grid.half_extent as f64
        || s <= 0.0
    {
        return grid.clone();
    }
    let k = (half_window / s).floor() as usize; // native cells each side of centre
    let centre = (n - 1) / 2;
    let lo = centre.saturating_sub(k);
    let hi = (centre + k).min(n - 1);
    let res = hi - lo + 1;
    let mut heights = vec![0.0f64; res * res];
    for (zo, z) in (lo..=hi).enumerate() {
        for (xo, x) in (lo..=hi).enumerate() {
            heights[zo * res + xo] = grid.heights[z * n + x];
        }
    }
    let half_extent = ((res - 1) as f64 * 0.5 * s) as f32;
    HeightGrid {
        res,
        half_extent,
        heights,
    }
}

/// Sample `src` into a square, origin-centred `res`×`res` grid spanning
/// `[-half_extent, half_extent]` on both X and Z (metres). Sample positions
/// coincide with the grid nodes, so resampling a grid at its own resolution is
/// exact. `res` is clamped to ≥ 2.
pub fn resample(src: &dyn HeightSource, half_extent: f64, res: usize) -> HeightGrid {
    let res = res.max(2);
    let step = (2.0 * half_extent) / (res as f64 - 1.0);
    let mut heights = vec![0.0f64; res * res];
    for iz in 0..res {
        let z = -half_extent + iz as f64 * step;
        for ix in 0..res {
            let x = -half_extent + ix as f64 * step;
            heights[iz * res + ix] = src.height_at(x, z);
        }
    }
    HeightGrid {
        res,
        half_extent: half_extent as f32,
        heights,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_terrain_core::source::AnalyticHeightSource;

    #[test]
    fn crop_keeps_native_samples() {
        // 5×5 over [-2, 2], spacing 1. Crop to ±1 → central 3×3, samples verbatim.
        let mut heights = vec![0.0f64; 25];
        for iz in 0..5 {
            for ix in 0..5 {
                heights[iz * 5 + ix] = (iz * 5 + ix) as f64;
            }
        }
        let src = HeightGrid {
            res: 5,
            half_extent: 2.0,
            heights,
        };
        let c = crop_centered(&src, 1.0);
        assert_eq!(c.res, 3);
        assert_eq!(c.half_extent, 1.0);
        // central block rows/cols 1..=3 — exact values, no interpolation.
        assert_eq!(
            c.heights,
            vec![6.0, 7.0, 8.0, 11.0, 12.0, 13.0, 16.0, 17.0, 18.0]
        );
    }

    #[test]
    fn crop_full_when_window_covers_grid() {
        let src = HeightGrid {
            res: 4,
            half_extent: 3.0,
            heights: (0..16).map(|i| i as f64).collect(),
        };
        // window ≥ extent, zero, and non-finite all return the whole grid.
        assert_eq!(crop_centered(&src, 100.0).heights, src.heights);
        assert_eq!(crop_centered(&src, 0.0).res, src.res);
        assert_eq!(crop_centered(&src, f64::INFINITY).res, src.res);
    }

    #[test]
    fn flat_source_resamples_flat() {
        let src = AnalyticHeightSource::new(0, 0.0, 100.0, 3); // zero amplitude
        let g = resample(&src, 500.0, 16);
        assert_eq!(g.res, 16);
        assert_eq!(g.half_extent, 500.0);
        assert!(g.heights.iter().all(|&h| h.abs() < 1e-9));
    }

    #[test]
    fn resampling_a_grid_at_its_own_res_is_exact() {
        // A 3×3 source over [-1, 1]; nodes at x,z ∈ {-1, 0, 1}.
        let src = HeightGrid {
            res: 3,
            half_extent: 1.0,
            heights: vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0],
        };
        let g = resample(&src, 1.0, 3);
        // Sample nodes coincide with source nodes → values reproduced exactly.
        assert_eq!(g.heights, src.heights);
    }

    #[test]
    fn downsample_keeps_corner_values() {
        // 5×5 ramp in X; downsampling to 3×3 keeps the shared corner samples.
        let mut heights = vec![0.0f64; 25];
        for iz in 0..5 {
            for ix in 0..5 {
                heights[iz * 5 + ix] = ix as f64; // 0..4 across X
            }
        }
        let src = HeightGrid {
            res: 5,
            half_extent: 2.0,
            heights,
        };
        let g = resample(&src, 2.0, 3);
        // Corners of the 3×3 land on x = -2, 0, +2 → source ix = 0, 2, 4.
        assert_eq!(g.height_at(-2.0, 0.0), 0.0);
        assert_eq!(g.height_at(0.0, 0.0), 2.0);
        assert_eq!(g.height_at(2.0, 0.0), 4.0);
    }
}
