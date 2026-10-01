//! Layered lunar terrain material — `regolith.wgsl` + non-destructive map layers.
//!
//!@interface lunco.lunar-surface-continuation.v1
//!
//! The procedural regolith (DEM-anchored micro detail + lunar BRDF + heightfield
//! shadow march) is the **floor**: even where a layer map is low-res or absent
//! the rover camera still sees real close-range detail. Larger procedural relief
//! remains available for non-measured regolith, while measured DEM geometry owns
//! its landform normals. Footprint-filtered sub-metre clumps and grain remain
//! shading detail on measured terrain. On top ride UV-registered raster **layers**
//! (design `terrain-layered-pipeline-design.md`
//! Part C.2), each blended by a reflected `weight_*` knob:
//!
//!   * albedo  (binding 2/3) — real colour raster (e.g. the NASA lunar mosaic
//!     downloaded via `Assets.toml`); it owns the colour frequencies it resolves.
//!     Filtered sub-texel grain adds close detail around that colour's mean.
//!   * mineral (binding 4/5) — classification/analysis OVERLAY (e.g. the LROC
//!     slope map): composited UNLIT after lighting/shadowing, so it stays
//!     readable in shadow (doc 18 §4 — overlays are data, not material).
//!   * surface (binding 6/7) — packed R=roughness G=AO B=rockDens A=hazard.
//!   * normal  (binding 8/9) — meso-scale normal (DEM-derived Sobel) perturbing
//!     the procedural bump normal.
//!
//! A `weight_* = 0` layer contributes nothing. Maps are sampled by the planar
//! UV the horizon bake establishes, under `#ifdef VERTEX_UVS_A` (the shadow
//! march uses the same guard); without UVs the material is procedural.
//!
//! Self-describing: the engine reflects `struct Material` (field → std140 offset)
//! and the `//!@` annotations, so every `weight_*` is a free Inspector slider /
//! `SetObjectProperty` target and the layout hot-reloads on shader edit.

#import bevy_pbr::{
    forward_io::VertexOutput,
    pbr_types,
    pbr_functions,
    mesh_functions,
    mesh_bindings::mesh,
    mesh_view_bindings::view,
    mesh_view_bindings::lights,
}
#import lunco::horizon::sun_visibility_resolved
#import lunco::lunar::regolith_factor
#import lunco::terrain::{aa_fade, bump_layer, dem_normal_to_world, filter_detail_roughness, layer_height, ramp, surface_fbm, terrain_apply_sun_response, terrain_detail_normal_to_local, terrain_detail_normal_to_world, terrain_detail_position, terrain_map_weights, terrain_surface_occlusion}

