//! Solar panel material for the general `ShaderMaterial`.
//!
//! Renders a photovoltaic cell grid procedurally on a panel's UV [0,1] surface:
//! a grid of silicon cells separated by dark gaps, metallic bus lines along the
//! cell boundaries, and a dark frame border around the array.
//!
//! Works purely in UV space (no panel dimensions needed). The procedural grid
//! is the albedo for full scene PBR lighting (real sun, shadow maps), so
//! panels respond to the actual light environment.
//!
//! Dynamic, self-describing parameters: the engine reflects the `Material`
//! struct (field names → offsets) and the `//!@` annotations straight out of
//! this file. Edit live (hot-reload) or via the Inspector / `SetObjectProperty`.

#import bevy_pbr::{
    forward_io::VertexOutput,
    pbr_types,
    pbr_functions,
    mesh_bindings::mesh,
    mesh_view_bindings::view,
}

//!@ui      cell_color  color "Cell colour"
//!@default cell_color  0.05,0.05,0.35
//!@ui      bus_color   color "Bus line colour"
//!@default bus_color   0.85,0.85,0.90
//!@ui      frame_color color "Frame border colour"
//!@default frame_color 0.35,0.35,0.38
//!@ui      cell_rows   1 32  "Cell rows (along U)"
//!@default cell_rows   12
//!@ui      cell_cols   1 32  "Cell cols (along V)"
//!@default cell_cols   6
//!@ui      cell_gap    0 0.1 "Cell gap (UV)"
//!@default cell_gap    0.02
//!@ui      bus_width   0 0.02 "Bus width (UV)"
//!@default bus_width   0.004
//!@ui      border      0 0.2 "Frame border (UV)"
//!@default border      0.04
//!@ui      seamless_u  0 1 "Seamless U"
//!@default seamless_u  0
//!@ui      v_scale     0.1 10 "V scale / aspect ratio"
//!@default v_scale     1.0
struct Material {
    cell_color:  vec3<f32>,
    cell_rows:   f32,
    bus_color:   vec3<f32>,
    cell_cols:   f32,
    frame_color: vec3<f32>,
    cell_gap:    f32,
    bus_width:   f32,
    border:      f32,
    seamless_u:  f32,
    v_scale:     f32,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0)
var<uniform> mat: Material;

/// True if `p` is within `half_w` of the nearest grid line at `spacing`.
fn on_line(p: f32, spacing: f32, half_w: f32) -> bool {
    let d = abs(fract(p / spacing - 0.5) - 0.5) * spacing;
    return d < half_w;
}

@fragment
fn fragment(input: VertexOutput, @builtin(front_facing) is_front: bool) -> @location(0) vec4<f32> {
    let uv = input.uv;
    let rows   = max(mat.cell_rows, 1.0);
    let cols   = max(mat.cell_cols, 1.0);
    let gap    = mat.cell_gap;
    let bus    = mat.bus_width;
    let border = mat.border;

    let sx = 1.0 / rows;
    let sy = 1.0 / cols;

    var color = mat.cell_color;  // silicon cell

    let gap_x = gap * 0.5;
    let gap_y = gap * 0.5 * mat.v_scale;
    let bus_x = bus * 0.5;
    let bus_y = bus * 0.5 * mat.v_scale;

    if (on_line(uv.x, sx, bus_x) || on_line(uv.y, sy, bus_y)) {
        color = mix(mat.cell_color, mat.bus_color, 0.9);         // metallic bus line
    } else if (on_line(uv.x, sx, gap_x) || on_line(uv.y, sy, gap_y)) {
        color = vec3<f32>(0.02, 0.02, 0.02);                     // dark cell gap
    }

    let border_x = (mat.seamless_u < 0.5) && (uv.x < border || uv.x > 1.0 - border);
    let border_y = uv.y < border || uv.y > 1.0 - border;
    if (border_x || border_y) {
        color = mat.frame_color;                                 // frame border
    }

    // Full scene lighting (real sun direction, shadow maps, ambient) over
    // the procedural cell grid — panels go dark on the night side and when
    // the horizon system pulls the entity out of the sun's render layer.
    var pbr_input = pbr_types::pbr_input_new();
    pbr_input.flags = mesh[input.instance_index].flags; // keep SHADOW_RECEIVER
    pbr_input.frag_coord = input.position;
    pbr_input.world_position = input.world_position;
    pbr_input.world_normal = pbr_functions::prepare_world_normal(
        normalize(input.world_normal), false, is_front);
    pbr_input.is_orthographic = view.clip_from_view[3].w == 1.0;
    pbr_input.N = pbr_input.world_normal;
    pbr_input.V = pbr_functions::calculate_view(input.world_position, pbr_input.is_orthographic);
    pbr_input.material.base_color = vec4(color, 1.0);
    // Glassy cell surface: low roughness so panels catch a sun glint.
    pbr_input.material.perceptual_roughness = 0.3;
    pbr_input.material.metallic = 0.0;
    pbr_input.material.reflectance = vec3(0.5);

    var out = pbr_functions::apply_pbr_lighting(pbr_input);
    out = pbr_functions::main_pass_post_lighting_processing(pbr_input, out);
    return out;
}
