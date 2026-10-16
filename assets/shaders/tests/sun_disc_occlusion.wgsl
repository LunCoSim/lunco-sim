#import bevy_render::view::View
#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput
#import bevy_pbr::utils::coords_to_viewport_uv
@group(0) @binding(0) var<uniform> view: View;
//!@engine sun_dir_view
//!@engine sun_tan_radius
struct Material { sun_dir_view: vec3<f32>, sun_tan_radius: f32, }
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> mat: Material;

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    // USD authors a two-degree source aimed at the camera's forward axis.
    let valid = abs(mat.sun_tan_radius - 0.017455065) < 0.00001
        && distance(mat.sun_dir_view, vec3(0.0, 0.0, -1.0)) < 0.001;
    if (!valid) { return vec4(1.0, 0.0, 0.0, 1.0); }
    let uv = coords_to_viewport_uv(in.position.xy, view.viewport);
    let p = view.view_from_clip * vec4(uv * vec2(2.0, -2.0) + vec2(-1.0, 1.0), 1.0, 1.0);
    let ray = normalize(p.xyz / p.w);
    let angle = atan2(length(cross(ray, mat.sun_dir_view)), dot(ray, mat.sun_dir_view));
    let disc = angle < atan(mat.sun_tan_radius);
    return select(vec4(0.0, 1.0, 0.0, 1.0), vec4(1.0, 1.0, 0.0, 1.0), disc);
}
