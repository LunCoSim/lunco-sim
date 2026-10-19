// Constant unlit opaque geometry for image-based depth/volume acceptance.
#import bevy_pbr::forward_io::VertexOutput
struct Material { color: vec4<f32>, }
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> mat: Material;
@fragment
fn fragment(input: VertexOutput) -> @location(0) vec4<f32> {
    return mat.color;
}
