// Transfer — the value→colour plane of the terrain's Data → Transfer → Blend
// pipeline, shader side.
//
// This is the GPU twin of `lunco_terrain_core::transfer` (Rust). A field value
// (slope, elevation, …) carries no colour of its own; a TRANSFER assigns one.
// Keeping the transfer in its own module means the on-screen pixel, the
// Inspector's legend swatch (`TransferFn::SlopeHazard.sample`) and a headless
// export all run the SAME ramp — a legend that disagrees with the terrain it
// explains is a bug, and the only way to guarantee it can't happen is to have
// one definition per side and no copies.
//
// Every parameter arrives as a uniform, so re-tuning a critical angle is a
// uniform write: no re-bake, no pipeline permutation. Keep the constants below
// in lockstep with `HAZARD_SAFE`/`HAZARD_WARN`/`HAZARD_CLIFF` in transfer.rs.

#define_import_path lunco::transfer

/// Green (traversable) → amber (caution) → red (impassable).
const HAZARD_SAFE:  vec3<f32> = vec3(0.15, 0.75, 0.20);
const HAZARD_WARN:  vec3<f32> = vec3(0.95, 0.85, 0.10);
const HAZARD_CLIFF: vec3<f32> = vec3(0.90, 0.15, 0.10);

/// Traversability hazard in `[0,1]` from a slope angle (radians): `0` below
/// `safe_rad`, smoothstepping to `1` at/above `cliff_rad`. Mirrors
/// `lunco_terrain_core::derive::hazard_from_slope`.
fn hazard_from_slope(slope_rad: f32, safe_rad: f32, cliff_rad: f32) -> f32 {
    let lo = min(safe_rad, cliff_rad);
    let hi = max(safe_rad, cliff_rad);
    if (hi - lo < 1e-6) {
        return select(0.0, 1.0, slope_rad >= hi);
    }
    let t = clamp((slope_rad - lo) / (hi - lo), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t); // smoothstep
}

/// The hazard ramp: `0` → green, `0.5` → amber, `1` → red. Mirrors
/// `lunco_terrain_core::transfer::hazard_color`.
fn hazard_color(t: f32) -> vec3<f32> {
    if (t < 0.5) {
        return mix(HAZARD_SAFE, HAZARD_WARN, t * 2.0);
    }
    return mix(HAZARD_WARN, HAZARD_CLIFF, (t - 0.5) * 2.0);
}

/// `TransferFn::SlopeHazard` end to end: slope angle (radians) → hazard colour.
/// The surface normal must be the GEOMETRIC one — the bump-perturbed normal
/// speckles the overlay into micro-noise, since a hazard map answers "can the
/// rover climb this hillside", not "is this pebble tilted".
fn slope_hazard_color(slope_rad: f32, safe_rad: f32, cliff_rad: f32) -> vec3<f32> {
    return hazard_color(hazard_from_slope(slope_rad, safe_rad, cliff_rad));
}

/// Slope angle (radians) of a world-space geometric normal. `0` = flat ground,
/// `π/2` = a vertical wall.
fn slope_of(n_geo: vec3<f32>) -> f32 {
    return acos(clamp(n_geo.y, -1.0, 1.0));
}
