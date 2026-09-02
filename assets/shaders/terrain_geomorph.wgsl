//! CDLOD geomorph terrain tile — the production lunar material with a custom
//! `@vertex` morph stage. Macro and meso relief come from the DEM and authored
//! terrain layers; this material adds only one anti-aliased close-range
//! regolith micro-normal and the measured lunar photometric response.
//!
//! Each LOD-tile vertex carries two positions: its own LOD `POSITION` and the
//! `MORPH_TARGET` (the vertex snapped to the parent's coarser even lattice, baked
//! by `bake_tile_mesh`). The vertex shader lerps `POSITION → MORPH_TARGET` by
//! camera distance over the node's CDLOD morph band, so a tile collapses smoothly
//! onto its parent. No texture fetch, no compute → wasm-safe.
//!
//! The micro-normal needs only the terrain's authored DEM extent, so per-tile
//! materials remain independent of the `wire_terrain_materials` heightfield
//! wiring (which only reaches the single static terrain entity). It therefore
//! omits `regolith.wgsl`'s live horizon ray-march. Streamed tiles use
//! the pre-baked DEM horizon cache for terrain self-shadow at every distance and
//! receive Bevy's CSM for shadows cast by dynamic objects.
//!
//! Driven by `ShaderMaterial` (NOT a bespoke material): `m.shader` and
//! `m.vertex_shader` both point here; `m.vertex_shader = Some` makes
//! `ShaderMaterial::specialize` swap the vertex stage and bind
//! `ATTRIBUTE_MORPH_TARGET` at `@location(8)`, `ATTRIBUTE_MORPH_NORMAL` at
//! `@location(9)`, and `ATTRIBUTE_MORPH_EDGE` at `@location(10)`. Params are reflected from
//! `struct Material` like any self-describing shader.

#import bevy_pbr::{
    mesh_functions,
    view_transformations::position_world_to_clip,
    forward_io::VertexOutput,
    mesh_view_bindings::view,
}
#import lunco::pbr_lit::lit_n
#import lunco::terrain::{aa_fade, bump_layer, decode_dem_normal, dem_normal_to_world, map_weights, terrain_detail_normal_to_local, terrain_detail_normal_to_world, terrain_detail_position}
#import lunco::lunar::{regolith_factor, ORTHO_GAIN}

