//! Default terrain material with ray-marched heightfield sun shadows.
//!
//! Applied automatically by `lunco-environment`'s horizon system to a
//! `HorizonShadowTerrain` that authors no custom shader: plain albedo
//! (`color_a`, taken from the prim's `displayColor`) under full scene PBR
//! lighting, multiplied by per-pixel ray-marched sun visibility from the
//! terrain heightfield (see `horizon_march.wgsl` for the algorithm; the
//! engine writes the heightfield + sun uniforms).
//!
//! Near/far split: within the sun's cascade range the terrain casts into the
//! CSM (mesh-accurate self-shadow via `apply_pbr_lighting`), so the march
//! only fades in beyond ~half that range (`csm_far`; 0 ⇒ march everywhere)
//! — its heightfield-texel-quantized edges never show up close, and near
//! pixels skip the march loop entirely.

#import bevy_pbr::{
    forward_io::VertexOutput,
    pbr_types,
    pbr_functions,
    mesh_bindings::mesh,
    mesh_view_bindings::view,
    mesh_view_bindings::lights,
}
#import lunco::horizon::{sun_visibility_resolved, SHADOW_FILL}
#import lunco::lunar::regolith_factor

// Dynamic, self-describing parameters (reflected from this file). Only the
// albedo is author-settable; the rest are engine-filled by the horizon system.
//!@ui      albedo color "Albedo"
//!@default albedo 0.13,0.13,0.13
//!@engine  sun_dir
//!@engine  sun_dir_world
//!@engine  sun_tan_radius
//!@engine  hf_size
//!@engine  hf_res
//!@engine  csm_far
//!@engine  shadow_cache_on
struct Material {
    albedo:         vec3<f32>,
    sun_tan_radius: f32,
    sun_dir:        vec3<f32>,    // engine: terrain-local to-sun (heightfield march)
    sun_dir_world:  vec3<f32>,    // engine: world-space to-sun (lunar BRDF)
    hf_size:        vec2<f32>,
    hf_res:         f32,
    csm_far:        f32,
    shadow_cache_on: f32,  // engine: 1 = sample pre-baked shadow cache, 0 = ray-march
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0)
var<uniform> mat: Material;

@group(#{MATERIAL_BIND_GROUP}) @binding(1)
var height_map: texture_2d<f32>;

// Pre-baked horizon shadow cache (R8Unorm, 0..1 sun visibility) — sampled
// with a single `textureSampleLevel` when `mat.shadow_cache_on > 0.5` instead
// of the 48-step heightfield ray-march. Filterable (GPU bilinear interp).
@group(#{MATERIAL_BIND_GROUP}) @binding(10)
var shadow_cache: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(11)
var shadow_cache_sampler: sampler;

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> @location(0) vec4<f32> {
    let albedo = mat.albedo;

    var pbr_input = pbr_types::pbr_input_new();
    pbr_input.flags = mesh[in.instance_index].flags; // keep SHADOW_RECEIVER
    pbr_input.frag_coord = in.position;
    pbr_input.world_position = in.world_position;
    pbr_input.world_normal = pbr_functions::prepare_world_normal(
        normalize(in.world_normal), false, is_front);
    pbr_input.is_orthographic = view.clip_from_view[3].w == 1.0;
    pbr_input.N = pbr_input.world_normal;
    pbr_input.V = pbr_functions::calculate_view(in.world_position, pbr_input.is_orthographic);
    // Lunar regolith photometry (Lommel-Seeliger + opposition surge); see
    // lunar_brdf.wgsl. Pre-multiplies base_color so bevy's Lambert completes it.
    // World-space to-sun comes from the engine (the CPU-picked canonical sun),
    // not directional_lights[0] — the earthshine fill light can shuffle that.
    // Guarded: zero until the engine fills it (unwired terrain / first frame).
    var lunar_k = 1.0;
    let sw = mat.sun_dir_world;
    if (dot(sw, sw) > 0.25) {
        lunar_k = regolith_factor(pbr_input.N, normalize(sw), pbr_input.V);
    }
    pbr_input.material.base_color = vec4(albedo * lunar_k, 1.0);
    pbr_input.material.perceptual_roughness = 0.95;
    pbr_input.material.metallic = 0.0;
    pbr_input.material.reflectance = vec3(0.5);

    var color = pbr_functions::apply_pbr_lighting(pbr_input);

#ifdef VERTEX_UVS_A
    let csm_far = mat.csm_far;
    var march_blend = 1.0;
    if (csm_far > 0.0) {
        let cam_d = distance(view.world_position, in.world_position.xyz);
        march_blend = smoothstep(csm_far * 0.5, csm_far * 0.9, cam_d);
    }
    if (march_blend > 0.0) {
        let vis = sun_visibility_resolved(
            shadow_cache, shadow_cache_sampler, mat.shadow_cache_on,
            height_map, in.uv, mat.sun_dir, mat.sun_tan_radius, mat.hf_size, mat.hf_res);
        color = vec4(color.rgb * mix(1.0, vis, march_blend), color.a);
    }
    // Unconditional shadow fill — see `SHADOW_FILL` (lunco::horizon). Applied
    // outside the march branch so near (CSM) and far (march) pixels get the
    // SAME lift; a branch-local fill painted a bright ring at the handoff.
    color = vec4(color.rgb + albedo * SHADOW_FILL, color.a);
#endif

    color = pbr_functions::main_pass_post_lighting_processing(pbr_input, color);
    return color;
}
