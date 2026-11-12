//!@capability lunco.shadow-receiver.surface.v1
import bevy_pbr::render::forward_io::VertexOutput;
import bevy_pbr::render::mesh_functions;
import super::pbr_lit::lit;
import super::blackbody_radiance::blackbody_radiance;

//! Opaque PBR metal with temperature-driven thermal emission. Temperature is
//! a Modelica result, never throttle. Coarse CIE 1931 spectral quadrature in
//! absolute luminance units avoids an arbitrary orange tint or glow gain.
//! CIE 2019, 1931 2-degree observer: DOI 10.25039/CIE.DS.xvudnb9b.
//!@default base_color 0.16,0.17,0.18
//!@default temperature_throat_k 293.15
//!@default temperature_middle_k 293.15
//!@default temperature_exit_k 293.15
//!@default nozzle_length_m 1.0
//!@default emissivity 0.8
//!@default roughness 0.48
//!@default metallic 0.85
struct Material {
    base_color: vec3<f32>,
    temperature_throat_k: f32,
    temperature_middle_k: f32,
    temperature_exit_k: f32,
    nozzle_length_m: f32,
    emissivity: f32,
    roughness: f32,
    metallic: f32,
}
@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> mat: Material;

@fragment
fn fragment(input: VertexOutput,@builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let inverse=mesh_functions::get_local_from_world(input.instance_index);
    let local=(inverse*input.world_position).xyz;
    let s=clamp(0.5-local.y/max(0.000001,mat.nozzle_length_m),0.0,1.0);
    var temperature=mat.temperature_throat_k;
    if s>0.425 {
        temperature=mix(mat.temperature_middle_k,mat.temperature_exit_k,clamp((s-0.425)/0.375,0.0,1.0));
    } else {
        temperature=mix(mat.temperature_throat_k,mat.temperature_middle_k,clamp((s-0.125)/0.3,0.0,1.0));
    }
    let emission=blackbody_radiance(temperature)*clamp(mat.emissivity,0.0,1.0);
    return lit(input,front,mat.base_color,mat.roughness,mat.metallic,emission);
}
