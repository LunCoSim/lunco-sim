//! Blueprint grid material — self-describing `ShaderMaterial` (replaces the old
//! hand-rolled `ExtendedMaterial` in `blueprint.rs`).
//!
//! Two grid modes blended by `transition` (0 → 1):
//!   * `transition < 0.5` — **spherical lat/long grid** over a body's UV
//!     (`uv.x`=lon, `uv.y`=lat). Used by the celestial Earth/Moon tiles seen
//!     from orbit. Needs `VERTEX_UVS_A`; with no UVs the mode contributes nothing.
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

#import bevy_pbr::forward_io::VertexOutput
#import lunco::pbr_lit::lit

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

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> @location(0) vec4<f32> {
    var base = mat.surface_color;
    var grid_mask = 0.0;

    if (mat.transition < 0.5) {
        // --- Lat/Long grid (spherical bodies) — needs UVs.
#ifdef VERTEX_UVS_A
        base *= textureSample(albedo_tex, albedo_smp, in.uv).rgb;
        let ll_coords = in.uv * mat.subdivisions;
        let ll_f = abs(fract(ll_coords - 0.5) - 0.5) / fwidth(ll_coords);
        let ll_line = min(ll_f.x, ll_f.y);
        let ll_fade = 1.0 - smoothstep(
            mat.fade_range.x, mat.fade_range.y,
            max(fwidth(ll_coords).x, fwidth(ll_coords).y));
        grid_mask = (1.0 - smoothstep(0.0, mat.line_width, ll_line)) * ll_fade;
#endif
    } else {
        // --- Blueprint grid (Cartesian XZ, flat ground).
#ifdef VERTEX_UVS_A
        base *= textureSample(albedo_tex, albedo_smp, in.uv).rgb;
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
