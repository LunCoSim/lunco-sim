//! CDLOD geomorph terrain tile — **procedural regolith look** with a custom
//! `@vertex` morph stage. This is the production material for streamed LOD tiles
//! (promoted from the old depth-tint debug view): the same world-space FBM bump +
//! albedo variation as `regolith.wgsl`, lit by the scene sun through `lit_n`, on
//! top of the CDLOD vertex geomorph so tiles never pop as the LOD switches.
//!
//! Each LOD-tile vertex carries two positions: its own LOD `POSITION` and the
//! `MORPH_TARGET` (the vertex snapped to the parent's coarser even lattice, baked
//! by `bake_tile_mesh`). The vertex shader lerps `POSITION → MORPH_TARGET` by
//! camera distance over the node's CDLOD morph band, so a tile collapses smoothly
//! onto its parent. No texture fetch, no compute → wasm-safe.
//!
//! Self-contained shading: the FBM/bump look needs no engine-filled uniforms, so
//! per-tile materials render correctly without the `wire_terrain_materials`
//! heightfield wiring (which only reaches the single static terrain entity). It
//! therefore omits `regolith.wgsl`'s lunar-BRDF reshape and beyond-CSM horizon
//! ray-march — both engine-fed refinements the static mesh keeps; the streamed
//! tiles still get the full FBM regolith albedo/bump + Bevy PBR sun + CSM shadows.
//!
//! Driven by `ShaderMaterial` (NOT a bespoke material): `m.shader` and
//! `m.vertex_shader` both point here; `m.vertex_shader = Some` makes
//! `ShaderMaterial::specialize` swap the vertex stage and bind
//! `ATTRIBUTE_MORPH_TARGET` at `@location(8)`. Params are reflected from
//! `struct Material` like any self-describing shader.

#import bevy_pbr::{
    mesh_functions,
    view_transformations::position_world_to_clip,
    forward_io::VertexOutput,
    mesh_view_bindings::view,
}
#import lunco::pbr_lit::lit_n

//!@ui      albedo            color  "Albedo"
//!@default albedo            0.13,0.13,0.13
//!@ui      macro_clump_scale 1 20   "Macro clump scale (/m)"
//!@default macro_clump_scale 8
//!@ui      macro_bump        0 0.3  "Macro bump strength"
//!@default macro_bump        0.06
//!@ui      mid_scale         0.02 1 "Mid hummock scale (/m)"
//!@default mid_scale         0.15
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
//!@default morph_start  1.0e20
//!@default morph_end    1.0e21
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
    morph_start:       f32,  // distance where geomorph toward the parent begins
    morph_end:         f32,  // distance where the parent fully takes over
}
@group(#{MATERIAL_BIND_GROUP}) @binding(0)
var<uniform> mat: Material;

// --- 3D value noise + FBM (ported from regolith.wgsl) --------------------

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

fn fbm(p: vec3<f32>, octaves: i32, gain: f32) -> f32 {
    var sum = 0.0;
    var amp = 1.0;
    var total = 0.0;
    var q = p;
    for (var o = 0; o < octaves; o++) {
        sum += amp * vnoise(q);
        total += amp;
        amp *= gain;
        q *= 2.0;
    }
    return sum / total;
}

fn ramp(x: f32, lo: f32, hi: f32) -> f32 {
    return saturate((x - lo) / (hi - lo));
}

fn aa_fade(scale: f32, pw: f32) -> f32 {
    let px_per_period = 1.0 / max(scale * pw, 1e-6);
    return saturate((px_per_period - 6.0) / 18.0);
}

fn layer_height(p: vec3<f32>, scale: f32, octaves: i32, gain: f32, lo: f32, hi: f32) -> f32 {
    return ramp(fbm(p * scale, octaves, gain), lo, hi);
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
    let h0 = layer_height(p, scale, octaves, gain, lo, hi);
    let ht = layer_height(p + t * eps, scale, octaves, gain, lo, hi);
    let hb = layer_height(p + b * eps, scale, octaves, gain, lo, hi);
    *out_h = h0;
    let grad = (ht - h0) * t + (hb - h0) * b;
    let perturbed = n - strength * grad / eps;
    if (length(perturbed) < 1e-3 || dot(perturbed, n) <= 0.0) {
        return n;
    }
    return normalize(perturbed);
}

// --- vertex: CDLOD geomorph ---------------------------------------------

// Standard mesh attributes + the morph-target at location 8 (added to the layout
// by ShaderMaterial::specialize when vertex_shader is set).
struct GeoVertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(8) morph_target: vec3<f32>,
};