//!@ui      albedo            color       "Albedo"
//!@default albedo            0.13,0.13,0.13
//!@ui      micro_scale       8 80        "Regolith micro scale (/m)"
//!@default micro_scale       35
//!@ui      micro_bump        0 0.05      "Regolith micro-relief amplitude (m)"
//!@default micro_bump        0.0005
//!@ui      micro_albedo      0 0.2       "Regolith micro-albedo strength"
//!@default micro_albedo      0.002
//!@ui      roughness         0 1         "Base regolith roughness"
//!@default roughness         0.88
//!@ui      macro_clump_scale 1 20        "Macro clump scale (/m)"
//!@default macro_clump_scale 8
//!@ui      macro_bump        0 0.3       "Macro bump strength"
//!@default macro_bump        0.002
//!@ui      mid_scale         0.02 1      "Mid hummock scale (/m)"
//!@default mid_scale         0.15
//!@ui      mid_bump          0 1.5       "Mid hummock strength"
//!@default mid_bump          0.01
//!@ui      fine_scale        50 400      "Fine grain scale (/m)"
//!@default fine_scale        180
//!@ui      fine_bump         0 0.1       "Fine grain strength"
//!@default fine_bump         0.0005
//!@ui      rough_mix         0 1         "Roughness mix"
//!@default rough_mix         0.35
//!@ui      mottle            0 0.6       "Albedo mottle"
//!@default mottle            0.02
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
// Derived-map weights are resolved from the screen-space surface footprint and
// explicit CPU source bits. The same contract is used by streamed geomorph
// tiles, so static and streamed terrain cannot shade the same site differently.
//!@default map_texel_size_m 1.0
//!@default derived_surface_on 0
//!@default derived_normal_on  0
//!@default authored_surface_on 0
//!@default authored_normal_on  0
// --- lunar photometry (lunco::lunar) ---------------------------------------
// Fitted lunar values (Chrono/UW-Madison, arxiv 2410.04371 Table 1), not taste.
// `terrain_geomorph.wgsl` is only the optional vertex stage for this fragment;
// it must share this Material ABI but does not define another appearance path.
//!@ui      surge_amp         0 3         "Opposition surge amplitude (Hapke Bs0)"
//!@default surge_amp         1.80
//!@ui      surge_width       0.01 0.3    "Opposition surge width, rad (Hapke hs)"
//!@default surge_width       0.0715
//!@ui      photometry_gain   0.2 2       "Photometry gain (1 = Lambert parity at mu0==mu)"
//!@default photometry_gain   1.0
//!@engine  sun_dir
//!@engine  sun_dir_world
//!@engine  sun_tan_radius
//!@engine  hf_size
//!@engine  hf_res
//!@engine  csm_far
//!@engine  shadow_cache_on
//!@engine  horizon_march_steps
//!@default map_texel_size_m 1.0
//!@default derived_surface_on 0
//!@default derived_normal_on  0
//!@default authored_surface_on 0
//!@default authored_normal_on  0
//!@default terrain_half_extent 1.0
//!@engine  site_blend_widths_m
//!@engine site_photometry
//!@default site_photometry 0,1,1,0
//!@engine  site_base_color
//!@default site_base_color 0,0,0,0
//!@engine  site_weight_albedo
//!@engine  site_weight_rough
//!@engine  site_weight_ao
//!@default site_blend_widths_m 1.0,1.0,1.0,1.0
//!@default site_weight_albedo 0.0
//!@default site_weight_rough 0.0
//!@default site_weight_ao 0.0
//!@default morph_start  1.0e20
//!@default morph_end    1.0e21
//!@default stitch_edges 0,0,0,0
// `info:wgsl:vertexAsset` can pair this fragment with `terrain_geomorph.wgsl`.
// Both stages therefore declare this exact uniform ABI; the cross-file contract
// test in `lunco-materials` prevents either stage from silently drifting.
struct Material {
    albedo:            vec3<f32>,
    micro_scale:       f32,
    micro_bump:        f32,
    micro_albedo:      f32,
    roughness:         f32,
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
    surge_amp:         f32,  // Hapke Bs0 — opposition surge amplitude
    surge_width:       f32,  // Hapke hs (rad) — opposition surge angular width
    photometry_gain:   f32,  // trim on the Lommel-Seeliger x surge multiplier
    sun_tan_radius:    f32,  // engine-filled: tan(sun angular radius)
    sun_dir:           vec3<f32>,  // engine-filled: terrain-local to-sun dir
    morph_start:      f32,  // engine-filled: CDLOD morph start distance
    sun_dir_world:     vec3<f32>,  // engine-filled: world-space to-sun (lunar BRDF)
    morph_end:        f32,  // engine-filled: CDLOD morph end distance
    hf_size:           vec2<f32>,  // engine-filled: heightfield extent (m)
    hf_res:            f32,  // engine-filled: heightfield resolution
    terrain_geometry_on: f32, // engine-filled: measured DEM owns relief
    csm_far:           f32,  // engine-filled: native terrain shadow range; 0 = heightfield everywhere
    shadow_cache_on:   f32,  // engine-filled: 1 = sample pre-baked shadow cache, 0 = ray-march
    horizon_march_steps: f32, // engine-filled: configured live ray-march iterations
    map_texel_size_m:  f32,  // engine-filled: level-zero map spacing (m)
    derived_surface_on: f32, // engine-filled: derived surface is active
    derived_normal_on:  f32, // engine-filled: derived normal is active
    authored_surface_on: f32, // engine-filled: authored surface is active
    authored_normal_on:  f32, // engine-filled: authored normal is active
    terrain_half_extent: f32, // engine-filled: streamed DEM half extent (m)
    site_weight_albedo: f32, // engine-filled: site-local albedo contribution
    site_weight_rough: f32, // engine-filled: site-local roughness contribution
    site_weight_ao: f32, // engine-filled: site-local ambient-occlusion contribution
    site_blend_widths_m: vec4<f32>, // engine-filled: visual-only site-to-globe material blend
    stitch_edges:     vec4<f32>, // engine-filled: coarser-neighbour edge mask
    site_base_color: vec4<f32>,
    site_photometry: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0)
var<uniform> mat: Material;

