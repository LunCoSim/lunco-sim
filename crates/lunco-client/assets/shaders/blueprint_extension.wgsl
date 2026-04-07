#import bevy_pbr::forward_io::VertexOutput
#import bevy_pbr::pbr_fragment::pbr_input_from_standard_material
#import bevy_pbr::pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing}
#import bevy_pbr::forward_io::FragmentOutput

struct BlueprintExtension {
    high_color: vec4<f32>,
    low_color: vec4<f32>,
    high_line_color: vec4<f32>,
    low_line_color: vec4<f32>,
    subdivisions: vec2<f32>,
    fade_range: vec2<f32>,
    grid_scale: f32,
    line_width: f32,
    transition: f32,
    body_radius: f32,
    major_grid_spacing: f32,
    minor_grid_spacing: f32,
    major_line_width: f32,
    minor_line_width: f32,
    minor_line_fade: f32,
    surface_color: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100)
var<uniform> extension: BlueprintExtension;

@fragment
fn fragment(
    input: VertexOutput,
    @builtin(front_facing) is_front: bool,
) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(input, is_front);
    // Use the PBR-sampled base color (includes base_color_texture from StandardMaterial
    // if present). The blueprint grid lines are layered on top as an overlay.
    // When no texture is set, pbr_input.material.base_color falls back to the
    // StandardMaterial's base_color field.
    var final_base_color = pbr_input.material.base_color;

    var grid_mask = 0.0;

    if extension.transition < 0.5 {
        // --- Lat/Long Grid (spherical bodies) ---
        let lon = input.uv.x;
        let lat = input.uv.y;
        let lon_scaled = lon * extension.subdivisions.x;
        let lat_scaled = lat * extension.subdivisions.y;
        let ll_coords = vec2<f32>(lon_scaled, lat_scaled);
        let ll_f = abs(fract(ll_coords - 0.5) - 0.5) / fwidth(ll_coords);
        let ll_line = min(ll_f.x, ll_f.y);
        let ll_fade = 1.0 - smoothstep(extension.fade_range.x, extension.fade_range.y, max(fwidth(ll_coords).x, fwidth(ll_coords).y));
        grid_mask = (1.0 - smoothstep(0.0, extension.line_width, ll_line)) * ll_fade;
    } else {
        // --- Blueprint Grid (Cartesian XZ, flat ground) ---
        // fwidth on raw world position (smooth everywhere, no wrap discontinuities).
        let pos = input.world_position.xz;
        let world_per_px = abs(fwidth(pos));  // world units per pixel

        // Major grid
        let major_dist = vec2<f32>(
            abs(fract(pos.x / extension.major_grid_spacing - 0.5) - 0.5) * extension.major_grid_spacing,
            abs(fract(pos.y / extension.major_grid_spacing - 0.5) - 0.5) * extension.major_grid_spacing,
        );
        let major_px = min(major_dist.x / max(world_per_px.x, 1e-6), major_dist.y / max(world_per_px.y, 1e-6));
        let major_m = 1.0 - smoothstep(0.0, extension.major_line_width, major_px);

        // Minor grid
        let minor_dist = vec2<f32>(
            abs(fract(pos.x / extension.minor_grid_spacing - 0.5) - 0.5) * extension.minor_grid_spacing,
            abs(fract(pos.y / extension.minor_grid_spacing - 0.5) - 0.5) * extension.minor_grid_spacing,
        );
        let minor_px = min(minor_dist.x / max(world_per_px.x, 1e-6), minor_dist.y / max(world_per_px.y, 1e-6));
        let minor_raw = 1.0 - smoothstep(0.0, extension.minor_line_width, minor_px);
        let minor_m = minor_raw * extension.minor_line_fade * (1.0 - major_m);

        grid_mask = max(major_m, minor_m);
    }

    let line_color = mix(extension.high_line_color, extension.low_line_color, extension.transition);
    pbr_input.material.base_color = mix(final_base_color, line_color, grid_mask);

    var out: FragmentOutput;
    out.color = main_pass_post_lighting_processing(pbr_input, apply_pbr_lighting(pbr_input));
    return out;
}
