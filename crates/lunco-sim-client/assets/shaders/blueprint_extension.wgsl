#import bevy_pbr::forward_io::VertexOutput
#import bevy_pbr::pbr_fragment::pbr_input_from_standard_material
#import bevy_pbr::pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing}
#import bevy_pbr::forward_io::FragmentOutput

struct BlueprintExtension {
    line_color: vec4<f32>,
    grid_scale: f32,
    line_width: f32,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100)
var<uniform> extension: BlueprintExtension;

@fragment
fn fragment(
    input: VertexOutput,
    @builtin(front_facing) is_front: bool,
) -> FragmentOutput {
    // 1. Get the standard PBR input from the base material
    var pbr_input = pbr_input_from_standard_material(input, is_front);
    
    // 2. Blueprint Grid Logic using world position (xz plane)
    let grid_coord = input.world_position.xz / extension.grid_scale;
    let grid_f = abs(fract(grid_coord - 0.5) - 0.5) / fwidth(grid_coord);
    let line = min(grid_f.x, grid_f.y);
    let grid_mask = 1.0 - smoothstep(0.0, extension.line_width, line);
    
    // 3. Overlay the grid on the base color
    pbr_input.material.base_color = mix(pbr_input.material.base_color, extension.line_color, grid_mask);
    
    // 4. Apply standard lighting with our modified color
    var out: FragmentOutput;
    out.color = main_pass_post_lighting_processing(pbr_input, apply_pbr_lighting(pbr_input));
    return out;
}
