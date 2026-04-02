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
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100)
var<uniform> extension: BlueprintExtension;

const PI: f32 = 3.14159265359;

@fragment
fn fragment(
    input: VertexOutput,
    @builtin(front_facing) is_front: bool,
) -> FragmentOutput {
    // 1. Get the standard PBR input from the base material
    var pbr_input = pbr_input_from_standard_material(input, is_front);
    
    // --- Colors Transition ---
    // We no longer multiply by base_color_mix as planetary maps provide the real colors now.
    let final_base_color = pbr_input.material.base_color;

    // --- Lat/Long Grid (High Alt) ---
    // Use mesh UVs directly - they rotate with the planet and are seam-corrected
    let lon = input.uv.x;
    let lat = input.uv.y;

    // Parameterized subdivisions
    let lon_scaled = lon * extension.subdivisions.x;
    let lat_scaled = lat * extension.subdivisions.y;
    let lat_long_coords = vec2<f32>(lon_scaled, lat_scaled);
    
    let fw = fwidth(lat_long_coords);
    
    let lat_long_f = abs(fract(lat_long_coords - 0.5) - 0.5) / fw;
    let lat_long_line = min(lat_long_f.x, lat_long_f.y);
    
    // Thinner coordinate lines when high, fade out using parameterized range
    let lat_long_width = mix(0.5, extension.line_width, extension.transition);
    let lat_long_fade = 1.0 - smoothstep(extension.fade_range.x, extension.fade_range.y, max(fw.x, fw.y));
    let lat_long_mask = (1.0 - smoothstep(0.0, lat_long_width, lat_long_line)) * lat_long_fade;

    // --- Blueprint Grid (Low Alt) ---
    let blueprint_coords = input.world_position.xz / extension.grid_scale;
    let blueprint_f = abs(fract(blueprint_coords - 0.5) - 0.5) / fwidth(blueprint_coords);
    let blueprint_line = min(blueprint_f.x, blueprint_f.y);
    let blueprint_mask = 1.0 - smoothstep(0.0, extension.line_width, blueprint_line);
    
    // --- Mixing ---
    let grid_mask = mix(lat_long_mask, blueprint_mask, extension.transition) * (1.0 - smoothstep(0.9, 1.0, extension.transition));
    let line_color = mix(extension.high_line_color, extension.low_line_color, extension.transition);
    
    pbr_input.material.base_color = mix(final_base_color, line_color, grid_mask);
    
    // 4. Apply standard lighting with our modified color
    var out: FragmentOutput;
    out.color = main_pass_post_lighting_processing(pbr_input, apply_pbr_lighting(pbr_input));
    return out;
}
