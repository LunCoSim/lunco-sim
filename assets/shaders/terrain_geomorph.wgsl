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
    mesh_view_bindings::lights,
}
#import lunco::pbr_lit::lit_n
#import lunco::lunar::regolith_factor

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
//!@default reveal       1.0
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
    reveal:            f32,  // 1 = own geometry; <1 = settling in from the parent lattice
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
    // Gentler, wider ramp than the original (6→24): start fading only once a period
    // shrinks under ~3 px and fade out slowly, so detail reaches much farther and
    // the near/far transition stops reading as a hard "detail bubble".
    return saturate((px_per_period - 3.0) / 22.0);
}

// --- procedural lunar relief: a Voronoi crater + rock field ----------------
// Real macro/meso relief on the Moon is IMPACT CRATERS (circular bowl + raised
// ejecta rim) and scattered rocks — NOT the isotropic FBM that reads as blobby
// cottage cheese. We stamp a jittered-grid crater field as a world-space HEIGHT
// field and perturb the shading normal by its analytic gradient: circular
// features that read as craters. Pure math, no textures → wasm-safe.
//
// Why this is the "proper" lunar look (research, 2026-06-29):
//   * Real macro relief = crater/rock geometry or procedural crater HEIGHT maps,
//     not FBM (FBM noise reads as cottage-cheese; regolith is smooth BETWEEN
//     impacts). — SIGGRAPH Asia 2025, "Materials for the Moon":
//     https://dl.acm.org/doi/10.1145/3757374.3771428
//   * Photometry: dark albedo (~0.08-0.13) + Hapke / Lommel-Seeliger + opposition
//     surge (see lunar_brdf.wgsl). — JPL/arXiv physics-based lunar ground sim:
//     https://arxiv.org/html/2410.04371v1
//     and JPL AAS 23-122 image rendering / terrain generation.
//   * Airless body → NO atmospheric haze: stays high-contrast, crisp to the
//     horizon (the opposition/heiligenschein surge):
//     https://the-moon.us/wiki/Retro-Reflection_phenomena

