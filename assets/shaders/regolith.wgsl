//! Lunar regolith material for the general `ShaderMaterial`.
//!
//! WGSL port of the procedural Blender node graph in the moonbase Twin's
//! `shackleton_connecting_ridge_render_readyframing.blend` (material
//! `Shackleton_Realistic_Regolith`) — the look that could not survive glTF
//! export. Two world-space FBM noise layers drive bump-style normal
//! perturbation and roughness variation over a flat albedo:
//!
//!   * macro clumps: noise(scale 8) → ramp 0.40..0.62 → bump 0.12 + roughness
//!   * fine grain:   noise(scale 180) → ramp 0.45..0.57 → bump 0.025
//!
//! Noise is sampled in **world space** (the Blender graph used object
//! coordinates on a world-aligned terrain), so the mesh needs no UVs — the
//! Shackleton DEM glb ships POSITION/NORMAL only.
//!
//! Unlike the prop shaders (wheel/balloon), this feeds a full `PbrInput` into
//! `apply_pbr_lighting`, so the regolith is lit by the *scene* sun — the low
//! grazing Shackleton light and its shadows are the whole look.
//!
//! Every noise layer is **analytically anti-aliased**: it fades out as its
//! period approaches the pixel footprint (`fwidth` of the world position).
//! Sub-pixel noise sampled once per pixel is salt-and-pepper speckle — under
//! grazing lunar light it reads as static, never as detail — so a layer is
//! fully shown only while the footprint is ≤ ⅛ of its period and is gone by
//! ¼ period. This is also the perf model: far pixels skip the expensive
//! fine/macro FBM entirely.
//!
//! Three bump scales + albedo variation cover every viewing distance:
//!   fine 5.5 mm grain (≲3 m) → macro 12.5 cm clumps (≲30 m) →
//!   mid ~7 m hummocks (≲1 km) → hectometre albedo patches (orbital).
//!
//! ## Params (0 → default)
//!   param0 = macro noise scale, periods/m (default 8)
//!   param1 = fine  noise scale, periods/m (default 180)
//!   param2 = macro bump strength          (default 0.12)
//!   param3 = fine  bump strength          (default 0.025)
//!   param4 = roughness mix toward 1.0     (default 0.35)
//!   param5 = mid   noise scale, periods/m (default 0.15 — ~7 m hummocks)
//!   param6 = mid   bump strength          (default 0.6)
//!   param7 = albedo mottle amount         (default 0.22)
//!   color_a = albedo (default 0.17 gray — measured lunar regolith is ~0.12)
//!
//! Edit live (hot-reload) or tweak via `SetObjectProperty { property:"param0".. }`.

#import bevy_pbr::{
    forward_io::VertexOutput,
    pbr_types,
    pbr_functions,
    mesh_bindings::mesh,
    mesh_view_bindings::view,
}
#import lunco::horizon::sun_visibility

struct ShaderParams {
    color_a: vec4<f32>,
    color_b: vec4<f32>,
    color_c: vec4<f32>,
    params:  vec4<f32>,
    params2: vec4<f32>,
    engine:  vec4<f32>,  // engine-written: (sun_local.xyz, tan sun radius)
    engine2: vec4<f32>,  // engine-written: (size_x, size_z, heightfield res, csm far bound m)
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0)
var<uniform> mat: ShaderParams;