//!@ui      albedo            color  "Albedo"
//!@default albedo            0.13,0.13,0.13
//!@ui      micro_scale       8 80    "Regolith micro scale (/m)"
//!@default micro_scale       35
//!@ui      micro_bump        0 0.05  "Regolith micro-normal strength"
//!@default micro_bump        0.015
//!@ui      roughness         0 1     "Base regolith roughness"
//!@default roughness         0.88
// Derived-map weights come from the screen-space surface footprint and the map's
// physical texel size. Authored-map weights come from USD. Mesh LOD is deliberately
// absent, so replacing a tile by its parent cannot change colour.
//!@default map_texel_size_m 1.0
//!@default derived_surface_on 0
//!@default derived_normal_on  0
//!@default authored_surface_on 0
//!@default authored_normal_on  0
//!@default terrain_half_extent 1.0
//!@ui      weight_albedo     0 1    "Authored albedo (orthophoto) weight"
//!@default weight_albedo     0
//!@ui      weight_rough      0 1    "Authored surface roughness weight"
//!@default weight_rough      0
//!@ui      weight_ao         0 1    "Authored surface AO weight"
//!@default weight_ao         0
//!@ui      weight_normal     0 1    "Authored normal map weight"
//!@default weight_normal     0
// --- lunar photometry (lunco::lunar) ---------------------------------------
// The knobs that decide whether this reads as the Moon or as grey PBR. Defaults
// are the fitted lunar values from the Chrono/UW-Madison sensor simulator
// (arxiv 2410.04371 Table 1), NOT taste: `Bs0 = 1.80238`, `hs = 0.07145 rad`.
// Ranges bracket maria (darker, less backscatter) through highlands.
//!@ui      surge_amp         0 3    "Opposition surge amplitude (Hapke Bs0)"
//!@default surge_amp         1.80
//!@ui      surge_width       0.01 0.3 "Opposition surge width, rad (Hapke hs)"
//!@default surge_width       0.0715
//!@ui      photometry_gain   0.2 2  "Photometry gain (1 = Lambert parity at mu0==mu)"
//!@default photometry_gain   1.0
// The CANONICAL scene sun, picked STRUCTURALLY on the CPU (`pick_sun`) and
// written to every non-terrain ShaderMaterial by `wire_sun_for_non_terrain_materials`
// — streamed tiles included, since a tile carries no `HorizonMap`. This shader
// used to re-derive the sun in-shader with `sun_to_light()`, which picked the
// BRIGHTEST directional light: correct only while the sun happens to be the
// brightest, silent when it is not, and a different answer from the one the
// static-mesh terrain shaders were already using from this very uniform.
//!@engine  sun_dir_world
//!@engine  shadow_cache_on
//!@default morph_start  1.0e20
//!@default morph_end    1.0e21
//!@default stitch_edges 0,0,0,0
struct Material {
    albedo:            vec3<f32>,
    micro_scale:       f32,
    micro_bump:        f32,
    roughness:         f32,
    map_texel_size_m:  f32,  // engine-filled: level-zero map texel spacing in terrain metres
    derived_surface_on: f32, // engine-filled: derived surface map is the selected source
    derived_normal_on:  f32, // engine-filled: derived normal map is the selected source
    authored_surface_on: f32, // engine-filled: USD surface map is the selected source
    authored_normal_on:  f32, // engine-filled: USD normal map is the selected source
    terrain_half_extent: f32, // engine-filled: authored DEM half side in terrain metres
    weight_albedo:     f32,  // AUTHORED albedo raster (orthophoto) over the procedural regolith
    weight_rough:      f32,  // AUTHORED surface roughness weight
    weight_ao:         f32,  // AUTHORED surface AO weight
    weight_normal:     f32,  // AUTHORED normal weight
    surge_amp:         f32,  // Hapke Bs0 — opposition surge amplitude
    surge_width:       f32,  // Hapke hs (rad) — opposition surge angular width
    photometry_gain:   f32,  // trim on the Lommel-Seeliger x surge multiplier
    sun_dir_world:     vec3<f32>,  // engine-filled: world-space to-sun, the canonical scene sun
    shadow_cache_on:   f32,  // engine-filled: 1 = far-shadow cache bound and valid
    morph_start:       f32,  // distance where geomorph toward the parent begins
    morph_end:         f32,  // distance where the parent fully takes over
    stitch_edges:      vec4<f32>, // [top,bottom,left,right] coarser-neighbour mask
}
@group(#{MATERIAL_BIND_GROUP}) @binding(0)
var<uniform> mat: Material;

// Rasters from the terrain's UsdShade Material network (doc 18 §3.1) use the
// same slots and whole-DEM planar UV as the static-mesh terrain shader. The
// CPU material reconciler selects an authored surface/normal role over the
// engine-derived source for that role; the shader only receives one source in
// each fixed slot. Weight-gated like everything else, so an unbound map (Bevy's
// fallback white) contributes nothing at weight 0.
@group(#{MATERIAL_BIND_GROUP}) @binding(2)
var albedo_tex: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(3)
var albedo_smp: sampler;

// Baked derived maps (lunco-terrain-surface derived_layers; whole-DEM planar
// UV). `None` binds Bevy's fallback white — every read is weight-gated so an
// unbound map contributes nothing. surface: R=roughness G=AO B=rockDens
// A=hazard; normal: RGB = DEM-local ENU normal biased, A = albedo scalar
// (0.5 neutral).  The fragment transforms it through the tile instance before
// mixing it with world-space lighting normals.
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
// fetch replaces the live ray-march; gated by `shadow_cache_on`. Streamed
// terrain does not cast into the directional cascade, so this is the one
// terrain-on-terrain self-shadow solution at every distance. Tiles still
// receive the cascade, which carries rover/rock shadows independently.
@group(#{MATERIAL_BIND_GROUP}) @binding(10)
var shadow_cache: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(11)
var shadow_cache_sampler: sampler;

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
    @location(10) edge_mask: vec4<f32>,
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
    // Restricted quadtree selection guarantees that a resident neighbour is
    // either the same depth or one level coarser. On the latter boundary the
    // fine edge uses its parent lattice immediately, the exact surface sampled
    // by the coarser tile. Same-depth edges remain untouched.
    let edge_stitch = max(max(vertex.edge_mask.x * mat.stitch_edges.x,
                              vertex.edge_mask.y * mat.stitch_edges.y),
                          max(vertex.edge_mask.z * mat.stitch_edges.z,
                              vertex.edge_mask.w * mat.stitch_edges.w));
    let m = max(morph, edge_stitch);
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
    var albedo = mat.albedo;

    let world_p = in.world_position.xyz;
    var detail_p = world_p;
    var detail_n = normalize(in.world_normal);
    var n = normalize(in.world_normal);
#ifdef VERTEX_UVS_A
    detail_p = terrain_detail_position(in.uv, mat.terrain_half_extent);
#endif
    // Pixel footprint in world metres (BEFORE any branch — fwidth needs uniform
    // control flow). Drives the footprint-based detail fades.
    let pw = length(fwidth(detail_p));

    // Baked derived maps (sampled unconditionally — uniform control flow; the
    // weight gates make an unbound/fallback map a no-op). UVs are DEM-global.
    var map_n = vec4(0.5, 1.0, 0.5, 0.5);
    var map_s = vec4(0.6, 1.0, 0.0, 0.0);
    // Authored rasters. Neutral defaults so a shader compiled without UVs (or
    // with the maps unbound) behaves exactly as before these existed.
    var map_a = vec3(1.0, 1.0, 1.0);
#ifdef VERTEX_UVS_A
    map_n = textureSample(normal_tex, normal_smp, in.uv);
    map_s = textureSample(surface_tex, surface_smp, in.uv);
    map_a = textureSample(albedo_tex, albedo_smp, in.uv).rgb;
#endif

    // All macro/meso SHAPE comes from the mesh (DEM + crater geometry) and from
    // scattered rock meshes — the fragment no longer fakes relief. It adds only
    // one believable normal-only micro-texture layer (features small enough that
    // the absence of parallax is imperceptible) and lunar photometry.
    // Physical material detail is a property of the map and the projected
    // surface, not of whichever quadtree mesh currently represents it. `pw` is
    // continuous across a CDLOD edge, while an integer tile depth is not.
    let map_footprint = pw / mat.map_texel_size_m;
    // Optional GPU bindings are always populated by the material binder, but
    // their fallback texels are not terrain data. The CPU source bits are the
    // authoritative boundary between authored, derived, and procedural terms.
    let derived_weights = map_weights(map_footprint);
    var weight_normal = derived_weights.x * mat.derived_normal_on;
    var weight_ao = derived_weights.y * mat.derived_surface_on;
    var weight_rough = 0.35 * derived_weights.y * mat.derived_surface_on;
    var weight_tone = derived_weights.z * mat.derived_normal_on;
    if (mat.authored_normal_on > 0.5) {
        weight_normal = mat.weight_normal;
        weight_tone = 0.0;
    }
    if (mat.authored_surface_on > 0.5) {
        weight_rough = mat.weight_rough;
        weight_ao = mat.weight_ao;
    }

    // Baked meso normal: once a screen pixel covers roughly a map texel, the map
    // carries stable filtered crater slopes. Below that physical scale the mesh
    // and procedural close-detail layers remain sharper, so the map fades out.
    if (weight_normal > 0.0) {
        // The derived map is baked from the DEM oracle, whose coordinates are
        // site-local ENU.  `n` is in Bevy's current render world.  Mixing the
        // two directly only happened to work while the site grid was aligned
        // with that world; after a Moon/Earth camera-frame change it made the
        // coarse LODs (where this weight rises) shade from a different sun
        // angle, often all the way to black.  The tile instance is the
        // authoritative local->render transform, including BigSpace's current
        // floating-origin frame.
        let n_map = dem_normal_to_world(map_n.xyz, in.instance_index);
        n = normalize(mix(n, n_map, weight_normal));
    }

#ifdef VERTEX_UVS_A
    detail_n = terrain_detail_normal_to_local(n, in.instance_index);
#endif

    // Regolith grain is a material response, not terrain shape. Keep one
    // footprint-filtered layer so it disappears before it can alias or shimmer.
    let micro_scale = clamp(mat.micro_scale, 8.0, 80.0);
    let micro_fade = aa_fade(micro_scale, pw);
    var micro_h = 0.5;
    if (micro_fade > 0.0) {
        detail_n = bump_layer(
            detail_n, detail_p, micro_scale, 2, 0.5, 0.42, 0.58,
            mat.micro_bump * micro_fade, &micro_h);
    }

    n = detail_n;
#ifdef VERTEX_UVS_A
    n = terrain_detail_normal_to_world(detail_n, in.instance_index);
#endif

    // Baked relief tone: rims/ejecta brighter, bowls darker (normal_tex alpha,
    // 0.5 = neutral). This is what keeps distant relief legible after the
    // procedural layers and even the mesh detail have faded out.
    albedo *= 1.0 + (map_n.a - 0.5) * (0.6 * weight_tone);

    // Baked ambient occlusion: crater interiors and valley floors receive less
    // sky/bounce light. Darkens the diffuse base rather than the direct sun
    // term (lit_n owns that), which visually matches at the distances where
    // this weight is raised.
    var map_ao = mix(1.0, 0.4 + 0.6 * map_s.g, weight_ao);
    if (mat.authored_surface_on > 0.5) {
        map_ao = mix(1.0, map_s.g, weight_ao);
    }
    albedo *= map_ao;

    // AUTHORED albedo (the site's real orthophoto). Applied HERE — after every
    // procedural tone layer, before photometry — so the mosaic is what the sun
    // then lights, and so the micro-grain above still modulates it instead of
    // being erased by it.
    //
    // MODULATES rather than replaces, and the formula must stay character-for-
    // character the one in `terrain_layered.wgsl`. Both paths must agree on what
    // a given `weight_albedo` MEANS, or the same authored scene reads differently
    // depending on whether its site streams.
    //
    // ORTHO_GAIN is what makes the multiply energy-preserving, and it is NOT
    // arbitrary. The bake (`process.rs`, `kind = "map"`) writes a 1–99 PERCENTILE
    // STRETCH: it spends the full 0..255 range on the site's own brightness
    // spread, so the result is a CONTRAST map whose mean lands near 1/3 — not a
    // reflectance map with mean 1. Multiplying by it therefore does not tint the
    // regolith, it DIMS it by that mean.
    //
    // Measured on the shipped Apollo 15 ortho (2500², 2026-07-26): mean over real
    // measurements = 0.412, so a plain multiply renders the authored 0.13 lunar
    // albedo at 0.054 — 41% of it, a permanent ~1.3-stop underexposure of the
    // ground and nothing else. That is the "near-black mud" this comment used to
    // warn about while the code below did the plain multiply anyway.
    //
    // The exact normaliser is 1/mean ≈ 2.43; 3.0 is the authored round number and
    // lands at 0.161 (124% of lunar), which is the right side of correct for a
    // stretch whose mean drifts per site. If a future bake normalises the map to
    // unit mean, this constant goes to 1.0 and the comment goes with it.
    if (mat.weight_albedo > 0.0) {
        albedo = mix(albedo, albedo * map_a * ORTHO_GAIN, mat.weight_albedo);
    }

    // --- Lunar photometry: the actual realism lever -----------------------
    // Lommel-Seeliger + opposition surge (retroreflective backscatter) from the
    // scene sun — read from the light bindings, so streamed tiles get it WITHOUT
    // the per-material sun wiring the static mesh needs. Pre-multiplies albedo;
    // lit_n's Lambert + the two-owner shadow pipeline complete the response. This is what makes the
    // surface read as the Moon (flat, then a bright surge toward opposition)
    // instead of generic grey PBR.
    // Same guard as the static-mesh path: before the wiring system has run the
    // uniform is zero, and `normalize` of that is NaN — which would propagate
    // through the albedo multiply and paint black holes across the terrain.
    let V = normalize(view.world_position - world_p);
    var lunar_k = 1.0;
    let sw = mat.sun_dir_world;
    if (dot(sw, sw) > 0.25) {
        lunar_k = regolith_factor(
            n, normalize(sw), V, mat.surge_amp, mat.surge_width, mat.photometry_gain);
    }
    let base_albedo = albedo;
    albedo = albedo * lunar_k;

    // Shadow fill: a whisper of hemispheric bounce (earthshine + regolith
    // inter-reflection) rides the emissive slot so hard shadows don't crush to
    // pure black — shadowed crater floors stay readable without washing out the
    // raking-light contrast that sells the surface. Hemispheric fill is a
    // half-sky integral — insensitive to micro-relief — so it reads the
    // GEOMETRIC normal, not the bump-perturbed one: driving it with `n` turned
    // every shadowed slope into high-contrast micro-speckle static.
    let n_geo = normalize(in.world_normal);
    let fill = base_albedo * (0.02 + 0.03 * max(n_geo.y, 0.0));

    // Regolith is rough and non-metallic; the baked slope-derived roughness
    // (surface_tex R) replaces the base only where its source contract is active.
    let roughness = clamp(
        mix(mat.roughness, map_s.r, weight_rough),
        0.05,
        1.0,
    );
    var color = lit_n(in, is_front, n, albedo, roughness, 0.0, fill);

    // Terrain self-shadow: sample the pre-baked sun-visibility cache at every
    // distance. The directional cascade intentionally contains dynamic casters
    // but not streamed terrain, so multiplying the two combines independent
    // occluder sets instead of double-shadowing the ground.
#ifdef VERTEX_UVS_A
    if (mat.shadow_cache_on > 0.5) {
        let vis = textureSampleLevel(shadow_cache, shadow_cache_sampler, in.uv, 0.0).r;
        color = vec4(color.rgb * vis, color.a);
    }
#endif
    return color;
}