// Terrain heightfield (R32Float, world-space heights) for the sun-shadow march.
@group(#{MATERIAL_BIND_GROUP}) @binding(1)
var height_map: texture_2d<f32>;

// Pre-baked horizon shadow cache (R8Unorm, 0..1 sun visibility) — sampled
// with a single `textureSampleLevel` when `mat.shadow_cache_on > 0.5` instead
// of the configured heightfield ray-march. Filterable (GPU bilinear interp).
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
// Site maps are independent from the body-wide Albedo/Surface layers. The
// collar blends these local DEM inputs into the material selected for the body.
@group(#{MATERIAL_BIND_GROUP}) @binding(12)
var continuation_albedo_tex: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(13)
var continuation_albedo_smp: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(14)
var continuation_surface_tex: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(15)
var continuation_surface_smp: sampler;

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> @location(0) vec4<f32> {
    let macro_scale = mat.macro_clump_scale;
    let fine_scale  = mat.fine_scale;
    let micro_scale = mat.micro_scale;
    let macro_bump  = mat.macro_bump;
    let fine_bump   = mat.fine_bump;
    let micro_bump  = mat.micro_bump;
    let micro_albedo = mat.micro_albedo;
    let rough_mix   = mat.rough_mix;
    let mid_scale   = mat.mid_scale;
    let mid_bump    = mat.mid_bump;
    let mottle      = mat.mottle;
    let authored_albedo_weight = clamp(mat.weight_albedo, 0.0, 1.0);
    let procedural_albedo_weight = 1.0 - authored_albedo_weight;
    var albedo = mat.albedo;
    var site_material_weight = 0.0;

    let world_p = in.world_position.xyz;
    let dist = distance(view.world_position, world_p);
    var detail_p = world_p;
    var detail_n = normalize(in.world_normal);
#ifdef VERTEX_UVS_A
    detail_p = terrain_detail_position(in.uv, mat.hf_size.x * 0.5);
    detail_n = terrain_detail_normal_to_local(detail_n, in.instance_index);
#endif
    let pw = length(fwidth(detail_p));
    let fine_fade  = aa_fade(fine_scale, pw);
    let macro_fade = aa_fade(macro_scale, pw);
    let mid_fade   = aa_fade(mid_scale, pw);
    let micro_fade = aa_fade(micro_scale, pw);

    // Footprint-filtered micro-detail is the shading-only close-range floor.
    var mid_h = 0.5;
    var macro_h = 0.5;
    var fine_h = 0.5;
    var micro_h = 0.5;
    if (micro_fade > 0.0) {
        detail_n = bump_layer(detail_n, detail_p, micro_scale, 1, 0.5, 0.1, 0.9, micro_bump * micro_fade, &micro_h);
    }
    // Measured geometry owns landform relief. Sub-metre clumps and grain remain
    // shading detail; the mid band is reserved for non-measured surfaces.
    if (mat.terrain_geometry_on < 0.5 && mid_fade > 0.0) {
        detail_n = bump_layer(detail_n, detail_p, mid_scale, 4, 0.55, 0.35, 0.65, mid_bump * mid_fade, &mid_h);
    }
    if (macro_fade > 0.0) {
        detail_n = bump_layer(detail_n, detail_p, macro_scale, 2, 0.6, 0.1, 0.9, macro_bump * macro_fade, &macro_h);
    }
    if (fine_fade > 0.0) {
        detail_n = bump_layer(detail_n, detail_p, fine_scale, 3, 0.5, 0.1, 0.9, fine_bump * fine_fade, &fine_h);
    }

    var n = detail_n;
#ifdef VERTEX_UVS_A
    n = terrain_detail_normal_to_world(detail_n, in.instance_index);
#endif

    // Large-scale procedural colour is a fallback only. With an authored
    // orthophoto, adding independent per-metre variation over it makes the
    // surface change between unrelated tones as the camera footprint and LOD
    // change. Sub-texel grain, normals, and roughness remain independent.
    if (procedural_albedo_weight > 0.0) {
        let dust_fade = aa_fade(0.008, pw);
        if (dust_fade > 0.0) {
            let dust = surface_fbm(detail_p * 0.008, 2, 0.5);
            albedo *= 1.0 + (dust - 0.5) * 0.04 * dust_fade * procedural_albedo_weight;
        }
        albedo *= 1.0
            + (mix(0.5, mid_h, mid_fade) - 0.5)
                * mottle
                * procedural_albedo_weight;
    }
    let macro_rough = mix(0.5, macro_h, macro_fade);
    var roughness = clamp(mat.roughness + (macro_rough - 0.5) * rough_mix, 0.05, 1.0);

    // ── Non-destructive raster layers (planar UV; weight 0 = no contribution).
    // Guarded by VERTEX_UVS_A: with no UVs we stay pure procedural.
#ifdef VERTEX_UVS_A
    let uv = in.uv;
    var map_n = textureSample(normal_tex, normal_smp, uv);
    var map_s = textureSample(surface_tex, surface_smp, uv);
    let map_footprint = pw / mat.map_texel_size_m;
    let outside_site_uv = vec2(
        max(max(-uv.x, uv.x - 1.0), 0.0),
        max(max(-uv.y, uv.y - 1.0), 0.0),
    );
    // The exterior material uses the same rectangle as the geometry.
    let outside_m = outside_site_uv * 2.0 * mat.terrain_half_extent;
    let width_x = select(mat.site_blend_widths_m.y, mat.site_blend_widths_m.x, uv.x < 0.0);
    let width_z = select(mat.site_blend_widths_m.z, mat.site_blend_widths_m.w, uv.y < 0.0);
    let transition = max(outside_m.x / max(width_x, 1e-6), outside_m.y / max(width_z, 1e-6));
    site_material_weight = 1.0 - smoothstep(0.0, 1.0, transition);
    let map_weights = terrain_map_weights(
        map_footprint,
        mat.derived_surface_on,
        mat.derived_normal_on,
        mat.authored_surface_on,
        mat.authored_normal_on,
        authored_albedo_weight,
        mat.weight_rough,
        mat.weight_ao,
        mat.weight_normal,
    );
    let map_weight_normal = map_weights.x;
    let map_weight_rough = map_weights.y;
    let map_weight_ao = map_weights.z;
    let map_weight_tone = map_weights.w;
    var map_ao = 1.0;
    // Albedo is already a linear material colour. The asset pipeline separates
    // source-image illumination from local surface detail before writing this
    // sRGB-authored texture; this shader must not relight the source image.
    if (authored_albedo_weight > 0.0) {
        let a = textureSample(albedo_tex, albedo_smp, uv).rgb;
        albedo = mix(albedo, a, authored_albedo_weight);
    }
    var continuation_color = mat.site_base_color.rgb;
    if (mat.site_weight_albedo > 0.0) {
        let site_uv = clamp(uv, vec2(0.0), vec2(1.0));
        let site_albedo = textureSample(
            continuation_albedo_tex, continuation_albedo_smp, site_uv).rgb;
        continuation_color = mix(continuation_color, site_albedo, clamp(mat.site_weight_albedo, 0.0, 1.0));
    }
    albedo = mix(albedo, continuation_color, mat.site_base_color.a * site_material_weight);
    // (Mineral/classification is NOT applied here: it is an OVERLAY — data
    // visualization, not material — and composites after lighting below, so a
    // slope-class drape stays readable inside the crater's shadow. Tinting the
    // albedo here would multiply it through sun, CSM and the shadow march —
    // the exact bug doc 18 §4 removes.)
    // Surface pack: R=roughness, G=AO (B=rockDens, A=hazard consumed elsewhere).
    if (map_weight_rough > 0.0 || map_weight_ao > 0.0) {
        roughness = clamp(mix(roughness, map_s.r, map_weight_rough), 0.05, 1.0);
        map_ao = terrain_surface_occlusion(
            map_s, map_weight_ao, mat.authored_surface_on);
    }
    if (mat.site_weight_rough > 0.0 || mat.site_weight_ao > 0.0) {
        let site_uv = clamp(uv, vec2(0.0), vec2(1.0));
        let site_surface = textureSample(
            continuation_surface_tex, continuation_surface_smp, site_uv);
        roughness = clamp(mix(
            roughness,
            site_surface.r,
            clamp(mat.site_weight_rough, 0.0, 1.0) * site_material_weight,
        ), 0.05, 1.0);
        map_ao = mix(
            map_ao,
            site_surface.g,
            clamp(mat.site_weight_ao, 0.0, 1.0) * site_material_weight,
        );
    }
    // The DEM map owns the landform band, independently of the grain band.
    // Recover the detail's tangent slope around the geometric normal, then
    // project it onto the selected landform normal. A full-weight DEM must not
    // erase grain or inherit camera-dependent tile-normal interpolation.
    if (map_weight_normal > 0.0) {
        let n_baked = dem_normal_to_world(
            map_n.xyz, in.instance_index);
        let geometric_n = normalize(in.world_normal);
        let detail_slope = geometric_n - n / max(dot(n, geometric_n), 1e-4);
        let landform_n = normalize(mix(geometric_n, n_baked, map_weight_normal));
        let landform_slope = detail_slope - landform_n * dot(detail_slope, landform_n);
        n = normalize(landform_n - landform_slope);
    }
    // The derived normal's alpha carries the same DEM-anchored relief tone as
    // the streamed path. Authored normal maps intentionally do not supply it.
    albedo *= 1.0 + (map_n.a - 0.5) * (0.6 * map_weight_tone);
#endif

    // Resolved sub-texel grain modulates the material colour around its mean.
    // The same stable position and footprint fade used for its normal prevent
    // view-dependent speckle; broad colour remains owned by the orthophoto.
    if (micro_fade > 0.0 && micro_albedo > 0.0) {
        var grain_source_weight = 1.0;
#ifdef VERTEX_UVS_A
        let albedo_texel_m = mat.hf_size.x / f32(textureDimensions(albedo_tex).x);
        grain_source_weight = mix(1.0,
            smoothstep(1.0, 2.0, micro_scale * albedo_texel_m), authored_albedo_weight);
#endif
        albedo *= 1.0 + (micro_h - 0.5) * micro_albedo * micro_fade * grain_source_weight;
    }

    // Keep the high-frequency normal detail stable as it leaves the pixel
    // footprint. Its unresolved slope variance becomes GGX roughness instead
    // of vanishing at the anti-aliasing fade.
    roughness = filter_detail_roughness(roughness, micro_bump, micro_scale, micro_fade);
    roughness = filter_detail_roughness(roughness, macro_bump, macro_scale, macro_fade);
    roughness = filter_detail_roughness(roughness, fine_bump, fine_scale, fine_fade);

    var pbr_input = pbr_types::pbr_input_new();
    pbr_input.flags = mesh[in.instance_index].flags;
    pbr_input.frag_coord = in.position;
    pbr_input.world_position = in.world_position;
    pbr_input.world_normal = pbr_functions::prepare_world_normal(
        normalize(in.world_normal), false, is_front);
    pbr_input.is_orthographic = view.clip_from_view[3].w == 1.0;
    pbr_input.N = n;
    pbr_input.V = pbr_functions::calculate_view(in.world_position, pbr_input.is_orthographic);
    // AO belongs to indirect diffuse light. Keeping it out of base colour
    // preserves the authored orthophoto and prevents the derived low-frequency
    // field from becoming broad albedo patches or suppressing direct sunlight.
    pbr_input.diffuse_occlusion = vec3(map_ao);
    var lunar_k = 1.0;
    let sw = mat.sun_dir_world;
    if (dot(sw, sw) > 0.25) {
        lunar_k = regolith_factor(
            pbr_input.N, normalize(sw), pbr_input.V,
            mix(mat.surge_amp, mat.site_photometry.x, mat.site_base_color.a * site_material_weight),
            mix(mat.surge_width, mat.site_photometry.y, mat.site_base_color.a * site_material_weight),
            mix(mat.photometry_gain, mat.site_photometry.z, mat.site_base_color.a * site_material_weight));
    }
    pbr_input.material.base_color = vec4(albedo, 1.0);
    pbr_input.material.perceptual_roughness = roughness;
    pbr_input.material.metallic = 0.0;
    pbr_input.material.reflectance = vec3(0.5);

    var color = pbr_functions::apply_pbr_lighting(pbr_input);
    var sun_vis = 1.0;
    var march_blend = 0.0;

#ifdef VERTEX_UVS_A
    let csm_far = mat.csm_far;
    march_blend = 1.0;
    if (csm_far > 0.0) {
        march_blend = smoothstep(csm_far, csm_far * 1.1, dist);
    }
    if (march_blend > 0.0) {
        sun_vis = sun_visibility_resolved(
            shadow_cache, shadow_cache_sampler, mat.shadow_cache_on,
            height_map, in.uv, mat.sun_dir, mat.sun_tan_radius,
            mat.horizon_march_steps, mat.hf_size, mat.hf_res);
    }
#endif

    color = terrain_apply_sun_response(
        pbr_input, color, mat.sun_dir_world, lunar_k, sun_vis, march_blend);

#ifdef VERTEX_UVS_A
    // ── Overlay plane (UNLIT, doc 18 §4): the mineral/classification drape
    // composites over the LIT result — after PBR, after the sun march, after
    // shadow fill — and is never multiplied by any of them. Same composite
    // point as the separate terrain diagnostic material, so texture overlays
    // and the diagnostic view use the same transfer definition. The drape is the
    // map's own colour (e.g. the LROC slope classes), not an albedo tint:
    // its whole job is to stay readable where the light does not reach.
    if (mat.weight_mineral > 0.0) {
        let m = textureSample(mineral_tex, mineral_smp, uv).rgb;
        color = vec4(mix(color.rgb, m, mat.weight_mineral), color.a);
    }
#endif

    color = pbr_functions::main_pass_post_lighting_processing(pbr_input, color);
    return color;
}
