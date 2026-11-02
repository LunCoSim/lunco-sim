import bevy_pbr::render::forward_io::VertexOutput;
import bevy_pbr::render::mesh_functions;
import super::pbr_lit::lit;

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

fn thermal_rgb(temperature: f32) -> vec3<f32> {
    // CIE colour-matching samples, 400..700 nm in 20 nm steps. Endpoint
    // trapezoid weights. RGB can exceed one: tonemapping owns display exposure.
    let cmf=array<vec3<f32>,16>(
        vec3(0.01431,0.000396,0.067850),vec3(0.13438,0.004,0.64560),
        vec3(0.34828,0.023,1.74706),vec3(0.29080,0.060,1.66920),
        vec3(0.09564,0.13902,0.81295),vec3(0.00490,0.323,0.272),
        vec3(0.06327,0.710,0.07825),vec3(0.29040,0.954,0.02030),
        vec3(0.59450,0.995,0.00390),vec3(0.91630,0.870,0.00165),
        vec3(1.06220,0.631,0.00080),vec3(0.85445,0.381,0.00019),
        vec3(0.44790,0.175,0.00002),vec3(0.16490,0.061,0.0),
        vec3(0.04677,0.017,0.0),vec3(0.011359,0.004102,0.0));
    var xyz=vec3<f32>(0.0);
    for(var i=0u;i<16u;i+=1u) {
        let wavelength=(400.0+20.0*f32(i))*1.0e-9;
        let exponent=0.01438776877/(wavelength*max(1.0,temperature));
        // exp overflows for cold metal, whose visible radiance is negligible.
        if exponent<80.0 {
            let spectral=1.191042972e-16/(pow(wavelength,5.0)*(exp(exponent)-1.0));
            let weight=select(1.0,0.5,i==0u || i==15u);
            xyz+=cmf[i]*spectral*(683.0*20.0e-9*weight);
        }
    }
    return max(vec3<f32>(
        dot(vec3(3.2406,-1.5372,-0.4986),xyz),
        dot(vec3(-0.9689,1.8758,0.0415),xyz),
        dot(vec3(0.0557,-0.2040,1.0570),xyz)),vec3<f32>(0.0));
}
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
    let emission=thermal_rgb(temperature)*clamp(mat.emissivity,0.0,1.0);
    return lit(input,front,mat.base_color,mat.roughness,mat.metallic,emission);
}
