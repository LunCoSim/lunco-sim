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
    let side = (2.0 * half_extent) as f64;
    let count = ((craters.density as f64 * side * side) / 10_000.0).round().max(0.0) as usize;
    if count == 0 {
        return Vec::new();
    }
    let pitch = (side / count.max(1) as f64).sqrt() as f32 * 0.7;
    let min_spacing = (craters.size.mode * 2.0).max(pitch).max(6.0);
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
