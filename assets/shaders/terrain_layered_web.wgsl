//! Web-optimized Layered lunar terrain material — `regolith_web.wgsl` + non-destructive map layers.
//! Uses 2D value noise and fewer octaves for high-performance rendering in the browser.

#import bevy_pbr::{
    forward_io::VertexOutput,
    pbr_types,
    pbr_functions,
    mesh_bindings::mesh,
    mesh_view_bindings::view,
    mesh_view_bindings::lights,
}
#import lunco::horizon::{sun_visibility_resolved, shadow_fill}
#import lunco::lunar::regolith_factor
#import lunco::noise::fbm2d

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
// --- layer blend weights (0 = layer off → pure procedural) -----------------
//!@ui      weight_albedo     0 1         "Albedo map weight"
//!@default weight_albedo     0
//!@ui      weight_mineral    0 1         "Overlay drape weight (unlit)"
//!@default weight_mineral    0
//!@ui      weight_rough      0 1         "Surface roughness weight"
//!@default weight_rough      0
//!@ui      weight_ao         0 1         "Surface AO weight"
//!@default weight_ao         0
//!@ui      weight_normal     0 1         "Normal map weight"
//!@default weight_normal     0
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
    weight_albedo:     f32,
    weight_mineral:    f32,
    weight_rough:      f32,
    weight_ao:         f32,
    weight_normal:     f32,
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

