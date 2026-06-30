//! Flat CDLOD geomorph tile — the same `@vertex` morph as `terrain_geomorph.wgsl`
//! but a trivial flat-lit fragment (single `base_color`, no FBM). Lightweight.
//!
//! Used for two inspector-selectable terrain shader modes (the colour is set per
//! tile by the streamer, not the shader):
//!   * **Plain** — `base_color` a flat lunar grey → cheapest "no fancy shader" look.
//!   * **Debug-LOD** — `base_color` a per-quadtree-depth tint → SEE the LOD
//!     structure refine as the camera moves.
//!
//! Keeps the vertex geomorph so tiles still don't pop, and rides `ShaderMaterial`
//! (`m.shader` + `m.vertex_shader` both point here) like every other shader.

#import bevy_pbr::{
    mesh_functions,
    view_transformations::position_world_to_clip,
    forward_io::VertexOutput,
    mesh_view_bindings::view,
}
#import lunco::pbr_lit::lit

//!@ui      base_color  color "Tile colour"
//!@default base_color  0.4,0.4,0.4
//!@ui      roughness   0 1   "Roughness"
//!@default roughness   1.0
//!@default morph_start  1.0e20
//!@default morph_end    1.0e21
//!@default reveal       1.0
struct Material {
    base_color:  vec3<f32>,
    roughness:   f32,
    morph_start: f32,
    morph_end:   f32,
    reveal:      f32,  // 1 = own geometry; <1 = settling in from the parent lattice
}
@group(#{MATERIAL_BIND_GROUP}) @binding(0)
var<uniform> mat: Material;

struct GeoVertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(8) morph_target: vec3<f32>,
};

@vertex
fn vertex(vertex: GeoVertex) -> VertexOutput {
    var out: VertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    let base_world = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(vertex.position, 1.0));
    let dist = distance(base_world.xyz, view.world_position);
    var morph = 0.0;
    if (mat.morph_end > mat.morph_start) {
        morph = smoothstep(mat.morph_start, mat.morph_end, dist);
    }
    // Reveal "settle" (see terrain_geomorph.wgsl): grow in from the parent lattice.
    let m = max(morph, 1.0 - mat.reveal);
    let local_pos = mix(vertex.position, vertex.morph_target, m);
    out.world_position = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(local_pos, 1.0));
    out.position = position_world_to_clip(out.world_position.xyz);
    out.world_normal = mesh_functions::mesh_normal_local_to_world(vertex.normal, vertex.instance_index);
#ifdef VERTEX_UVS_A
    out.uv = vertex.uv;
#endif
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    out.instance_index = vertex.instance_index;
#endif
    return out;
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> @location(0) vec4<f32> {
    return lit(in, is_front, mat.base_color, mat.roughness, 0.0, vec3(0.0));
}
