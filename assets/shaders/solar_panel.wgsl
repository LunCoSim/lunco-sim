//! Solar panel material for the general `ShaderMaterial`.
//!
//! Renders a photovoltaic cell grid procedurally on a panel's UV [0,1] surface:
//! a grid of silicon cells separated by dark gaps, metallic bus lines along the
//! cell boundaries, and a dark frame border around the array.
//!
//! Works purely in UV space (no panel dimensions needed). Unlit base + a mild
//! normal-based shade, matching the other `ShaderMaterial` shaders.
//!
//! ## Params
//!   param0 = cell_rows   (cells along U, default 12)
//!   param1 = cell_cols   (cells along V, default 6)
//!   param2 = cell_gap    (UV fraction, default 0.02)
//!   param3 = bus_width   (UV fraction, default 0.004)
//!   param4 = border      (frame border, UV fraction, default 0.04)
//!   color_a = cell   color_b = bus line (metal)   color_c = frame border
//!
//! Edit live (hot-reload) or tweak via `SetObjectProperty { property:"param0".. }`.

#import bevy_pbr::forward_io::VertexOutput

struct ShaderParams {
    color_a: vec4<f32>,
    color_b: vec4<f32>,
    color_c: vec4<f32>,
    params:  vec4<f32>,
    params2: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0)
var<uniform> mat: ShaderParams;

/// True if `p` is within `half_w` of the nearest grid line at `spacing`.
fn on_line(p: f32, spacing: f32, half_w: f32) -> bool {
    let d = abs(fract(p / spacing - 0.5) - 0.5) * spacing;
    return d < half_w;
}

@fragment
fn fragment(input: VertexOutput) -> @location(0) vec4<f32> {
    let uv = input.uv;
    let rows   = max(select(mat.params.x, 12.0, mat.params.x < 0.5), 1.0);
    let cols   = max(select(mat.params.y, 6.0,  mat.params.y < 0.5), 1.0);
    let gap    = select(mat.params.z, 0.02,  mat.params.z < 0.0001);
    let bus    = select(mat.params.w, 0.004, mat.params.w < 0.0001);
    let border = select(mat.params2.x, 0.04, mat.params2.x < 0.0001);

    let sx = 1.0 / rows;
    let sy = 1.0 / cols;

    var color = mat.color_a;  // silicon cell

    if (on_line(uv.x, sx, gap * 0.5) || on_line(uv.y, sy, gap * 0.5)) {
        color = vec4<f32>(0.02, 0.02, 0.02, 1.0);                 // dark cell gap
    } else if (on_line(uv.x, sx, bus * 0.5) || on_line(uv.y, sy, bus * 0.5)) {
        color = mix(mat.color_a, mat.color_b, 0.9);              // metallic bus line
    }

    if (uv.x < border || uv.x > 1.0 - border || uv.y < border || uv.y > 1.0 - border) {
        color = mat.color_c;                                     // frame border
    }

    let n = normalize(input.world_normal);
    let light_dir = normalize(vec3<f32>(0.4, 1.0, 0.6));
    let shade = 0.55 + 0.45 * clamp(dot(n, light_dir), 0.0, 1.0);
    return vec4<f32>(color.rgb * shade, color.a);
}
