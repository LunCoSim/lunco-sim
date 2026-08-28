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

#import bevy_pbr::mesh_functions

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

/// Baked-map blend weights as a function of `r` — the ratio of one fragment's
/// screen-space WORLD footprint to the derived map's physical TEXEL spacing.
/// Returns `(weight_normal, weight_ao, weight_tone)`.
///
///   * normal fades IN when a pixel covers at least one map texel and OFF when the
///     view resolves below the map — blending a coarser normal there would only
///     blur the geometry and close-range procedural detail.
///   * AO and tone are physical surface data, so their weights are exactly one at
///     every distance. Texture mips filter their frequency; the camera must not
///     change their energy.
///
/// LIVES IN THE SHADER and is evaluated PER FRAGMENT because appearance must be
/// continuous when CDLOD substitutes one mesh depth for another. CPU-derived
/// per-tile weights and the later depth-plus-morph ratio both encoded topology in
/// the material, producing square changes in AO, tone and normal blending. The
/// fragment footprint is the renderer-standard detail signal and has no tile
/// identity to leak into the result.
fn map_weights(r: f32) -> vec3<f32> {
    let w_normal = clamp((r - 0.75) / 1.5, 0.0, 1.0);
    return vec3(w_normal, 1.0, 1.0);
}

/// Decode the normal-map convention shared by the DEM baker and terrain
/// shaders.  The result is in the DEM's local ENU frame, not in whichever
/// floating render frame is active for the current camera.
fn decode_dem_normal(encoded: vec3<f32>) -> vec3<f32> {
    return normalize(encoded * 2.0 - 1.0);
}

/// Convert a baked DEM-local ENU normal into the current render world through
/// the mesh instance.  This is the one coordinate boundary for derived terrain
/// normals: static meshes and streamed BigSpace tiles must both use it before
/// combining a map normal with `VertexOutput.world_normal` or scene lighting.
fn dem_normal_to_world(encoded: vec3<f32>, instance_index: u32) -> vec3<f32> {
    return mesh_functions::mesh_normal_local_to_world(
        decode_dem_normal(encoded), instance_index);
}

/// Raw FBM at a terrain-stable position, platform-correct. Use this for the un-ramped
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

// Procedural terrain detail is anchored to the authored DEM frame, not to the
// transient render-world frame. BigSpace rebases world positions as the camera
// and body move; sampling FBM from `VertexOutput.world_position` therefore makes
// the material slide and re-evaluate at different noise coordinates every frame.
// UVs are the existing DEM-global coordinate carried by both terrain meshes, so
// this stays batched and needs no per-tile material or new vertex attribute.
fn terrain_detail_position(uv: vec2<f32>, half_extent: f32) -> vec3<f32> {
    return vec3(
        (uv.x * 2.0 - 1.0) * half_extent,
        0.0,
        (uv.y * 2.0 - 1.0) * half_extent,
    );
}

// The procedural detail coordinate is DEM-local. Transform the interpolated
// render normal through the same mesh instance before bumping it, then cross the
// one boundary back to render-world once the local perturbation is complete.
fn terrain_detail_normal_to_local(world_normal: vec3<f32>, instance_index: u32) -> vec3<f32> {
    return normalize((mesh_functions::get_local_from_world(instance_index)
        * vec4<f32>(world_normal, 0.0)).xyz);
}

fn terrain_detail_normal_to_world(local_normal: vec3<f32>, instance_index: u32) -> vec3<f32> {
    return mesh_functions::mesh_normal_local_to_world(local_normal, instance_index);
}

/// One ramped FBM layer sampled at terrain-stable position `p`.
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
