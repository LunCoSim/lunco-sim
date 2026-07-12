//! Crater-layer geometry stamping — the pure, deterministic core shared by the
//! native async bake task and the wasm Web Worker.
//!
//! Moved out of `lunco-terrain-surface::terrain` so the worker can call the SAME
//! code without linking Bevy: placement is derived from the spec seed, so every
//! peer (and the worker) regenerates identical basins with nothing to transfer.

use lunco_obstacle_field::field::HeightGrid;
use lunco_obstacle_field::sampler::{salt, sample_layer, Placement};
use lunco_obstacle_field::spec::{CraterLayer, Pattern};

/// Stamp the [`CraterLayer`] into a DEM working grid as REAL geometry — so the
/// large basins appear in BOTH the streamed visual mesh AND the heightfield
/// collider (you can drive into them).
///
/// **Non-destructive** (only the in-memory working copy is touched) and
/// **deterministic** (the seed drives placement). Craters fill the WHOLE DEM
/// window (`grid.half_extent`). Placement is **blue-noise** with a `min_spacing`
/// derived from crater size + density: `stamp_crater` is additive, so uniform
/// overlap stacks rims into spikes and sums bowls into bottomless holes. Returns
/// the count stamped. Pure → safe to call off-thread / in a worker.
pub fn stamp_spec_craters(grid: &mut HeightGrid, craters: &CraterLayer, seed: u64) -> usize {
    let placements = crater_placements(craters, seed, grid.half_extent);
    grid.stamp_craters(&placements, craters);
    placements.len()
}

/// The deterministic crater placements for a terrain of the given `half_extent` —
/// the SAME set [`stamp_spec_craters`] rasterises into the grid, so the dedicated
/// high-fidelity crater mesh (the craters layer's overlay) lands exactly on the
/// stamped basins. Blue-noise `min_spacing` derived from crater size + density
/// (NOT the spec pattern): a 3 m Poisson over an 8 km window would blow up.
pub fn crater_placements(craters: &CraterLayer, seed: u64, half_extent: f32) -> Vec<Placement> {
    if !craters.enabled || craters.density <= 0.0 {
        return Vec::new();
    }
    /// `density` is craters per hectare; convert to a count over the window area.
    const M2_PER_HECTARE: f64 = 10_000.0;
    /// Blue-noise spacing as a fraction of the mean nearest-neighbour pitch (leaves
    /// room to jitter without clumping).
    const PITCH_FRACTION: f32 = 0.7;
    /// Absolute floor (m) on crater centre spacing, so a dense field of tiny
    /// craters can't stack rims into spikes.
    const MIN_SPACING_FLOOR_M: f32 = 6.0;
    /// Keep centres at least this many crater-diameters apart (`2 × radius mode`).
    const DIAMETERS_APART: f32 = 2.0;

    let side = (2.0 * half_extent) as f64;
    let count = ((craters.density as f64 * side * side) / M2_PER_HECTARE).round().max(0.0) as usize;
    if count == 0 {
        return Vec::new();
    }
    let pitch = (side / count.max(1) as f64).sqrt() as f32 * PITCH_FRACTION;
    let min_spacing = (craters.size.mode * DIAMETERS_APART).max(pitch).max(MIN_SPACING_FLOOR_M);
    sample_layer(
        seed,
        salt::CRATERS,
        Pattern::PoissonDisk { min_spacing },
        half_extent,
        count,
        craters.size,
        0.0,
    )
}
