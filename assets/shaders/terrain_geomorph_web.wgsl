//! CDLOD geomorph terrain tile — **Web-optimized procedural regolith look**
//! Uses 2D value noise and fewer octaves for high-performance rendering in the browser.

#import bevy_pbr::{
    mesh_functions,
    view_transformations::position_world_to_clip,
    forward_io::VertexOutput,
    mesh_view_bindings::view,
}
#import lunco::pbr_lit::{lit_n, sun_to_light}
#import lunco::noise::fbm2d
#import lunco::lunar::regolith_factor
#import lunco::transfer::{slope_hazard_color, slope_of}

//!@ui      albedo            color  "Albedo"
//!@default albedo            0.13,0.13,0.13
//!@ui      macro_clump_scale 1 20   "Macro clump scale (/m)"
//!@default macro_clump_scale 8
//!@ui      macro_bump        0 0.3  "Meso hummock strength"
//!@default macro_bump        0.1
//!@ui      mid_scale         0.02 1 "Meso hummock scale (/m)"
//!@default mid_scale         0.45
//!@ui      mid_bump          0 1.5  "Mid hummock strength"
//!@default mid_bump          0.6
//!@ui      fine_scale        50 400 "Fine grain scale (/m)"
//!@default fine_scale        180
//!@ui      fine_bump         0 0.1  "Fine grain strength"
//!@default fine_bump         0.025
//!@ui      rough_mix         0 1    "Roughness mix"
//!@default rough_mix         0.35
//!@ui      mottle            0 0.6  "Albedo mottle"
//!@default mottle            0.22
//!@ui      weight_normal     0 1    "Baked normal-map weight"
//!@default weight_normal     0
//!@ui      weight_ao         0 1    "Baked AO weight"
//!@default weight_ao         0
//!@ui      weight_tone       0 1    "Baked tonal (albedo) weight"
//!@default weight_tone       0
//!@engine  shadow_cache_on
//!@engine  csm_far
//!@default morph_start  1.0e20
//!@default morph_end    1.0e21
//!@default overlay_mode      0
//!@default overlay_opacity   0
//!@default overlay_safe_rad  0
//!@default overlay_cliff_rad 0
//!@ui      lod_depth         0 12   "CDLOD tile depth (LOD-depth overlay)"
//!@default lod_depth         0
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
    weight_normal:     f32,  // baked meso normal (fades IN where geometry is coarser than the map)
    weight_ao:         f32,  // baked ambient occlusion (crater bowls/valleys darken)
    weight_tone:       f32,  // baked relief-correlated albedo scalar (normal_tex alpha)
    shadow_cache_on:   f32,  // engine-filled: 1 = far-shadow cache bound and valid
    csm_far:           f32,  // engine-filled: CSM far bound (m); cache fades in beyond ~half
    morph_start:       f32,  // distance where geomorph toward the parent begins
    morph_end:         f32,  // distance where the parent fully takes over
    overlay_mode:      f32,  // analysis overlay: 0 = off, 1 = slope hazard, 2 = LOD depth
    overlay_opacity:   f32,  // blend weight of the overlay colour over the lit surface
    overlay_safe_rad:  f32,  // slope (rad) at/below which ground is green (safe)
    overlay_cliff_rad: f32,  // slope (rad) at/above which ground is red (cliff)
    lod_depth:         f32,  // this tile's CDLOD depth, for the depth overlay
}
@group(#{MATERIAL_BIND_GROUP}) @binding(0)
var<uniform> mat: Material;

// Baked derived maps (see terrain_geomorph.wgsl) — on web these matter even
// more: they are the ONLY texture detail once the cheap 1-octave FBM fades.
@group(#{MATERIAL_BIND_GROUP}) @binding(6)
var surface_tex: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(7)
var surface_smp: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(8)
var normal_tex: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(9)
var normal_smp: sampler;

// Pre-baked horizon shadow cache (see terrain_geomorph.wgsl). Off by default
// on web (config gates the bake) — the branch is dead until enabled.
@group(#{MATERIAL_BIND_GROUP}) @binding(10)
var shadow_cache: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(11)
var shadow_cache_sampler: sampler;

fn ramp(x: f32, lo: f32, hi: f32) -> f32 {
    return saturate((x - lo) / (hi - lo));
}

// Layer CUT-OUT threshold in screen px per noise period — the knob that sets the
// radius of the disc where the bump FBM is paid at all (see the long note in
// `terrain_geomorph.wgsl`). Kept identical to the native shader so the two look the
// same at a given distance; the WebGL saving is bigger here, since this is the
// single-threaded target.
const AA_CUT_PX: f32 = 5.0;
const AA_RAMP_PX: f32 = 7.0;

fn aa_fade(scale: f32, pw: f32) -> f32 {
    let px_per_period = 1.0 / max(scale * pw, 1e-6);
    // Tight ramp — the baked derived maps take over beyond the near field.
    return saturate((px_per_period - AA_CUT_PX) / AA_RAMP_PX);
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

// --- vertex: CDLOD geomorph ---------------------------------------------

struct GeoVertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(8) morph_target: vec3<f32>,
    @location(9) morph_normal: vec3<f32>,
};

@vertex
fn vertex(vertex: GeoVertex) -> VertexOutput {
    var out: VertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    let base_world = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(vertex.position, 1.0));
    let dist = distance(base_world.xyz, view.world_position);

    var morph = 0.0;
    if (mat.morph_end > mat.morph_start) {
        morph = smoothstep(mat.morph_start, mat.morph_end, dist);
    }
    // `morph` alone — deliberately. A per-tile term here (the old `reveal` settle)
    // makes two neighbours at the same depth and distance disagree at their shared
    // edge, cracking the seam. Keep this a pure function of world position.
    let m = morph;
    let local_pos = mix(vertex.position, vertex.morph_target, m);
    // Shade the surface we actually DRAW: the position lerps toward the parent
    // lattice, so the normal must lerp with it. Leaving the fine normal here made
    // a fully-morphed tile shade with detail its geometry no longer has — up to ~22 deg of error,
    // flipping N.L negative on some quads, i.e. new LOD tiles appearing BLACK.
    let local_normal = normalize(mix(vertex.normal, vertex.morph_normal, m));

    out.world_position = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(local_pos, 1.0));
    out.position = position_world_to_clip(out.world_position.xyz);
    out.world_normal = mesh_functions::mesh_normal_local_to_world(local_normal, vertex.instance_index);
#ifdef VERTEX_UVS_A
    out.uv = vertex.uv;
#endif
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    out.instance_index = vertex.instance_index;
#endif
    return out;
}

// --- fragment: procedural regolith (optimized) ---------------------------

// Palette-matched to `lod_rgb` in `stream_viz.rs`. Cycles rather than clamps so
// adjacent depths always differ; clamping blinds the top depths.
fn lod_depth_color(d: f32) -> vec3<f32> {
    let i = i32(max(d, 0.0)) % 7;
    var p = array<vec3<f32>, 7>(
        vec3(0.20, 0.35, 0.85), vec3(0.20, 0.75, 0.85), vec3(0.25, 0.80, 0.35),
        vec3(0.90, 0.85, 0.20), vec3(0.95, 0.55, 0.15), vec3(0.90, 0.25, 0.20),
        vec3(0.85, 0.30, 0.80),
    );
    return p[i];
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> @location(0) vec4<f32> {
    let fine_scale  = mat.fine_scale;
    let fine_bump   = mat.fine_bump;
    let rough_mix   = mat.rough_mix;
    let mottle      = mat.mottle;
    var albedo = mat.albedo;

    let p = in.world_position.xyz;
    let pw = length(fwidth(p));

    // Baked derived maps (weight-gated; see terrain_geomorph.wgsl).
    var map_n = vec4(0.5, 1.0, 0.5, 0.5);
    var map_s = vec4(0.6, 1.0, 0.0, 0.0);
#ifdef VERTEX_UVS_A
    map_n = textureSample(normal_tex, normal_smp, in.uv);
    map_s = textureSample(surface_tex, surface_smp, in.uv);
#endif

    var n = normalize(in.world_normal);
    if (mat.weight_normal > 0.0) {
        let n_baked = normalize(map_n.xyz * 2.0 - 1.0);
        n = normalize(mix(n, n_baked, mat.weight_normal));
    }

    let meso_scale = max(mat.mid_scale, 0.02);
    let meso_fade  = aa_fade(meso_scale, pw);
    var meso_h = 0.5;
    if (meso_fade > 0.0) {
        // Web optimized: 1 octave instead of 3
        n = bump_layer(n, p, meso_scale, 1, 0.55, 0.35, 0.65, mat.macro_bump * meso_fade, &meso_h);
    }
    let subm_scale = meso_scale * 3.0;
    let subm_fade  = aa_fade(subm_scale, pw);
    var subm_h = 0.5;
    if (subm_fade > 0.0) {
        // Web optimized: 1 octave instead of 2
        n = bump_layer(n, p, subm_scale, 1, 0.5, 0.40, 0.60, mat.macro_bump * 0.6 * subm_fade, &subm_h);
    }

    let tooth_scale = clamp(mat.macro_clump_scale, 4.0, 40.0);
    let tooth_fade  = aa_fade(tooth_scale, pw);
    var tooth_h = 0.5;
    if (tooth_fade > 0.0) {
        // Web optimized: 1 octave instead of 3
        n = bump_layer(n, p, tooth_scale, 1, 0.5, 0.40, 0.62, mat.mid_bump * 0.12 * tooth_fade, &tooth_h);
    }

    let fine_fade = aa_fade(fine_scale, pw);
    var fine_h = 0.5;
    if (fine_fade > 0.0) {
        // Web optimized: 1 octave instead of 2
        n = bump_layer(n, p, fine_scale, 1, 0.5, 0.42, 0.58, fine_bump * fine_fade, &fine_h);
    }

    // Web optimized: 1 octave instead of 3
    let dust = fbm2d(p.xz * 0.004, 1, 0.5);
    albedo *= 1.0 + (dust - 0.5) * mottle;

    let grain_fade = aa_fade(0.35, pw);
    if (grain_fade > 0.0) {
        // Web optimized: 1 octave instead of 2
        let grain = fbm2d(p.xz * 0.35, 1, 0.5);
        albedo *= 1.0 + (grain - 0.5) * 0.16 * grain_fade;
        albedo *= 1.0 + (meso_h - 0.5) * 0.10 * meso_fade;
    }

    // Baked relief tone + ambient occlusion (see terrain_geomorph.wgsl).
    albedo *= 1.0 + (map_n.a - 0.5) * (0.6 * mat.weight_tone);
    let map_ao = mix(1.0, 0.4 + 0.6 * map_s.g, mat.weight_ao);
    albedo *= map_ao;

    let L = normalize(sun_to_light());
    let V = normalize(view.world_position - p);
    let lunar_k = regolith_factor(n, L, V);
    let base_albedo = albedo;
    albedo = albedo * lunar_k;

    let n_geo = normalize(in.world_normal);
    let fill = base_albedo * (0.02 + 0.03 * max(n_geo.y, 0.0));

    let roughness =
        clamp(mix(0.6 + rough_mix * 0.4, map_s.r, 0.35 * mat.weight_ao), 0.05, 1.0);
    var color = lit_n(in, is_front, n, albedo, roughness, 0.0, fill);

    // Far-field terrain self-shadow via the pre-baked visibility cache (see
    // terrain_geomorph.wgsl). Dead branch while shadow_cache_on == 0.
#ifdef VERTEX_UVS_A
    if (mat.shadow_cache_on > 0.5) {
        let dist = distance(view.world_position, p);
        var blend = 1.0;
        if (mat.csm_far > 0.0) {
            blend = smoothstep(mat.csm_far * 0.5, mat.csm_far * 0.9, dist);
        }
        if (blend > 0.0) {
            let vis = textureSampleLevel(shadow_cache, shadow_cache_sampler, in.uv, 0.0).r;
            color = vec4(color.rgb * mix(1.0, vis, blend), color.a);
        }
    }
#endif
    // --- Analysis overlay (see terrain_geomorph.wgsl) -------------------------
    // Blend the Transfer's colour over the lit surface; the ramp itself is the
    // shared `lunco::transfer`, uniform-driven (live critical angle). Slope comes
    // from the baked DEM normal wherever that map is bound (`weight_normal > 0` —
    // the coarse tiles), NOT from the LOD mesh normal, which under-reports cliffs at
    // distance. Same rule as the native shader.
    if (mat.overlay_mode > 0.5 && mat.overlay_opacity > 0.0) {
        var tint = vec3(0.0);
        if (mat.overlay_mode < 1.5) {
            var n_haz = n_geo;
#ifdef VERTEX_UVS_A
            if (mat.weight_normal > 0.0) {
                n_haz = normalize(map_n.xyz * 2.0 - 1.0);
            }
#endif
            tint = slope_hazard_color(
                slope_of(n_haz), mat.overlay_safe_rad, mat.overlay_cliff_rad);
        } else {
            tint = lod_depth_color(mat.lod_depth);
        }
        color = vec4(mix(color.rgb, tint, mat.overlay_opacity), color.a);
    }
    return color;
}
