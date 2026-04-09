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
        // --- Blueprint Grid (Cartesian, body-local) ---
        // world_normal is the body-local unit vector from center to surface,
        // rotated into world space by Body rotation. Multiplying by body_radius
        // gives the body-local position in world-space coords (~1.7M m range).
        let body_local = input.world_normal * extension.body_radius;
        let world_per_px = abs(fwidth(body_local));

        // 3D grid: distance to nearest grid plane along each axis.
        // This gives consistent lines on ALL cube-to-sphere faces (no degenerate
        // projections at face centers where 2 of 3 coords are near zero).
        let gx = abs(fract(body_local.x / extension.major_grid_spacing - 0.5) - 0.5) * extension.major_grid_spacing;
        let gy = abs(fract(body_local.y / extension.major_grid_spacing - 0.5) - 0.5) * extension.major_grid_spacing;
        let gz = abs(fract(body_local.z / extension.major_grid_spacing - 0.5) - 0.5) * extension.major_grid_spacing;
        let major_px_x = gx / max(world_per_px.x, 1e-6);
        let major_px_y = gy / max(world_per_px.y, 1e-6);
        let major_px_z = gz / max(world_per_px.z, 1e-6);
        let major_px = min(major_px_x, min(major_px_y, major_px_z));
        let major_m = 1.0 - smoothstep(0.0, extension.major_line_width, major_px);

        // Minor grid (same pattern, finer spacing)
        let gx2 = abs(fract(body_local.x / extension.minor_grid_spacing - 0.5) - 0.5) * extension.minor_grid_spacing;
        let gy2 = abs(fract(body_local.y / extension.minor_grid_spacing - 0.5) - 0.5) * extension.minor_grid_spacing;
        let gz2 = abs(fract(body_local.z / extension.minor_grid_spacing - 0.5) - 0.5) * extension.minor_grid_spacing;
        let minor_px_x = gx2 / max(world_per_px.x, 1e-6);
        let minor_px_y = gy2 / max(world_per_px.y, 1e-6);
        let minor_px_z = gz2 / max(world_per_px.z, 1e-6);
        let minor_px = min(minor_px_x, min(minor_px_y, minor_px_z));
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
