//! CDLOD geomorph terrain tile — self-describing `ShaderMaterial` with a custom
//! `@vertex` stage (the one place a vertex override lives in this repo).
//!
//! Each LOD tile vertex carries two positions: its own LOD `POSITION` and the
//! `MORPH_TARGET` (the vertex snapped to the parent's coarser even lattice, baked
//! by `bake_tile_mesh`). The vertex shader lerps `POSITION → MORPH_TARGET` by
//! camera distance over the node's CDLOD morph band, so a tile collapses smoothly
//! onto its parent instead of popping when the LOD switches. No texture fetch, no
//! compute → wasm-safe.
//!
//! Driven by `ShaderMaterial` (NOT a bespoke material): `m.shader` and
//! `m.vertex_shader` both point here; `m.vertex_shader = Some` makes
//! `ShaderMaterial::specialize` swap the vertex stage and bind
//! `ATTRIBUTE_MORPH_TARGET` at `@location(8)`. Params are reflected from
//! `struct Material` like any self-describing shader.

#import bevy_pbr::{
    mesh_functions,
    view_transformations::position_world_to_clip,
    forward_io::VertexOutput,
    mesh_view_bindings::view,
}
#import lunco::pbr_lit::lit

//!@ui      base_color  color "Tile colour (LOD tint)"
//!@default base_color  0.4,0.4,0.4
//!@ui      roughness   0 1   "Roughness"
//!@default roughness   1.0
//!@default morph_start  1.0e20
//!@default morph_end    1.0e21
struct Material {
    base_color:  vec3<f32>,
    roughness:   f32,
    morph_start: f32,  // distance where geomorph toward the parent begins
    morph_end:   f32,  // distance where the parent fully takes over
}
@group(#{MATERIAL_BIND_GROUP}) @binding(0)
var<uniform> mat: Material;

// Vertex input: standard mesh attributes + the morph-target at location 8
// (added to the layout by ShaderMaterial::specialize when vertex_shader is set).
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

    // Camera distance measured from the un-morphed world position (big_space
    // rebases both the view and the mesh into the same render-space frame, so this
    // is the true eye→vertex distance).
    let base_world = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(vertex.position, 1.0));
    let dist = distance(base_world.xyz, view.world_position);

    // CDLOD morph: 0 near (own LOD geometry) → 1 far (collapse onto the parent
    // lattice). Root tiles pass morph_end <= morph_start → no morph.
    var morph = 0.0;
    if (mat.morph_end > mat.morph_start) {
        morph = smoothstep(mat.morph_start, mat.morph_end, dist);
    }
    let local_pos = mix(vertex.position, vertex.morph_target, morph);

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
