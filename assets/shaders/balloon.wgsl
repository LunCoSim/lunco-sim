//! Balloon / ball material for the general `ShaderMaterial`.
//!
//! Draws a checkerboard on a sphere so its rotation is obvious: alternating
//! lat-long cells + a marker wedge + coloured poles (so the spin axis stays
//! visible head-on).
//!
//! UV is mesh-fixed, so the pattern rotates with the geometry: `uv.x` wraps the
//! circumference (longitude), `uv.y` runs pole-to-pole (latitude).
//!
//! ## Params
//!   param0 = wedge_count   (longitude cells, default 8)
//!   param1 = band_count    (latitude cells, default 6)
//!   param3 = marker_wedges (default 1)
//!   color_a / color_b = alternating cells   color_c = marker wedge + poles
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

fn parity(i: f32) -> bool {
    return (i - 2.0 * floor(i * 0.5)) > 0.5;
}

@fragment
fn fragment(input: VertexOutput) -> @location(0) vec4<f32> {
    let uv = input.uv;
    let wedge_count = select(mat.params.x, 8.0, mat.params.x < 0.5);
    let band_count  = select(mat.params.y, 6.0, mat.params.y < 0.5);
    let marker      = max(mat.params.w, 1.0);

    let wi = floor(uv.x * wedge_count);
    let bi = floor(uv.y * band_count);
    let checker = parity(wi) != parity(bi);            // XOR → checkerboard
    var color = select(mat.color_a, mat.color_b, checker);

    if (wi < marker) { color = mat.color_c; }          // marker wedge
    if (uv.y < 0.04 || uv.y > 0.96) { color = mat.color_c; } // pole caps

    // Mild normal-based shading so the form reads, without full PBR.
    let n = normalize(input.world_normal);
    let light_dir = normalize(vec3<f32>(0.4, 1.0, 0.6));
    let shade = 0.55 + 0.45 * clamp(dot(n, light_dir), 0.0, 1.0);
    return vec4<f32>(color.rgb * shade, color.a);
}
