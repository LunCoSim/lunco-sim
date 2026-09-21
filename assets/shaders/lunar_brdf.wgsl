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
// The shared terrain lighting helper applies this factor to the reconstructed
// canonical Sun contribution after `apply_pbr_lighting`. Bevy's Lambert term
// has already supplied μ₀, so the result is albedo · μ₀/(μ₀+μ) · B(α) — the
// Lommel-Seeliger response with opposition surge. Applying it to `base_color`
// would also change earthshine, environment fill, and other authored lights.
// The factor is geometry-only and clamped; the direct response remains bounded
// and tends to zero at the terminator.
//
// Constants are conservative first-cut values; promote to `//!@ui` params for
// live maria/highlands tuning (highlands back-scatter more) as a follow-up.

#define_import_path lunco::lunar

/// Floor on μ = cos(emission). At a grazing view μ → 0 and the Lommel-Seeliger
/// denominator collapses onto μ₀ alone, so `ls` would run away. The final
/// direct-light response remains bounded because Lambert already supplies μ₀.
const MU_FLOOR: f32 = 0.01;

/// Ceiling on the whole photometric multiplier. Headroom for a full opposition
/// surge (1 + `surge_amp` ≈ 2.8) riding a Lommel-Seeliger boost (~2.5 at the
/// grazing geometry this scene is authored at), and nothing beyond.
///
/// The OLD ceiling was 1.8, which is *below* the surge alone — wiring the surge
/// without raising this would have clipped exactly the feature being added.
const K_MAX: f32 = 8.0;

/// Hapke-style opposition surge B(α) — the retroreflective brightening as the view
/// direction approaches the illumination direction. `alpha` is the phase angle in
/// radians (0 = looking straight down the sun vector).
///
///   amp    shadow-hiding amplitude `Bs0`   (1.80238 fitted; was 0.8 here)
///   width  shadow-hiding angular width `hs` (0.07145 rad ≈ 4.1°)
///
/// Normalised so B → 1 at large phase, hence "surge" rather than "gain": switching
/// it on changes the surface only near opposition.
///
/// COHERENT BACKSCATTER IS DELIBERATELY ABSENT. This function used to add a second,
/// narrower spike (`Bc0 = 0.4`, `hc = 0.02`). The reference lunar implementation
/// (Chrono/UW-Madison, arxiv 2410.04371 Table 1) sets `Bc0 = 0` and ignores the
/// term outright: it is a sub-degree feature that costs a second reciprocal and is
/// not separable from shadow-hiding at the angular resolution of a rover camera.
fn opposition_surge(alpha: f32, amp: f32, width: f32) -> f32 {
    let t = tan(clamp(alpha, 0.0, 3.14159) * 0.5);
    return 1.0 + amp / (1.0 + t / max(width, 1e-4));
}

/// Multiplier for the canonical Sun response after Bevy's Lambert term has
/// supplied μ₀, completing Lommel-Seeliger × opposition.
///
///   N  shading normal   (world, unit)
///   L  to-sun direction (world, unit)
///   V  to-camera        (world, unit)
///
/// LOMMEL-SEELIGER. Reflectance goes as μ₀/(μ₀+μ). Bevy's Lambert already supplies
/// the μ₀ numerator, so the multiplier this returns carries 1/(μ₀+μ) — and μ is
/// `dot(N, V)`, the emission cosine.
///
/// The 2.0 normalises to Lambert parity at μ₀ == μ (normal incidence, normal view),
/// so enabling LS is a RESHAPING rather than a global brightness step. `gain` is the
/// authored trim on top of that.
fn regolith_factor(
    N: vec3<f32>, L: vec3<f32>, V: vec3<f32>,
    surge_amp: f32, surge_width: f32, gain: f32,
) -> f32 {
    let mu0 = max(dot(N, L), 0.0);
    let mu = max(dot(N, V), MU_FLOOR);
    let ls = 2.0 / (mu0 + mu);

    // Phase angle. The clamp is not decorative: `dot` of two normalised vectors
    // routinely lands a few ULP outside [-1, 1], and `acos` of that is NaN — which
    // propagates through the multiply and paints black holes in the terrain.
    let alpha = acos(clamp(dot(L, V), -1.0, 1.0));
    let b = opposition_surge(alpha, surge_amp, surge_width);

    return clamp(gain * ls * b, 0.0, K_MAX);
}