// Terrain heightfield (R32Float, world-space heights) written by the
// horizon-shadow system after its bake; sun shadows are ray-marched against
// it per pixel (see horizon_march.wgsl). With no heightfield bound the
// march no-ops to fully lit.
@group(#{MATERIAL_BIND_GROUP}) @binding(1)
var height_map: texture_2d<f32>;

// --- 3D value noise + FBM ------------------------------------------------

fn hash13(p: vec3<f32>) -> f32 {
    var p3 = fract(p * 0.1031);
    p3 += dot(p3, p3.zyx + 31.32);
    return fract((p3.x + p3.y) * p3.z);
}

fn vnoise(p: vec3<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let n000 = hash13(i);
    let n100 = hash13(i + vec3(1.0, 0.0, 0.0));
    let n010 = hash13(i + vec3(0.0, 1.0, 0.0));
    let n110 = hash13(i + vec3(1.0, 1.0, 0.0));
    let n001 = hash13(i + vec3(0.0, 0.0, 1.0));
    let n101 = hash13(i + vec3(1.0, 0.0, 1.0));
    let n011 = hash13(i + vec3(0.0, 1.0, 1.0));
    let n111 = hash13(i + vec3(1.0, 1.0, 1.0));
    return mix(
        mix(mix(n000, n100, u.x), mix(n010, n110, u.x), u.y),
        mix(mix(n001, n101, u.x), mix(n011, n111, u.x), u.y),
        u.z,
    );
}

// Normalized to ~0..1 regardless of octave count (matches Blender's 0.5-centred
// noise output, which the ramps below were authored against).
fn fbm(p: vec3<f32>, octaves: i32, gain: f32) -> f32 {
    var sum = 0.0;
    var amp = 1.0;
    var total = 0.0;
    var q = p;
    for (var o = 0; o < octaves; o++) {
        sum += amp * vnoise(q);
        total += amp;
        amp *= gain;
        q *= 2.0; // lacunarity 2.0, as authored
    }
    return sum / total;
}

// Blender linear ColorRamp with two stops (black @ lo, white @ hi).
fn ramp(x: f32, lo: f32, hi: f32) -> f32 {
    return saturate((x - lo) / (hi - lo));
}

// Analytic anti-aliasing weight for a noise layer of `scale` periods/metre
// against pixel footprint `pw` (metres). Full strength only while features
// span ≥24 px, gone by 6 px: features a few pixels wide still read as
// static even when technically resolvable, so the rolloff starts well
// before Nyquist and spans two octaves — a wide band also keeps the
// texture→smooth transition from showing as a line on the ground.
fn aa_fade(scale: f32, pw: f32) -> f32 {
    let px_per_period = 1.0 / max(scale * pw, 1e-6);
    return saturate((px_per_period - 6.0) / 18.0);
}

// --- height-field bump ---------------------------------------------------

// Height of one noise layer at world point p.
fn layer_height(p: vec3<f32>, scale: f32, octaves: i32, gain: f32, lo: f32, hi: f32) -> f32 {
    return ramp(fbm(p * scale, octaves, gain), lo, hi);
}

// Perturbs n by the tangent-plane gradient of a height layer (classic bump
// mapping, same as Blender's Bump node). Returns the new normal; also writes
// the centre-tap height to `out_h` so the roughness path can reuse it.
fn bump_layer(
    n: vec3<f32>, p: vec3<f32>,
    scale: f32, octaves: i32, gain: f32, lo: f32, hi: f32,
    strength: f32, out_h: ptr<function, f32>,
) -> vec3<f32> {
    // Tangent basis from the (already possibly perturbed) normal.
    var up = vec3(0.0, 1.0, 0.0);
    if (abs(n.y) > 0.99) { up = vec3(1.0, 0.0, 0.0); }
    let t = normalize(cross(up, n));
    let b = cross(n, t);
    // Sample spacing: half a period of the base octave.
    let eps = 0.5 / scale;
    let h0 = layer_height(p, scale, octaves, gain, lo, hi);
    let ht = layer_height(p + t * eps, scale, octaves, gain, lo, hi);
    let hb = layer_height(p + b * eps, scale, octaves, gain, lo, hi);
    *out_h = h0;
    let grad = (ht - h0) * t + (hb - h0) * b;
    return normalize(n - strength * grad / eps);
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> @location(0) vec4<f32> {
    // Authored params with Blender-graph defaults (0 → unset).
    let macro_scale  = select(mat.params.x,  8.0,    mat.params.x  < 1e-4);
    let fine_scale   = select(mat.params.y,  180.0,  mat.params.y  < 1e-4);
    // Halved from the Blender graph's 0.12: under grazing lunar sun the
    // full strength flips N·L per-clump → harsh black/white static.
    let macro_bump   = select(mat.params.z,  0.06,   mat.params.z  < 1e-4);
    let fine_bump    = select(mat.params.w,  0.025,  mat.params.w  < 1e-4);
    let rough_mix    = select(mat.params2.x, 0.35,   mat.params2.x < 1e-4);
    let mid_scale    = select(mat.params2.y, 0.15,   mat.params2.y < 1e-4);
    let mid_bump     = select(mat.params2.z, 0.6,    mat.params2.z < 1e-4);
    let mottle       = select(mat.params2.w, 0.22,   mat.params2.w < 1e-4);
    var albedo = mat.color_a.rgb;
    // ShaderMaterial's built-in default color_a is a prop yellow; if the prim
    // didn't author colorA we want regolith gray, not that.
    if (distance(albedo, vec3(0.95, 0.85, 0.10)) < 1e-3) { albedo = vec3(0.17); }

    let p = in.world_position.xyz;
    let dist = distance(view.world_position, p);
    // Pixel footprint in world metres (computed BEFORE any branch — fwidth
    // needs uniform control flow). Drives per-layer anti-alias fades.
    let pw = length(fwidth(p));
    let fine_fade  = aa_fade(fine_scale, pw);
    let macro_fade = aa_fade(macro_scale, pw);
    let mid_fade   = aa_fade(mid_scale, pw);

    // Three chained bump layers, coarse to fine — each perturbed normal
    // feeds the next, as in the Blender graph; each layer only runs where
    // its features are actually resolvable.
    var n = normalize(in.world_normal);
    var mid_h = 0.5;
    var macro_h = 0.5;
    var fine_h = 0.5;
    if (mid_fade > 0.0) {
        n = bump_layer(n, p, mid_scale, 4, 0.55, 0.35, 0.65, mid_bump * mid_fade, &mid_h);
    }
    if (macro_fade > 0.0) {
        // Ramp widened from the authored 0.40..0.62 — the tight ramp made
        // every clump near-binary black/white at grazing sun angles.
        n = bump_layer(n, p, macro_scale, 5, 0.6, 0.34, 0.70, macro_bump * macro_fade, &macro_h);
    }
    if (fine_fade > 0.0) {
        n = bump_layer(n, p, fine_scale, 3, 0.5, 0.45, 0.57, fine_bump * fine_fade, &fine_h);
    }

    // Albedo variation — the Moon is low-contrast, but perfectly uniform
    // grey reads as plastic. Metre-scale mottle from the mid layer plus
    // hectometre dust patches (own AA fade for orbital views).
    let dust_fade = aa_fade(0.008, pw);
    if (dust_fade > 0.0) {
        let dust = fbm(p * 0.008, 3, 0.5);
        albedo *= 1.0 + (dust - 0.5) * 0.18 * dust_fade;
    }
    albedo *= 1.0 + (mix(0.5, mid_h, mid_fade) - 0.5) * mottle;

    // Roughness: macro ramp mixed 35% toward white (Blender Mix fac 0.35),
    // relaxing to its mean where the layer has faded out.
    let macro_rough = mix(0.5, macro_h, macro_fade);
    let roughness = clamp(mix(macro_rough, 1.0, rough_mix), 0.05, 1.0);

    // Full scene lighting: real sun direction, shadow maps, ambient.
    var pbr_input = pbr_types::pbr_input_new();
    pbr_input.flags = mesh[in.instance_index].flags; // keep SHADOW_RECEIVER
    pbr_input.frag_coord = in.position;
    pbr_input.world_position = in.world_position;
    pbr_input.world_normal = pbr_functions::prepare_world_normal(
        normalize(in.world_normal), false, is_front);
    pbr_input.is_orthographic = view.clip_from_view[3].w == 1.0;
    pbr_input.N = n;
    pbr_input.V = pbr_functions::calculate_view(in.world_position, pbr_input.is_orthographic);
    pbr_input.material.base_color = vec4(albedo, 1.0);
    pbr_input.material.perceptual_roughness = roughness;
    pbr_input.material.metallic = 0.0;
    pbr_input.material.reflectance = vec3(0.5);

    var color = pbr_functions::apply_pbr_lighting(pbr_input);

    // Ray-marched heightfield sun shadow (the mesh gains planar UVs once
    // the horizon bake lands; before that this branch compiles out). Within
    // the sun's cascade range (engine2.w) the terrain casts into the CSM,
    // so the march fades in only beyond ~half that range — near pixels get
    // mesh-accurate CSM self-shadow and skip the march loop entirely.
#ifdef VERTEX_UVS_A
    let csm_far = mat.engine2.w;
    var march_blend = 1.0;
    if (csm_far > 0.0) {
        march_blend = smoothstep(csm_far * 0.5, csm_far * 0.9, dist);
    }
    if (march_blend > 0.0) {
        let sun_vis = sun_visibility(
            height_map, in.uv, mat.engine.xyz, mat.engine.w, mat.engine2.xy, mat.engine2.z);
        color = vec4(color.rgb * mix(1.0, sun_vis, march_blend), color.a);
    }
#endif

    color = pbr_functions::main_pass_post_lighting_processing(pbr_input, color);
    return color;
}
