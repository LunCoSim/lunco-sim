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
#import lunco::pbr_lit::{lit_n, sun_to_light}
#import lunco::noise::vnoise
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
//!@ui      weight_albedo     0 1    "Authored albedo (orthophoto) weight"
//!@default weight_albedo     0
//!@ui      weight_mineral    0 1    "Overlay drape weight (unlit)"
//!@default weight_mineral    0
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
    weight_albedo:     f32,  // AUTHORED albedo raster (orthophoto) over the procedural regolith
    weight_mineral:    f32,  // AUTHORED overlay drape, composited UNLIT after lighting
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

// AUTHORED rasters from the terrain's UsdShade Material network (doc 18 §3.1)
// — `inputs:albedo_map` / `inputs:mineral_map`, same slots and same whole-DEM
// planar UV as the static-mesh `terrain_layered.wgsl`. Before these existed a
// streamed site could bake a real NAC orthophoto, wire it through the network,
// and still render pure procedural regolith: the maps only ever bound on the
// static-mesh path, so on a `lodViz = true` site the authored colour was
// invisible. Weight-gated like everything else, so an unbound map (Bevy's
// fallback white) contributes nothing at weight 0.
@group(#{MATERIAL_BIND_GROUP}) @binding(2)
var albedo_tex: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(3)
var albedo_smp: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(4)
var mineral_tex: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(5)
var mineral_smp: sampler;

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

// Layer CUT-OUT threshold, in screen pixels per noise period. A layer costs its
// full FBM (3 × `layer_height`, each octave an 8-tap `vnoise`) on EVERY fragment
// where this is > 0 — so this number, not the ramp width, is what sets the radius
// of the expensive disc around the camera. At 3 px the meso layer alone ran out to
// ~960 m: essentially the whole screen when standing on the surface (~20 vnoise ≈
// 160 hash13 per pixel).
//
// 5 px cuts each layer's disc radius by ~40% (⇒ ~2.8× fewer expensive fragments)
// and drops exactly the band that was closest to aliasing anyway: a 5 px period is
// where value-noise detail stops reading as relief and starts reading as shimmer.
// The baked normal/AO/tone maps carry the relief past the hand-off — the ramp below
// still fades rather than cuts, so nothing pops.
const AA_CUT_PX: f32 = 5.0;
const AA_RAMP_PX: f32 = 7.0;

fn aa_fade(scale: f32, pw: f32) -> f32 {
    let px_per_period = 1.0 / max(scale * pw, 1e-6);
    // Fade a layer out once its period shrinks under `AA_CUT_PX`. The ramp used to
    // be very wide (/22 — FBM carried to ~1 km because nothing else textured the
    // far field, at real fragment cost); now the baked derived maps take over
    // beyond the near field, so the procedural layers can hand off much sooner.
    return saturate((px_per_period - AA_CUT_PX) / AA_RAMP_PX);
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

// Palette-matched to `lod_rgb` in `stream_viz.rs` so the overlay and the debug
// shader agree on what a depth looks like.
fn lod_depth_color(d: f32) -> vec3<f32> {
    // Cycle rather than clamp: modulo guarantees ADJACENT depths always differ,
    // which is all the boundary question needs; clamping blinds the top depths.
    let i = i32(max(d, 0.0)) % 7;
    var p = array<vec3<f32>, 7>(
        vec3(0.20, 0.35, 0.85), vec3(0.20, 0.75, 0.85), vec3(0.25, 0.80, 0.35),
        vec3(0.90, 0.85, 0.20), vec3(0.95, 0.55, 0.15), vec3(0.90, 0.25, 0.20),
        vec3(0.85, 0.30, 0.80),
    );
    return p[i];
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
    @location(9) morph_normal: vec3<f32>,
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
    // Authored rasters. Neutral defaults so a shader compiled without UVs (or
    // with the maps unbound) behaves exactly as before these existed.
    var map_a = vec3(1.0, 1.0, 1.0);
    var map_m = vec3(0.0, 0.0, 0.0);
#ifdef VERTEX_UVS_A
    map_n = textureSample(normal_tex, normal_smp, in.uv);
    map_s = textureSample(surface_tex, surface_smp, in.uv);
    map_a = textureSample(albedo_tex, albedo_smp, in.uv).rgb;
    map_m = textureSample(mineral_tex, mineral_smp, in.uv).rgb;
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

    // AUTHORED albedo (the site's real orthophoto). Applied HERE — after every
    // procedural tone layer, before photometry — so the mosaic is what the sun
    // then lights, and so the micro-grain above still modulates it instead of
    // being erased by it.
    //
    // MODULATES rather than replaces, and the formula is character-for-character
    // the one in `terrain_layered.wgsl`: `mix(albedo, albedo * a * 3.0, w)`.
    // Both paths must agree on what a given `weight_albedo` MEANS, or the same
    // authored scene reads differently depending on whether its site streams —
    // and the ×3 is not arbitrary: the procedural base sits near 0.13 while the
    // baked ortho is a 1–99 percentile stretch, so a plain multiply would render
    // the real photograph as near-black mud.
    if (mat.weight_albedo > 0.0) {
        albedo = mix(albedo, albedo * map_a, mat.weight_albedo);
    }

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
    let fill = base_albedo * (0.02 + 0.03 * max(n_geo.y, 0.0));

    // Regolith is rough and non-metallic; rough_mix nudges it, and the baked
    // slope-derived roughness (surface_tex R) leans in where the maps are live.
    let roughness =
        clamp(mix(0.6 + rough_mix * 0.4, map_s.r, 0.35 * mat.weight_ao), 0.05, 1.0);
    var color = lit_n(in, is_front, n, albedo, roughness, 0.0, fill);

    // Far-field terrain self-shadow: beyond the Sun cascades, sample the
    // pre-baked sun-visibility cache. This keeps distant crater relief from
    // reading as flat unshaded mounds while native CSM handles the near field.
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
    // --- Analysis overlay (Data→Transfer→Blend, in-material shading plane) -----
    // The Blend step: composite the Transfer's colour over the lit regolith. The
    // transfer itself lives in `lunco::transfer` — ONE definition, shared with the
    // Inspector legend and any headless export, so the swatch can't drift from the
    // terrain it explains. Params are uniforms → dropping the cliff angle re-reds
    // steeper ground with no re-bake.
    //
    // SLOPE SOURCE: `n_geo` is the interpolated LOD MESH normal, so on a far/coarse
    // tile — where the mesh has thrown the relief away — a real cliff shaded green
    // and re-coloured as the camera approached. That is the wrong failure direction
    // for a traversability overlay. Where the baked normal map is bound
    // (`weight_normal > 0` — exactly the coarse tiles), take the slope from the
    // DEM-resolution normal instead; near tiles out-resolve the map, so their own
    // geometry stays the better answer.
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
            // LOD-depth view, composited OVER the production shading rather than
            // replacing it (`TerrainShaderMode::DebugLod` swaps in the flat shader,
            // so it cannot show where a detail boundary sits relative to the real
            // look). Coarse -> fine sweeps blue/cyan/green/yellow/orange/red/magenta,
            // matching `lod_rgb` in `stream_viz.rs`.
            tint = lod_depth_color(mat.lod_depth);
        }
        color = vec4(mix(color.rgb, tint, mat.overlay_opacity), color.a);
    }

    // --- AUTHORED overlay drape (the raster half of the same plane) -----------
    // A baked classification raster (slope ramp, hillshade, elevation gradient)
    // from `inputs:mineral_map`, composited at the SAME post-lit point as the
    // computed overlay above, and after it — so the two stack in a defined
    // order instead of racing.
    //
    // UNLIT, and that is the whole point (doc 18 §4): this is a MAP, not a
    // material. It must read the same on a shadowed crater floor as on the
    // sunlit rim, because "where is the ground dangerous" is not a question
    // about where the light happens to be. Tinting albedo instead would put the
    // answer behind the very shadow the student is trying to see into.
    if (mat.weight_mineral > 0.0) {
        color = vec4(mix(color.rgb, map_m, mat.weight_mineral), color.a);
    }
    return color;
}
