//! Default terrain material with ray-marched heightfield sun shadows.
//!
//! Applied automatically by `lunco-environment`'s horizon system to a
//! `HorizonShadowTerrain` that authors no custom shader: plain albedo
//! (`color_a`, taken from the prim's `displayColor`) under full scene PBR
//! lighting, multiplied by per-pixel ray-marched sun visibility from the
//! terrain heightfield (see `horizon_march.wgsl` for the algorithm; the
//! engine writes the heightfield + sun uniforms).
//!
//! Engine uniform contract (written by the horizon system, not authors):
//!   engine  = (sun_local.xyz, tan(sun_angular_radius))
//!   engine2 = (size_x, size_z, heightfield_resolution, csm_far_bound_m)
//!
//! Near/far split: within the sun's cascade range the terrain casts into the
//! CSM (mesh-accurate self-shadow via `apply_pbr_lighting`), so the march
//! only fades in beyond ~half that range (`engine2.w`; 0 ⇒ march everywhere)
//! — its heightfield-texel-quantized edges never show up close, and near
//! pixels skip the march loop entirely.

#import bevy_pbr::{
    forward_io::VertexOutput,
    pbr_types,
    pbr_functions,
    mesh_bindings::mesh,
    mesh_view_bindings::view,
}
#import lunco::horizon::sun_visibility

struct ShaderParams {
    color_a: vec4<f32>,
    color_b: vec4<f32>,
    color_c: vec4<f32>,
    params:  vec4<f32>,
    params2: vec4<f32>,
    engine:  vec4<f32>,
    engine2: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0)
var<uniform> mat: ShaderParams;

@group(#{MATERIAL_BIND_GROUP}) @binding(1)
var height_map: texture_2d<f32>;

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> @location(0) vec4<f32> {
    var albedo = mat.color_a.rgb;
    // Unset color_a (ShaderMaterial's prop-yellow default) → neutral grey.
    if (distance(albedo, vec3(0.95, 0.85, 0.10)) < 1e-3) { albedo = vec3(0.5); }

    var pbr_input = pbr_types::pbr_input_new();
    pbr_input.flags = mesh[in.instance_index].flags; // keep SHADOW_RECEIVER
    pbr_input.frag_coord = in.position;
    pbr_input.world_position = in.world_position;
    pbr_input.world_normal = pbr_functions::prepare_world_normal(
        normalize(in.world_normal), false, is_front);
    pbr_input.is_orthographic = view.clip_from_view[3].w == 1.0;
    pbr_input.N = pbr_input.world_normal;
    pbr_input.V = pbr_functions::calculate_view(in.world_position, pbr_input.is_orthographic);
    pbr_input.material.base_color = vec4(albedo, 1.0);
    pbr_input.material.perceptual_roughness = 0.95;
    pbr_input.material.metallic = 0.0;
    pbr_input.material.reflectance = vec3(0.5);

    var color = pbr_functions::apply_pbr_lighting(pbr_input);

#ifdef VERTEX_UVS_A
    let csm_far = mat.engine2.w;
    var march_blend = 1.0;
    if (csm_far > 0.0) {
        let cam_d = distance(view.world_position, in.world_position.xyz);
        march_blend = smoothstep(csm_far * 0.5, csm_far * 0.9, cam_d);
    }
    if (march_blend > 0.0) {
        let vis = sun_visibility(
            height_map, in.uv, mat.engine.xyz, mat.engine.w, mat.engine2.xy, mat.engine2.z);
        color = vec4(color.rgb * mix(1.0, vis, march_blend), color.a);
    }
#endif

    color = pbr_functions::main_pass_post_lighting_processing(pbr_input, color);
    return color;
}
