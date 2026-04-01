#import bevy_pbr::forward_io::VertexOutput
#import bevy_pbr::pbr_fragment::pbr_input_from_standard_material
#import bevy_pbr::pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing}
#import bevy_pbr::forward_io::FragmentOutput

struct BlueprintExtension {
    line_color: vec4<f32>,
    grid_scale: f32,
    line_width: f32,
    transition: f32, // 0.0 = Lat/Long (High), 1.0 = Blueprint (Low)
    moon_radius: f32,
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
    let color_grey = vec4<f32>(0.5, 0.5, 0.5, 1.0);
    let color_blue = vec4<f32>(0.0, 0.05, 0.15, 1.0);
    let base_color = mix(color_grey, color_blue, extension.transition);
    pbr_input.material.base_color = base_color;

    // --- Lat/Long Grid (High Alt) ---
    let n = normalize(input.world_normal);
    let lon = (atan2(n.z, n.x) + PI) / (2.0 * PI);
    let lat = (asin(n.y) + (PI / 2.0)) / PI;
    
    let lat_long_coords = vec2<f32>(lon * 24.0, lat * 12.0);
    let lat_long_f = abs(fract(lat_long_coords - 0.5) - 0.5) / fwidth(lat_long_coords);
    let lat_long_line = min(lat_long_f.x, lat_long_f.y);
    let lat_long_mask = 1.0 - smoothstep(0.0, extension.line_width, lat_long_line);

    // --- Blueprint Grid (Low Alt) ---
    // Standard XZ projection relative to moon center... 
    // actually input.world_position is okay for local look.
    let blueprint_coords = input.world_position.xz / extension.grid_scale;
    let blueprint_f = abs(fract(blueprint_coords - 0.5) - 0.5) / fwidth(blueprint_coords);
    let blueprint_line = min(blueprint_f.x, blueprint_f.y);
    let blueprint_mask = 1.0 - smoothstep(0.0, extension.line_width, blueprint_line);
    
    // --- Mixing ---
    let grid_mask = mix(lat_long_mask, blueprint_mask, extension.transition);
    let line_color = mix(vec4<f32>(1.0, 1.0, 1.0, 1.0), extension.line_color, extension.transition);
    
    pbr_input.material.base_color = mix(base_color, line_color, grid_mask);
    
    // 4. Apply standard lighting with our modified color
    var out: FragmentOutput;
    out.color = main_pass_post_lighting_processing(pbr_input, apply_pbr_lighting(pbr_input));
    return out;
}