@vertex
fn vertex(vertex: GeoVertex) -> VertexOutput {
    var out: VertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);

    // Camera distance from the un-morphed world position (big_space rebases both
    // view and mesh into the same render frame → true eye→vertex distance).
    let base_world = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(vertex.position, 1.0));
    let dist = distance(base_world.xyz, view.world_position);

    // CDLOD morph: 0 near (own LOD) → 1 far (collapse onto parent lattice). Root
    // tiles pass morph_end <= morph_start → no morph.
    var morph = 0.0;
    if (mat.morph_end > mat.morph_start) {
        morph = smoothstep(mat.morph_start, mat.morph_end, dist);
    }
    let local_pos = mix(vertex.position, vertex.morph_target, morph);

    out.world_position = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(local_pos, 1.0));
    out.position = position_world_to_clip(out.world_position.xyz);
    out.world_normal = mesh_functions::mesh_normal_local_to_world(vertex.normal, vertex.instance_index);
#ifdef VERTEX_UVS_A
    out.uv = vertex.uv;
#endif
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    out.instance_index = vertex.instance_index;
#endif
    return out;
}

// --- fragment: procedural regolith (ported from regolith.wgsl) -----------

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
    // Pixel footprint in world metres (BEFORE any branch — fwidth needs uniform
    // control flow). Drives per-layer anti-alias fades.
    let pw = length(fwidth(p));
    let fine_fade  = aa_fade(fine_scale, pw);
    let macro_fade = aa_fade(macro_scale, pw);
    let mid_fade   = aa_fade(mid_scale, pw);

    // Three chained bump layers, coarse → fine; each only runs where resolvable.
    var n = normalize(in.world_normal);
    var mid_h = 0.5;
    var macro_h = 0.5;
    var fine_h = 0.5;
    if (mid_fade > 0.0) {
        n = bump_layer(n, p, mid_scale, 4, 0.55, 0.35, 0.65, mid_bump * mid_fade, &mid_h);
    }
    if (macro_fade > 0.0) {
        n = bump_layer(n, p, macro_scale, 5, 0.6, 0.34, 0.70, macro_bump * macro_fade, &macro_h);
    }
    if (fine_fade > 0.0) {
        n = bump_layer(n, p, fine_scale, 3, 0.5, 0.45, 0.57, fine_bump * fine_fade, &fine_h);
    }

    // Albedo variation — hectometre dust patches + metre-scale mottle.
    let dust_fade = aa_fade(0.008, pw);
    if (dust_fade > 0.0) {
        let dust = fbm(p * 0.008, 3, 0.5);
        albedo *= 1.0 + (dust - 0.5) * 0.18 * dust_fade;
    }
    albedo *= 1.0 + (mix(0.5, mid_h, mid_fade) - 0.5) * mottle;

    // Roughness: macro ramp mixed toward white, relaxing where the layer faded.
    let macro_rough = mix(0.5, macro_h, macro_fade);
    let roughness = clamp(mix(macro_rough, 1.0, rough_mix), 0.05, 1.0);

    // Full Bevy PBR with the bump-perturbed normal: scene sun, shadow maps,
    // ambient. (Lunar BRDF reshape + beyond-CSM horizon march are engine-fed and
    // stay on the static mesh; tiles get standard PBR + CSM self-shadow.)
    return lit_n(in, is_front, n, albedo, roughness, 0.0, vec3(0.0));
}
