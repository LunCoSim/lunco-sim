// Lunar regolith photometry, shared by the terrain shaders (regolith.wgsl,
// terrain_shadow.wgsl) via naga_oil import.
//
// The Moon does not obey Cook-Torrance/Lambert: lunar soil is a porous,
// sub-wavelength-grained, **retroreflective** powder. Two corrections carry
// most of the realism, and both are pure geometry (no textures, no bake — so
// they work identically on the shadow-less web build):
//
//   * Lommel-Seeliger limb behaviour — diffuse ∝ μ₀/(μ₀+μ) rather than Lambert
//     μ₀, which cancels most of Lambert's limb darkening (the real Moon stays
//     bright to the limb; this is why the full Moon looks like a flat disc).
//   * Opposition surge / heiligenschein — the surface brightens sharply toward
//     zero phase angle (camera looking down the sun vector): a broad
//     shadow-hiding term (<~20°) plus a narrow coherent-backscatter spike
//     (<~3°). Lambert/GGX has no term that brightens toward the *light*.
//
// We apply this as a multiplier on `base_color` *before* bevy's
// `apply_pbr_lighting`. Bevy's built-in Lambert then multiplies by μ₀, so the
// net sun diffuse becomes  albedo · μ₀/(μ₀+μ) · B(α)  — exactly
// Lommel-Seeliger × opposition. The factor is geometry-only and clamped, and
// the final diffuse stays bounded (the μ₀ numerator → 0 at the terminator),
// so a large factor never produces fireflies. Ambient/specular ride the same
// `base_color`, but on an airless body ambient ≈ 0 and dielectric F0 is fixed
// (reflectance 0.5), so the side effects are negligible.
//
// Constants are conservative first-cut values; promote to `//!@ui` params for
// live maria/highlands tuning (highlands back-scatter more) as a follow-up.

#define_import_path lunco::lunar

// Hapke-style opposition surge B(α), phase angle `alpha` in radians.
//   b0   amplitude      (~0.8)
//   h_sh shadow-hiding  (~0.06 rad ≈ 3.5°, broad)
//   h_cb coherent       (~0.02 rad ≈ 1.2°, narrow spike)
fn opposition_surge(alpha: f32) -> f32 {
    let b0 = 0.8;
    let h_sh = 0.06;
    let h_cb = 0.02;
    let t = tan(alpha * 0.5);
    let shoe = b0 / (1.0 + t / h_sh);
    let cboe = (b0 * 0.5) / (1.0 + t / h_cb);
    return 1.0 + shoe + cboe;
}

// Multiplier applied to linear albedo so bevy's Lambert (·μ₀) completes a
// Lommel-Seeliger × opposition response for the dominant sun.
//   N  shading normal   (world, unit)
//   L  to-sun direction (world, unit)
//   V  to-camera        (world, unit)
// A small `gain` recentres average brightness near Lambert so this is a
// *reshaping*, not a global dim/brighten; `0.12` regularises the
// terminator/grazing denominator against blowup; result clamped for safety.
fn regolith_factor(N: vec3<f32>, L: vec3<f32>, V: vec3<f32>) -> f32 {
    let mu0 = max(dot(N, L), 0.0);
    // View-independent Lommel-Seeliger response (calibrated lunar regolith gain):
    // Cancels Lambertian limb-darkening without view-direction brightness swings.
    let gain = 0.95;
    let k = gain / (mu0 + 0.5);
    return clamp(k, 0.4, 1.8);
}
