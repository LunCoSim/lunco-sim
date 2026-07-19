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
//! Dynamic, self-describing parameters: the engine reflects the `Material`
//! struct (field names → offsets) and the `//!@` annotations (UI ranges,
//! defaults, engine-filled fields) straight out of this file. The named
//! params (`albedo`, `macro_clump_scale`, `mid_scale`, `fine_scale`, the
//! matching bump strengths, `rough_mix`, `mottle`) are listed with their
//! ranges/defaults in the annotation block below. Edit live (hot-reload) or
//! via the Inspector / `SetObjectProperty`.

#import bevy_pbr::{
    forward_io::VertexOutput,
    pbr_types,
    pbr_functions,
    mesh_bindings::mesh,
    mesh_view_bindings::view,
}
#import lunco::horizon::{sun_visibility_resolved, shadow_fill}
#import lunco::lunar::regolith_factor
#import lunco::noise::fbm

// Dynamic, self-describing parameters — the engine reflects this `Material`
// struct (field names → offsets) and the `//!@` annotations (UI ranges,
// defaults, engine-filled fields) straight out of this file. Edit live
// (hot-reload) or via the Inspector / `SetObjectProperty`.
//!@ui      albedo            color       "Albedo"
//!@default albedo            0.13,0.13,0.13
//!@ui      macro_clump_scale 1 20        "Macro clump scale (/m)"
//!@default macro_clump_scale 8
//!@ui      macro_bump        0 0.3       "Macro bump strength"
//!@default macro_bump        0.06
//!@ui      mid_scale         0.02 1      "Mid hummock scale (/m)"
//!@default mid_scale         0.15
//!@ui      mid_bump          0 1.5       "Mid hummock strength"
//!@default mid_bump          0.6
//!@ui      fine_scale        50 400      "Fine grain scale (/m)"
//!@default fine_scale        180
//!@ui      fine_bump         0 0.1       "Fine grain strength"
//!@default fine_bump         0.025
//!@ui      rough_mix         0 1         "Roughness mix"
//!@default rough_mix         0.35
//!@ui      mottle            0 0.6       "Albedo mottle"
//!@default mottle            0.22
//!@engine  sun_dir
//!@engine  sun_dir_world
//!@engine  sun_tan_radius
//!@engine  hf_size
//!@engine  hf_res
//!@engine  csm_far
//!@engine  shadow_cache_on
struct Material {
    albedo:            vec3<f32>,
    macro_clump_scale: f32,
    macro_bump:        f32,
    mid_scale:         f32,
    mid_bump:          f32,
    fine_scale:        f32,
    fine_bump:         f32,
    rough_mix:         f32,
    mottle:            f32,
    sun_tan_radius:    f32,  // engine-filled: tan(sun angular radius)
    sun_dir:           vec3<f32>,  // engine-filled: terrain-local to-sun dir
    sun_dir_world:     vec3<f32>,  // engine-filled: world-space to-sun (lunar BRDF)
    hf_size:           vec2<f32>,  // engine-filled: heightfield extent (m)
    hf_res:            f32,  // engine-filled: heightfield resolution
    csm_far:           f32,  // engine-filled: CSM far bound (m); march fades in beyond
    shadow_cache_on:   f32,  // engine-filled: 1 = sample pre-baked shadow cache, 0 = ray-march
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0)
var<uniform> mat: Material;

// Terrain heightfield (R32Float, world-space heights) written by the
// horizon-shadow system after its bake; sun shadows are ray-marched against
// it per pixel (see horizon_march.wgsl). With no heightfield bound the
// march no-ops to fully lit.
@group(#{MATERIAL_BIND_GROUP}) @binding(1)
var height_map: texture_2d<f32>;

// Pre-baked horizon shadow cache (R8Unorm, 0..1 sun visibility) — sampled
// with a single `textureSampleLevel` when `mat.shadow_cache_on > 0.5` instead
// of the 48-step heightfield ray-march (see `horizon_march.wgsl`). Filterable,
// so the GPU bilinearly interpolates the cache for free.
@group(#{MATERIAL_BIND_GROUP}) @binding(10)
var shadow_cache: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(11)
var shadow_cache_sampler: sampler;

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
    // Guard against a degenerate perturbation: a strong bump on a steep ramp
    // can push the normal to ~zero length or below the surface → normalize()
    // would NaN / flip. Keep the geometric normal in those cases.
    let perturbed = n - strength * grad / eps;
    if (length(perturbed) < 1e-3 || dot(perturbed, n) <= 0.0) {
        return n;
    }
    return normalize(perturbed);
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> @location(0) vec4<f32> {
    // Named params (defaults supplied by the schema, so no `select` fallbacks).
    let macro_scale = mat.macro_clump_scale;
    let fine_scale  = mat.fine_scale;
    let macro_bump  = mat.macro_bump;
    let fine_bump   = mat.fine_bump;
    let rough_mix   = mat.rough_mix;
    let mid_scale   = mat.mid_scale;
    let mid_bump    = mat.mid_bump;
    let mottle      = mat.mottle;
    var albedo = mat.albedo;

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
    // Lunar regolith photometry: reshape the sun diffuse from Lambert to
    // Lommel-Seeliger + opposition surge (retroreflective backscatter). The
    // factor pre-multiplies base_color; bevy's built-in Lambert (·μ₀) then
    // completes the response. World-space to-sun comes from the engine (the
    // CPU-picked canonical sun), NOT directional_lights[0] — the earthshine
    // fill light can shuffle that.
    //
    // No shader-side fallback. Every consumer is engine-filled: heightfield
    // terrain by `wire_terrain_materials`, everything else (landing pad disc,
    // ground plate) by `wire_sun_for_non_terrain_materials`. This used to guess
    // the brightest directional light when the uniform was unset, which was
    // exact only while that light WAS the sun and silently wrong otherwise.
    // A still-zero uniform now means the wiring is broken; leaving the BRDF
    // disengaged (flat Lambert) makes that visible instead of plausible.
    let sw = mat.sun_dir_world;
    var lunar_k = 1.0;
    if (dot(sw, sw) > 0.25) {
        lunar_k = regolith_factor(pbr_input.N, normalize(sw), pbr_input.V);
    }
    pbr_input.material.base_color = vec4(albedo * lunar_k, 1.0);
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
    let csm_far = mat.csm_far;
    var march_blend = 1.0;
    if (csm_far > 0.0) {
        march_blend = smoothstep(csm_far * 0.5, csm_far * 0.9, dist);
    }
    if (march_blend > 0.0) {
        let sun_vis = sun_visibility_resolved(
            shadow_cache, shadow_cache_sampler, mat.shadow_cache_on,
            height_map, in.uv, mat.sun_dir, mat.sun_tan_radius, mat.hf_size, mat.hf_res);
        color = vec4(color.rgb * mix(1.0, sun_vis, march_blend), color.a);
    }
    // Shadow fill — see `shadow_fill` (lunco::horizon), which gates itself on
    // `csm_far` so it fires only where the march machinery is actually wired.
    // Applied outside the march branch so near (CSM) and far (march) pixels get the
    // SAME lift; a branch-local fill painted a bright ring at the handoff.
    color = vec4(color.rgb + shadow_fill(albedo, in.uv, mat.hf_res), color.a);
#endif

    color = pbr_functions::main_pass_post_lighting_processing(pbr_input, color);
    return color;
}