fn hash21(p: vec2<f32>) -> f32 {
    var p3 = fract(vec3(p.x, p.y, p.x) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

// One octave of craters at average spacing `cell` (m), max depth `depth` (m).
// `density` ∈ (0,1]: fraction of grid cells that actually hold a crater.
fn crater_octave(xz: vec2<f32>, cell: f32, depth: f32, density: f32) -> f32 {
    let g  = xz / cell;
    let gi = floor(g);
    let gf = g - gi;
    var h = 0.0;
    for (var j = -1; j <= 1; j = j + 1) {
        for (var i = -1; i <= 1; i = i + 1) {
            let off = vec2<f32>(f32(i), f32(j));
            let id  = gi + off;
            if (hash21(id + 11.5) > density) { continue; }
            let jit    = vec2(hash21(id), hash21(id + 4.2));
            let center = off + jit;                       // crater centre, cell units
            let d      = distance(gf, center);
            let radius = mix(0.16, 0.42, hash21(id + 7.7));
            let r      = d / radius;
            let dep    = depth * mix(0.45, 1.0, hash21(id + 2.3));
            if (r < 1.0) {
                // parabolic bowl: deepest at centre, 0 at the rim
                h = h - dep * (1.0 - r * r);
            } else if (r < 1.45) {
                // raised ejecta rim: a soft bump just outside the bowl
                let rr = (r - 1.0) / 0.45;
                h = h + dep * 0.30 * (rr * (1.0 - rr) * 4.0);
            }
        }
    }
    return h;
}

// Scattered rocks/clods as sharp positive bumps (Voronoi, tighter than craters).
fn rock_octave(xz: vec2<f32>, cell: f32, height: f32, density: f32) -> f32 {
    let g  = xz / cell;
    let gi = floor(g);
    let gf = g - gi;
    var h = 0.0;
    for (var j = -1; j <= 1; j = j + 1) {
        for (var i = -1; i <= 1; i = i + 1) {
            let off = vec2<f32>(f32(i), f32(j));
            let id  = gi + off;
            if (hash21(id + 19.1) > density) { continue; }
            let jit    = vec2(hash21(id + 5.0), hash21(id + 8.0));
            let center = off + jit;
            let d      = distance(gf, center);
            let radius = mix(0.08, 0.22, hash21(id + 3.1));
            let r      = d / radius;
            if (r < 1.0) {
                let bump = 1.0 - r * r;
                h = h + height * mix(0.4, 1.0, hash21(id + 6.6)) * bump * bump;
            }
        }
    }
    return h;
}

// Combined impact relief height (m) at world xz. Per-octave footprint fades keep
// each scale from aliasing once it shrinks under a pixel: big craters survive to
// the far distance, small craters + rocks only close up. `amp`/`rock_amp` scale
// the whole field for live tuning.
fn relief_height(xz: vec2<f32>, pw: f32, amp: f32, rock_amp: f32) -> f32 {
    // "Maturity" mask: a smooth low-frequency field that thins the crater field
    // in patches → flatter mare-like basins between saturated-cratered highlands,
    // so the surface stops reading as uniform bubble-wrap.
    // PERF: footprint fades decide whether each octave is even visible. Compute them
    // FIRST and early-out — once a feature shrinks under the pixel footprint its
    // contribution is ~0, so far pixels must SKIP the Voronoi loops, not run them and
    // multiply by ~0. The @fragment gradient calls this 3×/pixel and looking at the
    // horizon fills the screen with far tiles; this early-out is the difference
    // between a ~1 s frame and a fast one.
    let f_crater = aa_fade(1.0 / 3.0, pw);
    let f_rock   = aa_fade(1.0 / 1.2, pw);
    if (f_crater <= 0.0 && f_rock <= 0.0) {
        return 0.0;
    }
    let mare = mix(0.45, 1.0, smoothstep(0.35, 0.7, fbm(vec3(xz.x, 0.0, xz.y) * 0.0025, 3, 0.5)));
    var h = 0.0;
    // The big + medium impact craters are now REAL GEOMETRY — the DEM-stamped crater
    // layer (lunco-terrain-surface, driven by ObstacleFieldSpec), which shows in the
    // mesh AND the collider. So the shader no longer fakes large craters as
    // normal-only "bubbles" (that competed with — and muddied — the real ones). It
    // only adds FINE sub-metre texture the geometry can't carry: a sparse scatter of
    // small ≈3 m pits + rock clods, as micro-relief grain.
    if (f_crater > 0.0) { h = h + crater_octave(xz, 3.0, 0.06, 0.18) * amp * mare * f_crater; } // ≈3 m pits
    if (f_rock   > 0.0) { h = h + rock_octave(xz,  1.2, 0.10, 0.28) * rock_amp * f_rock; }      // rock clods
    return h;
}

// World-space to-sun, read straight from the scene lights (no per-material uniform
// wiring needed for streamed tiles). Picks the brightest directional light so the
// dim earthshine fill can't be mistaken for the sun.
fn sun_to_light() -> vec3<f32> {
    var best = vec3(0.0, 1.0, 0.0);
    var best_lum = -1.0;
    let n = lights.n_directional_lights;
    for (var i = 0u; i < n; i = i + 1u) {
        let dl = lights.directional_lights[i];
        let lum = dot(dl.color.rgb, vec3(0.2126, 0.7152, 0.0722));
        if (lum > best_lum) {
            best_lum = lum;
            best = dl.direction_to_light;
        }
    }
    return best;
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
    // Reveal "settle": a freshly-spawned / re-baked tile starts on the parent's
    // coarse lattice (reveal 0 → morph 1) and grows into its own geometry as reveal
    // animates to 1. Combined with the distance morph it never pops — `max` keeps a
    // far tile collapsed while still letting a near tile finish revealing.
    let m = max(morph, 1.0 - mat.reveal);
    let local_pos = mix(vertex.position, vertex.morph_target, m);

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
    let fine_scale  = mat.fine_scale;
    let fine_bump   = mat.fine_bump;
    let rough_mix   = mat.rough_mix;
    let mottle      = mat.mottle;
    var albedo = mat.albedo;

    let p = in.world_position.xyz;
    // Pixel footprint in world metres (BEFORE any branch — fwidth needs uniform
    // control flow). Drives the footprint-based detail fades.
    let pw = length(fwidth(p));

    // IMPORTANT: macro/meso relief is REAL GEOMETRY (the DEM, plus the crater &
    // rock layers), NOT fragment FBM. The old metre-scale FBM "bump" read as
    // blobby cottage-cheese noise because the real Moon is *smooth* between
    // impact features — there is no metre-scale isotropic roughness to fake.
    // So the fragment now only adds a single CRISP micro-grain normal (sub-cm
    // regolith tooth) that catches the grazing sun close-up, and a gentle large-
    // scale albedo variation. Everything that reads as "shape" comes from the
    // mesh; the fragment only does micro-texture + lunar photometry.
    var n = normalize(in.world_normal);

    //   • IMPACT RELIEF (craters + rocks) — the macro/meso shape the smooth DEM
    //     lacks. Perturb the normal by the analytic gradient of the procedural
    //     height field; eps tracks the pixel footprint so the derivative never
    //     aliases into shimmer. (Reuses mid_bump/macro_bump uniforms as relief
    //     amplitudes so this hot-reloads without a Rust rebuild.)
    let amp      = mat.mid_bump * 1.6;
    let rock_amp = max(mat.macro_bump, 0.0) * 8.0;
    let eps = max(pw * 1.5, 0.04);
    let h0  = relief_height(p.xz,                    pw, amp, rock_amp);
    let hx  = relief_height(p.xz + vec2(eps, 0.0),   pw, amp, rock_amp);
    let hz  = relief_height(p.xz + vec2(0.0, eps),   pw, amp, rock_amp);
    let dhdx = (hx - h0) / eps;
    let dhdz = (hz - h0) / eps;
    // gentle terrain: surface up ≈ world +Y, so tilt n by the planar gradient.
    n = normalize(n - vec3(dhdx, 0.0, dhdz));

    //   • fine regolith grain — foreground only (tight fade → no detail bubble).
    let fine_fade = aa_fade(fine_scale, pw);
    var fine_h = 0.5;
    if (fine_fade > 0.0) {
        n = bump_layer(n, p, fine_scale, 2, 0.5, 0.42, 0.58, fine_bump * fine_fade, &fine_h);
    }

    // Large-scale tonal variation (albedo only — cheap, no relief, carries far).
    // Very low frequency = broad maria/highland-style patches, NOT per-metre
    // speckle. This breaks up the flat grey without inventing fake geometry.
    let dust = fbm(p * 0.004, 3, 0.5);
    albedo *= 1.0 + (dust - 0.5) * mottle;

    // --- Lunar photometry: the actual realism lever -----------------------
    // Lommel-Seeliger + opposition surge (retroreflective backscatter) from the
    // scene sun — read from the light bindings, so streamed tiles get it WITHOUT
    // the per-material sun wiring the static mesh needs. Pre-multiplies albedo;
    // lit_n's Lambert + CSM shadows complete the response. This is what makes the
    // surface read as the Moon (flat, then a bright surge toward opposition)
    // instead of generic grey PBR.
    let L = normalize(sun_to_light());
    let V = normalize(view.world_position - p);
    let lunar_k = regolith_factor(n, L, V);
    albedo = albedo * lunar_k;

    // Regolith is rough and non-metallic; rough_mix nudges it.
    let roughness = clamp(0.6 + rough_mix * 0.4, 0.05, 1.0);
    return lit_n(in, is_front, n, albedo, roughness, 0.0, vec3(0.0));
}
