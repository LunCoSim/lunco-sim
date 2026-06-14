//! wet — emissive glow PBR material (generated template). A lit base plus
//! a constant emissive term (×strength). Edit freely.

#import bevy_pbr::forward_io::VertexOutput
#import lunco::pbr_lit::lit

//!@ui      base_color color "Base colour"
//!@default base_color 0.1,0.1,0.12
//!@ui      emissive   color "Glow colour"
//!@default emissive   1.0,0.6,0.1
//!@ui      strength   0 8  "Glow strength"
//!@default strength   2.0
//!@ui      roughness  0 1  "Roughness"
//!@default roughness  0.5
struct Material {
    base_color: vec3<f32>,
    strength:   f32,
    emissive:   vec3<f32>,
    roughness:  f32,
}
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> mat: Material;

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> @location(0) vec4<f32> {
    return lit(in, is_front, mat.base_color, mat.roughness, 0.0, mat.emissive * mat.strength);
}
