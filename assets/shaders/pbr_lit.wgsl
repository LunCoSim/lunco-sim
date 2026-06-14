// Shared **PBR-lit mode** for the dynamic `ShaderMaterial`.
//
// A plain `ShaderMaterial` is unlit by default — its `.wgsl` returns a flat
// colour. Importing this module lets a *self-describing* shader get bevy's full
// PBR lighting (directional/point lights, shadows, ambient, fog, tonemapping)
// in one call, while staying dynamic (params reflected from its own `Material`
// struct — no per-shader Rust). It is the reusable version of the PbrInput
// boilerplate the terrain/prop shaders used to hand-copy.
//
//   #import lunco::pbr_lit::lit
//   @fragment
//   fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> @location(0) vec4<f32> {
//       // ... compute albedo/roughness procedurally from `mat` ...
//       return lit(in, is_front, albedo, roughness, metallic, emissive);
//   }
//
// For shaders that perturb the normal (bump/detail mapping) pass it explicitly
// with `lit_n`. For shaders that need to modify the colour *between* lighting
// and post-processing (e.g. the terrain ray-marched shadow), keep doing that
// inline — this module is the common all-in-one path.

#define_import_path lunco::pbr_lit

#import bevy_pbr::{
    forward_io::VertexOutput,
    pbr_types,
    pbr_functions,
    mesh_bindings::mesh,
    mesh_view_bindings::view,
}

// Full PBR lighting using the mesh's geometric normal. `base_color`/`emissive`
// are linear RGB; `perceptual_roughness`/`metallic` are 0..1.
fn lit(
    in: VertexOutput,
    is_front: bool,
    base_color: vec3<f32>,
    perceptual_roughness: f32,
    metallic: f32,
    emissive: vec3<f32>,
) -> vec4<f32> {
    return lit_n(in, is_front, normalize(in.world_normal), base_color, perceptual_roughness, metallic, emissive);
}

// As `lit`, but with a caller-supplied shading normal `n` (world space,
// normalized) — e.g. after procedural bump mapping. The geometric normal is
// still used for the front-facing flip and shadow-receiver flags.
fn lit_n(
    in: VertexOutput,
    is_front: bool,
    n: vec3<f32>,
    base_color: vec3<f32>,
    perceptual_roughness: f32,
    metallic: f32,
    emissive: vec3<f32>,
) -> vec4<f32> {
    var pbr_input = pbr_types::pbr_input_new();
    pbr_input.flags = mesh[in.instance_index].flags; // keep SHADOW_RECEIVER etc.
    pbr_input.frag_coord = in.position;
    pbr_input.world_position = in.world_position;
    pbr_input.world_normal = pbr_functions::prepare_world_normal(
        normalize(in.world_normal), false, is_front);
    pbr_input.is_orthographic = view.clip_from_view[3].w == 1.0;
    pbr_input.N = n;
    pbr_input.V = pbr_functions::calculate_view(in.world_position, pbr_input.is_orthographic);
    pbr_input.material.base_color = vec4(base_color, 1.0);
    pbr_input.material.perceptual_roughness = perceptual_roughness;
    pbr_input.material.metallic = metallic;
    pbr_input.material.emissive = vec4(emissive, 1.0);
    pbr_input.material.reflectance = vec3(0.5);
    var color = pbr_functions::apply_pbr_lighting(pbr_input);
    return pbr_functions::main_pass_post_lighting_processing(pbr_input, color);
}
