//! Gold multilayer-insulation foil for a complete `ShaderMaterial`.
//!
//! The material is intentionally a whole fragment shader, not a PBR helper
//! library: USD binds this file directly through `info:wgsl:sourceAsset`.
//! Crinkle is object-space so the facets stay attached to the foil while the
//! lander moves, and `v_scale` keeps the pattern readable on differently sized
//! blankets.

#import bevy_pbr::forward_io::VertexOutput
#import bevy_pbr::mesh_functions
#import lunco::noise::fbm
#import lunco::pbr_lit::lit

//!@ui      foil_color  color "Bright foil colour"
//!@default foil_color  0.83,0.62,0.18
//!@ui      shade_color color "Foil shadow colour"
//!@default shade_color 0.45,0.30,0.08
//!@ui      crinkle     0 64 "Crinkle cell density"
//!@default crinkle     26.0
//!@ui      facet_depth 0 1 "Facet contrast"
//!@default facet_depth 0.55
//!@ui      sheen       0 1 "Reflective sheen"
//!@default sheen       0.85
//!@ui      v_scale     0.01 4 "Vertical pattern scale"
//!@default v_scale     0.19
struct Material {
    foil_color:  vec3<f32>,
    crinkle:     f32,
    shade_color: vec3<f32>,
    facet_depth: f32,
    sheen:       f32,
    v_scale:     f32,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0)
var<uniform> mat: Material;

@fragment
fn fragment(input: VertexOutput, @builtin(front_facing) is_front: bool) -> @location(0) vec4<f32> {
    // Recover object-space coordinates from the mesh transform. The scale is
    // removed from the basis but retained in the position, so the authored
    // crinkle density remains in metres on every referenced foil mesh.
    let m = mesh_functions::get_world_from_local(input.instance_index);
    let R = mat3x3<f32>(normalize(m[0].xyz), normalize(m[1].xyz), normalize(m[2].xyz));
    let p_local = transpose(R) * (input.world_position.xyz - m[3].xyz);
    let n_local = normalize(transpose(R) * normalize(input.world_normal));

    let grain_pos = p_local * vec3<f32>(mat.crinkle, mat.crinkle * mat.v_scale, mat.crinkle);
    let grain = fbm(grain_pos, 3, 0.55);
    let facet = clamp(0.5 + (grain - 0.5) * mat.facet_depth, 0.0, 1.0);

    // A grazing facet catches less light than a face turned toward the scene;
    // the foil colour remains authored while the shade colour supplies the
    // folded valleys.
    let lightness = 0.55 + 0.45 * abs(n_local.y);
    let fold = clamp(facet * lightness, 0.0, 1.0);
    let albedo = mix(mat.shade_color, mat.foil_color, fold);
    let roughness = clamp(0.32 - mat.sheen * 0.20 + (1.0 - facet) * 0.18, 0.08, 0.75);
    let metallic = clamp(0.55 + mat.sheen * 0.4, 0.0, 1.0);

    return lit(input, is_front, albedo, roughness, metallic, vec3<f32>(0.0));
}