// Terrain heightfield (R32Float, world-space heights) for the sun-shadow march.
@group(#{MATERIAL_BIND_GROUP}) @binding(1)
var height_map: texture_2d<f32>;

// Pre-baked horizon shadow cache (R8Unorm, 0..1 sun visibility) — sampled
// with a single `textureSampleLevel` when `mat.shadow_cache_on > 0.5` instead
// of the 48-step heightfield ray-march. Filterable (GPU bilinear interp).
@group(#{MATERIAL_BIND_GROUP}) @binding(10)
var shadow_cache: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(11)
var shadow_cache_sampler: sampler;

// Layer maps (filterable; `None` → Bevy fallback white, gated by weight_*).
@group(#{MATERIAL_BIND_GROUP}) @binding(2)
var albedo_tex: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(3)
var albedo_smp: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(4)
var mineral_tex: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(5)
var mineral_smp: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(6)
var surface_tex: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(7)
var surface_smp: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(8)
var normal_tex: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(9)
var normal_smp: sampler;

fn ramp(x: f32, lo: f32, hi: f32) -> f32 {
    return saturate((x - lo) / (hi - lo));
}

fn aa_fade(scale: f32, pw: f32) -> f32 {
    let px_per_period = 1.0 / max(scale * pw, 1e-6);
    return saturate((px_per_period - 6.0) / 18.0);
}

// --- height-field bump ---------------------------------------------------

fn layer_height(p: vec2<f32>, scale: f32, octaves: i32, gain: f32, lo: f32, hi: f32) -> f32 {
    return ramp(fbm2d(p * scale, octaves, gain), lo, hi);
}

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
    
    let p2 = p.xz;
    let pt2 = (p + t * eps).xz;
    let pb2 = (p + b * eps).xz;
    
    let h0 = layer_height(p2, scale, octaves, gain, lo, hi);
    let ht = layer_height(pt2, scale, octaves, gain, lo, hi);
    let hb = layer_height(pb2, scale, octaves, gain, lo, hi);
    *out_h = h0;
    let grad = (ht - h0) * t + (hb - h0) * b;
    let perturbed = n - strength * grad / eps;
    if (length(perturbed) < 1e-3 || dot(perturbed, n) <= 0.0) {
        return n;
    }
    return normalize(perturbed);
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> @location(0) vec4<f32> {
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
    let pw = length(fwidth(p));
    let fine_fade  = aa_fade(fine_scale, pw);
    let macro_fade = aa_fade(macro_scale, pw);
    let mid_fade   = aa_fade(mid_scale, pw);

    var n = normalize(in.world_normal);
    var mid_h = 0.5;
    var macro_h = 0.5;
    var fine_h = 0.5;
    if (mid_fade > 0.0) {
        // Web optimized: 2 octaves instead of 4
        n = bump_layer(n, p, mid_scale, 2, 0.55, 0.35, 0.65, mid_bump * mid_fade, &mid_h);
    }
    if (macro_fade > 0.0) {
        // Web optimized: 2 octaves instead of 5
        n = bump_layer(n, p, macro_scale, 2, 0.6, 0.34, 0.70, macro_bump * macro_fade, &macro_h);
    }
    if (fine_fade > 0.0) {
        // Web optimized: 1 octave instead of 3
        n = bump_layer(n, p, fine_scale, 1, 0.5, 0.45, 0.57, fine_bump * fine_fade, &fine_h);
    }

    let dust_fade = aa_fade(0.008, pw);
    if (dust_fade > 0.0) {
        // Web optimized: 1 octave instead of 3
        let dust = fbm2d(p.xz * 0.008, 1, 0.5);
        albedo *= 1.0 + (dust - 0.5) * 0.18 * dust_fade;
    }
    albedo *= 1.0 + (mix(0.5, mid_h, mid_fade) - 0.5) * mottle;

    let macro_rough = mix(0.5, macro_h, macro_fade);
    var roughness = clamp(mix(macro_rough, 1.0, rough_mix), 0.05, 1.0);

#ifdef VERTEX_UVS_A
    let uv = in.uv;
    if (mat.weight_albedo > 0.0) {
        let a = textureSample(albedo_tex, albedo_smp, uv).rgb;
        albedo = mix(albedo, albedo * a, mat.weight_albedo);
    }
    // (Mineral/classification composites UNLIT after lighting below — see the
    // native variant and doc 18 §4; tinting albedo here would shadow the drape.)
    if (mat.weight_rough > 0.0 || mat.weight_ao > 0.0) {
        let s = textureSample(surface_tex, surface_smp, uv);
        roughness = clamp(mix(roughness, s.r, mat.weight_rough), 0.05, 1.0);
        albedo *= mix(1.0, s.g, mat.weight_ao);
    }
    // Normal: perturb the procedural normal toward the map's baked normal.
    // The baked map stores world-space normals (see derive.rs pack_normal_rgba8),
    // so decode directly — no tangent basis involved.
    if (mat.weight_normal > 0.0) {
        let n_baked = normalize(textureSample(normal_tex, normal_smp, uv).xyz * 2.0 - 1.0);
        n = normalize(mix(n, n_baked, mat.weight_normal));
    }
#endif

    var pbr_input = pbr_types::pbr_input_new();
    pbr_input.flags = mesh[in.instance_index].flags;
    pbr_input.frag_coord = in.position;
    pbr_input.world_position = in.world_position;
    pbr_input.world_normal = pbr_functions::prepare_world_normal(
        normalize(in.world_normal), false, is_front);
    pbr_input.is_orthographic = view.clip_from_view[3].w == 1.0;
    pbr_input.N = n;
    pbr_input.V = pbr_functions::calculate_view(in.world_position, pbr_input.is_orthographic);
    var lunar_k = 1.0;
    let sw = mat.sun_dir_world;
    if (dot(sw, sw) > 0.25) {
        lunar_k = regolith_factor(pbr_input.N, normalize(sw), pbr_input.V);
    }
    pbr_input.material.base_color = vec4(albedo * lunar_k, 1.0);
    pbr_input.material.perceptual_roughness = roughness;
    pbr_input.material.metallic = 0.0;
    pbr_input.material.reflectance = vec3(0.5);

    var color = pbr_functions::apply_pbr_lighting(pbr_input);

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

    // Overlay plane (UNLIT, doc 18 §4) — mirrors the native variant exactly.
    if (mat.weight_mineral > 0.0) {
        let m = textureSample(mineral_tex, mineral_smp, uv).rgb;
        color = vec4(mix(color.rgb, m, mat.weight_mineral), color.a);
    }
#endif

    color = pbr_functions::main_pass_post_lighting_processing(pbr_input, color);
    return color;
}
