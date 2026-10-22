import bevy_pbr::render::forward_io::VertexOutput;

// Constant unlit opaque geometry for image-based depth/volume acceptance.
struct Material { color: vec4<f32>, }
@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> mat: Material;
@fragment
fn fragment(input: VertexOutput) -> @location(0) vec4<f32> {
    return mat.color;
}
