//! Blueprint grid material — self-describing `ShaderMaterial` (replaces the old
//! hand-rolled `ExtendedMaterial` in `blueprint.rs`).
//!
//! Two grid modes blended by `transition` (0 → 1):
//!   * `transition < 0.5` — **spherical lat/long grid** derived per fragment from
//!     the body's radial direction. Used by celestial Earth/Moon tiles seen from
//!     orbit. Needs the `LUNCO_GLOBE_DIRECTION` vertex interface below.
//!   * `transition >= 0.5` — **Cartesian XZ blueprint grid** over world position.
//!     Used by the flat sandbox ground. Always available (no UVs needed).
//!
//! The base colour is `surface_color` multiplied by the optional `albedo_map`
//! (binding 2/3 — Bevy's white fallback when unbound, so a solid-colour ground
//! is `surface_color` and a textured planet tile is the imagery). Lighting is the
//! shared `lunco::pbr_lit::lit` path (full Bevy PBR — directional sun, shadows,
//! tonemapping) — no `StandardMaterial` inheritance needed.
//!
//! Self-describing: the engine reflects `struct Material` (field → std140 offset)
//! + the `//!@` annotations, so every knob is a free Inspector slider /
//! `SetObjectProperty` target / USD `primvars:<field>`, and it hot-reloads on edit.

#import bevy_pbr::{
    mesh_functions,
    view_transformations::position_world_to_clip,
    forward_io::VertexOutput,
}
#import lunco::pbr_lit::lit

#ifdef LUNCO_GLOBE_DIRECTION
struct GlobeVertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(11) globe_direction: vec3<f32>,
};

struct GlobeVertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) world_position: vec4<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(5) globe_direction: vec3<f32>,
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    @location(6) @interpolate(flat) instance_index: u32,
#endif
#ifdef VISIBILITY_RANGE_DITHER
    @location(7) @interpolate(flat) visibility_range_dither: i32,
#endif
};

@vertex
fn vertex(vertex: GlobeVertex) -> GlobeVertexOutput {
    var out: GlobeVertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    out.world_position = mesh_functions::mesh_position_local_to_world(
        world_from_local,
        vec4<f32>(vertex.position, 1.0),
    );
    out.position = position_world_to_clip(out.world_position.xyz);
    out.world_normal = mesh_functions::mesh_normal_local_to_world(
        vertex.normal,
        vertex.instance_index,
    );
    out.globe_direction = vertex.globe_direction;
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    out.instance_index = vertex.instance_index;
#endif
#ifdef VISIBILITY_RANGE_DITHER
    out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(
        vertex.instance_index,
        world_from_local[3],
    );
#endif
    return out;
}

fn globe_pbr_input(in: GlobeVertexOutput) -> VertexOutput {
    var out: VertexOutput;
    out.position = in.position;
    out.world_position = in.world_position;
    out.world_normal = in.world_normal;
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    out.instance_index = in.instance_index;
#endif
#ifdef VISIBILITY_RANGE_DITHER
    out.visibility_range_dither = in.visibility_range_dither;
#endif
    return out;
}
#endif

//!@ui      surface_color    color "Surface colour"
//!@default surface_color    0.2,0.2,0.2
//!@ui      roughness        0 1   "Roughness"
//!@default roughness        0.9
//!@ui      high_line_color  color "Line colour (high alt / sphere)"
//!@default high_line_color  0.0,0.5,1.0
//!@ui      transition       0 1   "Transition (sphere 0 ↔ grid 1)"
//!@default transition       0.85
//!@ui      low_line_color   color "Line colour (low alt / grid)"
//!@default low_line_color   0.0,0.5,1.0
//!@ui      minor_line_fade  0 1   "Minor line fade"
//!@default minor_line_fade  0.3
//!@default subdivisions     10,10
//!@default fade_range       0.2,0.6
//!@ui      line_width       0 8   "Line width (sphere px)"
//!@default line_width       2.0
//!@ui      major_grid_spacing 0.1 5000 "Major grid spacing (m)"
//!@default major_grid_spacing 1.0
//!@ui      minor_grid_spacing 0.1 5000 "Minor grid spacing (m)"
//!@default minor_grid_spacing 0.5
//!@ui      major_line_width 0 4   "Major line width (px)"
//!@default major_line_width 0.75
//!@ui      minor_line_width 0 4   "Minor line width (px)"
//!@default minor_line_width 0.4
struct Material {
    surface_color:      vec3<f32>,
    roughness:          f32,
    high_line_color:    vec3<f32>,
    transition:         f32,
    low_line_color:     vec3<f32>,
    minor_line_fade:    f32,
    subdivisions:       vec2<f32>,
    fade_range:         vec2<f32>,
    line_width:         f32,
    major_grid_spacing: f32,
    minor_grid_spacing: f32,
    major_line_width:   f32,
    minor_line_width:   f32,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0)
var<uniform> mat: Material;

