
//!@engine sun_dir_view
//!@engine sun_tan_radius
struct Material { sun_dir_view: vec3<f32>, sun_tan_radius: f32, }
@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> mat: Material;

@fragment
fn fragment() -> @location(0) vec4<f32> {
    // Zero is the point-source boundary; any positive angle must reach the GPU.
    let valid = mat.sun_tan_radius == 0.0 && length(mat.sun_dir_view) > 0.5;
    return select(vec4(1.0, 0.0, 0.0, 1.0), vec4(0.0, 1.0, 0.0, 1.0), valid);
}
