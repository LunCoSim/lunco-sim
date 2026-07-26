// Shared regolith SURFACE kernel for every terrain shader, via naga_oil import.
//
// WHY THIS FILE EXISTS. `ramp`, `aa_fade`, `layer_height` and `bump_layer` were
// copy-pasted into all six terrain shaders (`terrain_geomorph`, `terrain_layered`,
// `regolith`, and a `_web` variant of each). Measured 2026-07-26: the `_web` files
// were 88-92% byte-identical to their native twins. Two bugs had already grown in
// that gap, and both were invisible because nothing forced the copies to agree:
//
//   * `aa_fade` was retuned from (6.0, 18.0) to (5.0, 7.0) in `terrain_geomorph`
//     only. The other FOUR shaders kept the old wide ramp — carrying full FBM to
//     ~1 km at real fragment cost, the exact thing the retune removed.
//   * `ORTHO_GAIN` (see `lunar_brdf.wgsl`) was applied to the native shaders but
//     not `terrain_layered_web`, so the same scene rendered its ground 3x darker
//     in a browser than on the desktop.
//
// A shared kernel is not a tidiness preference here: it is the only structure in
// which "native and web agree" is a fact rather than a hope.
//
// PLATFORM SPLIT. Exactly two things ever differed between a native shader and
// its `_web` twin:
//
//   1. Noise dimensionality — `fbm` (3D world position) vs `fbm2d` (the XZ plane).
//   2. An octave budget — every `_web` call site used `max(1, native / 2)`. That
//      was verified against all six files: geomorph 3,2,3,2 -> 1,1,1,1 and
//      layered/regolith 4,5,3 -> 2,2,1. The rule reproduces every shipped web
//      value EXACTLY, so folding it in here changes neither platform's output.
//
// Both live behind `LUNCO_NOISE_2D`, a shader_def the material pipeline sets on
// wasm (`shader_material.rs::specialize`). One definition, one place to change.

#define_import_path lunco::terrain

#ifdef LUNCO_NOISE_2D
#import lunco::noise::fbm2d
#else
#import lunco::noise::fbm_rot
#endif

// Footprint fade thresholds, in screen pixels per noise period.
//
// A layer fades out once its period shrinks below `AA_CUT_PX`, because value-noise
// detail finer than ~5 px stops reading as relief and starts reading as shimmer.
// This is ALSO the cost knob: `bump_layer` runs a full FBM (3 taps x N octaves) on
// every fragment where the fade is > 0, so the cut radius — not the ramp width —
// sets the size of the expensive disc around the camera. At the old 3 px cut the
// meso layer alone reached ~960 m: essentially the whole screen when standing on
// the surface.
//
// CONVERGED 2026-07-26 on `terrain_geomorph`'s tuned pair. `terrain_layered` and
// `regolith` previously used (6.0, 18.0); that ramp is ~2.6x wider, so those two
// carried procedural detail much further out. The baked normal/AO/tone maps take
// over past the near field, which is what makes the tighter ramp safe — verify
// the static-mesh scenes still read correctly at distance.
const AA_CUT_PX: f32 = 5.0;
const AA_RAMP_PX: f32 = 7.0;

/// Remap `x` from [lo, hi] to [0, 1], clamped. LINEAR on purpose — every terrain
/// shader's bump strengths and albedo ramps were authored against this response,
/// so a smoothstep here would quietly restyle all six.
fn ramp(x: f32, lo: f32, hi: f32) -> f32 {
    return saturate((x - lo) / (hi - lo));
}

/// Footprint-based detail fade. `pw` is the world width of one pixel at the shading
/// point; `scale` is the layer's spatial frequency in 1/m. Returns 0 where the layer
/// would alias, 1 where it is comfortably resolved.
///
/// Note this is a HIGH-pass on period, not a distance fade: up close `pw` is small,
/// so the result saturates at 1 and every layer is fully on.
fn aa_fade(scale: f32, pw: f32) -> f32 {
    let px_per_period = 1.0 / max(scale * pw, 1e-6);
    return saturate((px_per_period - AA_CUT_PX) / AA_RAMP_PX);
}

/// The platform octave budget. Call sites pass the NATIVE octave count and this
/// halves it on web — see the header for why that reproduces the shipped values.
fn oct(full: i32) -> i32 {
#ifdef LUNCO_NOISE_2D
    return max(1, full / 2);
#else
    return full;
#endif
}