// Optional albedo raster (planet imagery). `None` → Bevy's white fallback, so a
// solid-colour ground stays `surface_color`. Same slot as ShaderMaterial.albedo_map.
@group(#{MATERIAL_BIND_GROUP}) @binding(2)
var albedo_tex: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(3)
var albedo_smp: sampler;

#ifdef LUNCO_GLOBE_DIRECTION
fn equirectangular_uv(d: vec3<f32>) -> vec2<f32> {
    return vec2(
        atan2(-d.z, d.x) / (2.0 * 3.141592653589793) + 0.5,
        0.5 - asin(clamp(d.y, -1.0, 1.0)) / 3.141592653589793,
    );
}

// Analytic derivative of the equirectangular projection. Differentiating the
// body direction rather than the wrapped UV has two important properties:
//   * longitude remains continuous for mip selection at the anti-meridian;
//   * the pole singularity produces a large, physically correct footprint and
//     therefore selects a coarse mip instead of requiring an arbitrary polar cap.
// The guards apply only at the mathematical coordinate singularity.
fn equirectangular_grad(d: vec3<f32>, delta: vec3<f32>) -> vec2<f32> {
    let radial_sq = max(d.x * d.x + d.z * d.z, 1e-8);
    let latitude_scale = sqrt(max(1.0 - d.y * d.y, 1e-8));
    return vec2(
        (d.z * delta.x - d.x * delta.z) / (2.0 * 3.141592653589793 * radial_sq),
        -delta.y / (3.141592653589793 * latitude_scale),
    );
}
#endif

fn shade(in: VertexOutput, is_front: bool, globe_direction: vec3<f32>) -> vec4<f32> {
    var base = mat.surface_color;
    var grid_mask = 0.0;
#ifdef LUNCO_GLOBE_DIRECTION
    let globe_direction_unit = normalize(globe_direction);
    let globe_uv = equirectangular_uv(globe_direction_unit);
    let globe_dx = equirectangular_grad(globe_direction_unit, dpdx(globe_direction_unit));
    let globe_dy = equirectangular_grad(globe_direction_unit, dpdy(globe_direction_unit));
#endif

    if (mat.transition < 0.5) {
        // --- Lat/Long grid (spherical bodies). Direction is interpolated in 3D
        // and normalised BEFORE projection, so this mapping is independent of
        // tile tessellation and quadtree level.
#ifdef LUNCO_GLOBE_DIRECTION
        let img = textureSampleGrad(albedo_tex, albedo_smp, globe_uv, globe_dx, globe_dy).rgb;
        base *= img;
        let ll_coords = globe_uv * mat.subdivisions;
        let ll_f = abs(fract(ll_coords - 0.5) - 0.5) / fwidth(ll_coords);
        let ll_line = min(ll_f.x, ll_f.y);
        let ll_fade = 1.0 - smoothstep(
            mat.fade_range.x, mat.fade_range.y,
            max(fwidth(ll_coords).x, fwidth(ll_coords).y));
        // Fade the graticule out as `transition` approaches 0 (fully-textured
        // imagery): a natural globe seen from orbit shows no lat/long lines;
        // they belong to the blueprint look that fades in on approach.
        grid_mask = (1.0 - smoothstep(0.0, mat.line_width, ll_line)) * ll_fade
            * smoothstep(0.05, 0.45, mat.transition);
#endif
    } else {
        // --- Blueprint grid (Cartesian XZ, flat ground).
#ifdef LUNCO_GLOBE_DIRECTION
        base *= textureSampleGrad(albedo_tex, albedo_smp, globe_uv, globe_dx, globe_dy).rgb;
#else
#ifdef VERTEX_UVS_A
        base *= textureSample(albedo_tex, albedo_smp, in.uv).rgb;
#endif
#endif
        let pos = in.world_position.xz;
        let world_per_px = abs(fwidth(pos));

        let major_dist = vec2<f32>(
            abs(fract(pos.x / mat.major_grid_spacing - 0.5) - 0.5) * mat.major_grid_spacing,
            abs(fract(pos.y / mat.major_grid_spacing - 0.5) - 0.5) * mat.major_grid_spacing,
        );
        let major_px = min(
            major_dist.x / max(world_per_px.x, 1e-6),
            major_dist.y / max(world_per_px.y, 1e-6));
        let major_m = 1.0 - smoothstep(0.0, mat.major_line_width, major_px);

        let minor_dist = vec2<f32>(
            abs(fract(pos.x / mat.minor_grid_spacing - 0.5) - 0.5) * mat.minor_grid_spacing,
            abs(fract(pos.y / mat.minor_grid_spacing - 0.5) - 0.5) * mat.minor_grid_spacing,
        );
        let minor_px = min(
            minor_dist.x / max(world_per_px.x, 1e-6),
            minor_dist.y / max(world_per_px.y, 1e-6));
        let minor_raw = 1.0 - smoothstep(0.0, mat.minor_line_width, minor_px);
        let minor_m = minor_raw * mat.minor_line_fade * (1.0 - major_m);

        grid_mask = max(major_m, minor_m);
    }

    let line_color = mix(mat.high_line_color, mat.low_line_color, mat.transition);
    let albedo = mix(base, line_color, grid_mask);
    return lit(in, is_front, albedo, mat.roughness, 0.0, vec3(0.0));
}

#ifdef LUNCO_GLOBE_DIRECTION
@fragment
fn fragment(in: GlobeVertexOutput, @builtin(front_facing) is_front: bool) -> @location(0) vec4<f32> {
    return shade(globe_pbr_input(in), is_front, in.globe_direction);
}
#else
@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> @location(0) vec4<f32> {
    return shade(in, is_front, vec3<f32>(0.0));
}
#endif
