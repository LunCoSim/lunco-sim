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
    weight_normal:     f32,  // baked meso normal (fades IN where geometry is coarser than the map)
    weight_ao:         f32,  // baked ambient occlusion (crater bowls/valleys darken)
    weight_tone:       f32,  // baked relief-correlated albedo scalar (normal_tex alpha)
    shadow_cache_on:   f32,  // engine-filled: 1 = far-shadow cache bound and valid
    csm_far:           f32,  // engine-filled: CSM far bound (m); cache fades in beyond ~half
    morph_start:       f32,  // distance where geomorph toward the parent begins
    morph_end:         f32,  // distance where the parent fully takes over
    reveal:            f32,  // 1 = own geometry; <1 = settling in from the parent lattice
}
@group(#{MATERIAL_BIND_GROUP}) @binding(0)
var<uniform> mat: Material;

// Baked derived maps (lunco-terrain-surface derived_layers; whole-DEM planar
// UV). `None` binds Bevy's fallback white — every read is weight-gated so an
// unbound map contributes nothing. surface: R=roughness G=AO B=rockDens
// A=hazard; normal: RGB = world normal biased, A = albedo scalar (0.5 neutral).
@group(#{MATERIAL_BIND_GROUP}) @binding(6)
var surface_tex: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(7)
var surface_smp: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(8)
var normal_tex: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(9)
var normal_smp: sampler;

// Pre-baked horizon shadow cache (R8Unorm 0..1 sun visibility, whole-DEM
// planar UV — same texture the static regolith/layered shaders sample). One
// fetch replaces the 48-step ray-march; gated by `shadow_cache_on` and blended
// in only beyond the CSM range, so near tiles keep mesh-accurate cascades.
@group(#{MATERIAL_BIND_GROUP}) @binding(10)
var shadow_cache: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(11)
var shadow_cache_sampler: sampler;

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
    // Rotate the domain about +Y by the golden angle (≈2.4 rad) each octave.
    // Value noise is axis-aligned, so un-rotated octaves stack into diagonal
    // grid streaks that read as static under grazing lunar light; a per-octave
    // rotation decorrelates them into isotropic grain.
    let rc = cos(2.399963);
    let rs = sin(2.399963);
    for (var o = 0; o < octaves; o++) {
        sum += amp * vnoise(q);
        total += amp;
        amp *= gain;
        q *= 2.0;
        q = vec3(rc * q.x - rs * q.z, q.y, rs * q.x + rc * q.z);
    }
    return sum / total;
}

fn ramp(x: f32, lo: f32, hi: f32) -> f32 {
    return saturate((x - lo) / (hi - lo));
}

fn aa_fade(scale: f32, pw: f32) -> f32 {
    let px_per_period = 1.0 / max(scale * pw, 1e-6);
    // Fade a layer out once its period shrinks under ~3 px. The ramp used to be
    // very wide (/22 — FBM carried to ~1 km because nothing else textured the
    // far field, at real fragment cost); now the baked derived maps take over
    // beyond the near field, so the procedural layers can hand off much sooner.
    return saturate((px_per_period - 3.0) / 9.0);
}

// NOTE (Phase 1, 2026-07-05): the fragment previously faked crater + rock relief
// here as a Voronoi HEIGHT field perturbing only the shading normal. That read as
// a painted-on "mess" up close — normal-only features have no silhouette/parallax,
// and the hard Voronoi cell/rim boundaries creased the normal under grazing sun.
// All macro/meso SHAPE now belongs to real geometry: the DEM + the crater HeightSource
// (sampled by the CDLOD baker + collider) and scattered rock meshes. The fragment
// does ONLY believable sub-decimetre regolith micro-tooth + lunar photometry + broad
// albedo variation. (crater_octave/rock_octave/relief_height/hash21 removed.)
//
// The "proper" lunar look (research, 2026-06-29):
//   * Real macro relief = crater/rock GEOMETRY, not FBM (isotropic FBM reads as
//     cottage-cheese; regolith is smooth BETWEEN impacts). — SIGGRAPH Asia 2025,
//     "Materials for the Moon": https://dl.acm.org/doi/10.1145/3757374.3771428
//   * Photometry: dark albedo (~0.08-0.13) + Hapke / Lommel-Seeliger + opposition
//     surge (see lunar_brdf.wgsl). — JPL/arXiv: https://arxiv.org/html/2410.04371v1
//   * Airless → NO haze: high-contrast, crisp to the horizon.

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

    // Baked derived maps (sampled unconditionally — uniform control flow; the
    // weight gates make an unbound/fallback map a no-op). UVs are DEM-global.
    var map_n = vec4(0.5, 1.0, 0.5, 0.5);
    var map_s = vec4(0.6, 1.0, 0.0, 0.0);
#ifdef VERTEX_UVS_A
    map_n = textureSample(normal_tex, normal_smp, in.uv);
    map_s = textureSample(surface_tex, surface_smp, in.uv);
#endif

    // All macro/meso SHAPE comes from the mesh (DEM + crater geometry) and from
    // scattered rock meshes — the fragment no longer fakes relief. It adds only
    // believable normal-only micro-texture (features small enough that the absence
    // of parallax is imperceptible) + lunar photometry + broad albedo variation.
    var n = normalize(in.world_normal);

    // Baked meso normal: where tile geometry has LOD'd coarser than the map's
    // texel pitch (far tiles), the map carries the crater rims/slopes the mesh
    // no longer has. weight_normal is set per LOD depth by the streamer (0 on
    // fine near tiles whose geometry out-resolves the map).
    if (mat.weight_normal > 0.0) {
        let n_baked = normalize(map_n.xyz * 2.0 - 1.0);
        n = normalize(mix(n, n_baked, mat.weight_normal));
    }

    //   • meso hummocks — the ~0.7–2 m relief band. The geometry stack carries
    //     craters ≥ 0.4 m as real relief, but between discrete craterlets real
    //     regolith still undulates (buried, saturated, gardened relief) at this
    //     scale; its absence was the "one step forward and the ground is a flat
    //     sheet" read. Normal-only is fine here: sub-2 m features at standing
    //     height don't need parallax. Two octave-spaced layers, footprint-faded.
    let meso_scale = max(mat.mid_scale, 0.02);           // default 0.45 → λ ≈ 2.2 m
    let meso_fade  = aa_fade(meso_scale, pw);
    var meso_h = 0.5;
    if (meso_fade > 0.0) {
        n = bump_layer(n, p, meso_scale, 3, 0.55, 0.35, 0.65, mat.macro_bump * meso_fade, &meso_h);
    }
    let subm_scale = meso_scale * 3.0;                   // λ ≈ 0.74 m
    let subm_fade  = aa_fade(subm_scale, pw);
    var subm_h = 0.5;
    if (subm_fade > 0.0) {
        n = bump_layer(n, p, subm_scale, 2, 0.5, 0.40, 0.60, mat.macro_bump * 0.6 * subm_fade, &subm_h);
    }

    //   • regolith tooth — smooth decimetre dimples that give the close-up ground
    //     life without pretending to be geometry. Both octaves footprint-faded so
    //     they never alias into shimmer. Amplitude/scale reuse mid_bump/macro_clump_scale
    //     (freed by dropping the fake relief) so they stay live-tunable via hot-reload.
    let tooth_scale = clamp(mat.macro_clump_scale, 4.0, 40.0); // default 8 → ≈12 cm
    let tooth_fade  = aa_fade(tooth_scale, pw);
    var tooth_h = 0.5;
    if (tooth_fade > 0.0) {
        n = bump_layer(n, p, tooth_scale, 3, 0.5, 0.40, 0.62, mat.mid_bump * 0.12 * tooth_fade, &tooth_h);
    }

    //   • fine regolith grain — millimetre tooth that catches the grazing sun in
    //     the immediate foreground (tight fade → no detail bubble).
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

    // Metre-scale tonal grain: between the 250 m dust wash above and the
    // normal-only micro layers there was NO albedo variation at human scale, so
    // genuinely smooth ground read as untextured plastic. Disturbed (cresting)
    // regolith is subtly brighter than compacted lows — tie a touch of the meso
    // height in, plus an independent ~3 m fbm. Footprint-faded like the bumps.
    let grain_fade = aa_fade(0.35, pw);
    if (grain_fade > 0.0) {
        let grain = fbm(p * 0.35, 2, 0.5);
        albedo *= 1.0 + (grain - 0.5) * 0.16 * grain_fade;
        albedo *= 1.0 + (meso_h - 0.5) * 0.10 * meso_fade;
    }

    // Baked relief tone: rims/ejecta brighter, bowls darker (normal_tex alpha,
    // 0.5 = neutral). This is what keeps distant relief legible after the
    // procedural layers and even the mesh detail have faded out.
    albedo *= 1.0 + (map_n.a - 0.5) * (0.6 * mat.weight_tone);

    // Baked ambient occlusion: crater interiors and valley floors receive less
    // sky/bounce light. Darkens the diffuse base rather than the direct sun
    // term (lit_n owns that), which visually matches at the distances where
    // this weight is raised.
    let map_ao = mix(1.0, 0.4 + 0.6 * map_s.g, mat.weight_ao);
    albedo *= map_ao;

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
    let base_albedo = albedo;
    albedo = albedo * lunar_k;

    // Shadow fill: a whisper of hemispheric bounce (earthshine + regolith
    // inter-reflection) rides the emissive slot so CSM shadows don't crush to
    // pure black — shadowed crater floors stay readable without washing out the
    // raking-light contrast that sells the surface. Hemispheric fill is a
    // half-sky integral — insensitive to micro-relief — so it reads the
    // GEOMETRIC normal, not the bump-perturbed one: driving it with `n` turned
    // every shadowed slope into high-contrast micro-speckle static.
    let n_geo = normalize(in.world_normal);
    let fill = base_albedo * (1.2 + 1.0 * max(n_geo.y, 0.0));

    // Regolith is rough and non-metallic; rough_mix nudges it, and the baked
    // slope-derived roughness (surface_tex R) leans in where the maps are live.
    let roughness =
        clamp(mix(0.6 + rough_mix * 0.4, map_s.r, 0.35 * mat.weight_ao), 0.05, 1.0);
    var color = lit_n(in, is_front, n, albedo, roughness, 0.0, fill);

    // Far-field terrain self-shadow: beyond the sun cascades (tiles are
    // NotShadowCaster — CSM only carries object shadows onto them) sample the
    // pre-baked sun-visibility cache. This is what stops distant crater relief
    // reading as flat unshaded mounds.
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
    return color;
}
