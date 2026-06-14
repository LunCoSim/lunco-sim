// Celestial grid overlay — a lat/long grid drawn OVER a textured, PBR-lit
// planet tile (Earth / Moon).
//
// This is an `ExtendedMaterial<StandardMaterial, CelestialGridExtension>`: the
// base `StandardMaterial` keeps the planet's albedo texture + full PBR, and this
// fragment overlays grid lines on the mesh UVs. The "spherical" lat/long mapping
// is NOT shader math — it comes from the quadsphere tile's UVs (a terrain/system
// concern); the shader just draws a grid on those UVs.
//
// `grid_fade` is written each frame by `lunco_celestial::celestial_visuals_system`
// from the camera's altitude over the nearest body: 1 = full grid (far out),
// 0 = hidden (near the surface, so the texture shows through).

#import bevy_pbr::forward_io::{VertexOutput, FragmentOutput}
#import bevy_pbr::pbr_fragment::pbr_input_from_standard_material
#import bevy_pbr::pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing}

struct CelestialGridExtension {
    line_color: vec4<f32>,
    subdivisions: vec2<f32>,
    fade_range: vec2<f32>,
    line_width: f32,
    grid_fade: f32,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100)
var<uniform> grid: CelestialGridExtension;

@fragment
fn fragment(input: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(input, is_front);

    // Lat/long grid on mesh UVs (the quadsphere tile provides the lat/long map).
    let ll = vec2<f32>(input.uv.x * grid.subdivisions.x, input.uv.y * grid.subdivisions.y);
    let ll_d = abs(fract(ll - 0.5) - 0.5) / fwidth(ll);
    let line = min(ll_d.x, ll_d.y);
    // Fade the grid where the cells get sub-pixel (avoids shimmer at distance).
    let aa = 1.0 - smoothstep(grid.fade_range.x, grid.fade_range.y, max(fwidth(ll).x, fwidth(ll).y));
    let mask = (1.0 - smoothstep(0.0, grid.line_width, line)) * aa * grid.grid_fade;

    pbr_input.material.base_color = mix(pbr_input.material.base_color, grid.line_color, mask);

    var out: FragmentOutput;
    out.color = main_pass_post_lighting_processing(pbr_input, apply_pbr_lighting(pbr_input));
    return out;
}