/// Baked-map blend weights as a function of `r` — the ratio of the tile's VERTEX
/// pitch to the derived map's TEXEL pitch (`r = map_res / (2^depth · quads)`,
/// window-size independent). Returns `(weight_normal, weight_ao, weight_tone)`.
///
///   * normal fades IN where the tile geometry is COARSER than the map (far tiles,
///     where the map still carries crater rims the mesh LOD'd away) and OFF where
///     fine near geometry out-resolves the map — blending the coarser map there
///     would only blur real relief.
///   * ao / tone stay partly on everywhere (bowls genuinely receive less sky light
///     at any range) and saturate on coarse tiles.
///
/// LIVES IN THE SHADER, evaluated PER FRAGMENT, because `r` must be continuous.
/// These weights used to be computed on the CPU from the tile's INTEGER LOD depth
/// and uploaded as three per-tile uniforms. `r` doubles per depth level, so every
/// LOD boundary was a step in `weight_normal`/`weight_ao`/`weight_tone` — and
/// therefore a step in albedo, AO and normal blending — along the tile edge.
///
/// The CDLOD vertex stage morphs POSITION and NORMAL smoothly across exactly that
/// boundary, so the mesh was continuous while its shading was not: a straight,
/// hard-edged brightness seam following the quadtree. Feeding `r` through the same
/// morph factor the geometry uses makes the two agree — a fine tile at full morph
/// evaluates its parent's `r`, which is what its coarse neighbour is evaluating.
fn map_weights(r: f32) -> vec3<f32> {
    let w_normal = clamp((r - 0.75) / 1.5, 0.0, 1.0);
    let w_ao = clamp(0.35 + (r - 0.5) * 0.4, 0.35, 1.0);
    let w_tone = clamp(0.5 + (r - 0.5) * 0.35, 0.5, 1.0);
    return vec3(w_normal, w_ao, w_tone);
}

/// Raw FBM at a world position, platform-correct. Use this for the un-ramped
/// tonal layers (dust wash, metre-scale grain) so they pick up the same noise
/// family and octave budget as the bump layers instead of calling `fbm`/`fbm2d`
/// directly — that direct call is how the native shaders ended up on unrotated
/// noise while their `_web` twins were on rotated.
fn surface_fbm(p: vec3<f32>, octaves: i32, gain: f32) -> f32 {
#ifdef LUNCO_NOISE_2D
    return fbm2d(p.xz, oct(octaves), gain);
#else
    return fbm_rot(p, oct(octaves), gain);
#endif
}

/// One ramped FBM layer sampled at world position `p`.
fn layer_height(p: vec3<f32>, scale: f32, octaves: i32, gain: f32, lo: f32, hi: f32) -> f32 {
#ifdef LUNCO_NOISE_2D
    return ramp(fbm2d(p.xz * scale, oct(octaves), gain), lo, hi);
#else
    return ramp(fbm_rot(p * scale, oct(octaves), gain), lo, hi);
#endif
}

/// Perturb shading normal `n` by the gradient of one noise layer, and report that
/// layer's height through `out_h` so the caller can reuse it for albedo/roughness
/// without paying for a second FBM.
///
/// Finite-differenced along the surface tangent frame rather than an analytic
/// derivative: the two agree to within the eps used here, and the FD form stays
/// correct if the noise function underneath is swapped.
fn bump_layer(
    n: vec3<f32>, p: vec3<f32>,
    scale: f32, octaves: i32, gain: f32, lo: f32, hi: f32,
    strength: f32, out_h: ptr<function, f32>,
) -> vec3<f32> {
    var up = vec3(0.0, 1.0, 0.0);
    if (abs(n.y) > 0.99) { up = vec3(1.0, 0.0, 0.0); }
    let t = normalize(cross(up, n));
    let b = cross(n, t);
    let eps = 0.5 / scale;

    let h0 = layer_height(p, scale, octaves, gain, lo, hi);
    let ht = layer_height(p + t * eps, scale, octaves, gain, lo, hi);
    let hb = layer_height(p + b * eps, scale, octaves, gain, lo, hi);
    *out_h = h0;

    let grad = (ht - h0) * t + (hb - h0) * b;
    let perturbed = n - strength * grad / eps;
    // A large `strength` can flip or annihilate the normal; fall back rather than
    // emit a NaN or an inward-facing normal that would read as a black speckle.
    if (length(perturbed) < 1e-3 || dot(perturbed, n) <= 0.0) {
        return n;
    }
    return normalize(perturbed);
}
