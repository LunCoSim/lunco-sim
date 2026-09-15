//! CDLOD geomorph vertex stage for the canonical terrain material.
//!
//! The fragment/material contract is authored by the USD Shader prim through
//! `info:wgsl:sourceAsset` and is normally `terrain_layered.wgsl`. This file is
//! only the optional `info:wgsl:vertexAsset` stage for streamed CDLOD tiles.
//! Keeping geometry selection here and appearance selection in USD means the
//! static and streamed representations cannot silently acquire different
//! shading laws.
//!
//! Each LOD-tile vertex carries two positions: its own LOD `POSITION` and the
//! `MORPH_TARGET` (the vertex snapped to the parent's coarser even lattice, baked
//! by `bake_tile_mesh`). The vertex shader lerps `POSITION → MORPH_TARGET` by
//! camera distance over the node's CDLOD morph band, so a tile collapses smoothly
//! onto its parent. No texture fetch, no compute → wasm-safe.
//!
//! `ShaderMaterial::specialize` swaps this stage in when the USD projection
//! supplies `vertex_shader` and binds `ATTRIBUTE_MORPH_TARGET` at `@location(8)`,
//! `ATTRIBUTE_MORPH_NORMAL` at `@location(9)`, and `ATTRIBUTE_MORPH_EDGE` at
//! `@location(10)`. Params are reflected from the canonical fragment's
//! `struct Material`; this file repeats that layout only because the vertex
//! stage reads the morph fields from the same uniform block.

#import bevy_pbr::{
    mesh_functions,
    view_transformations::position_world_to_clip,
    forward_io::VertexOutput,
    mesh_view_bindings::view,
}
// Keep this layout byte-for-byte compatible with terrain_layered.wgsl. The
// fragment stage owns the reflected UI/default/engine annotations; this stage
// only needs the final morph fields, but the uniform is shared by both stages.
struct Material {
    albedo:            vec3<f32>,
    micro_scale:       f32,
    micro_bump:        f32,
    roughness:         f32,
    macro_clump_scale: f32,
    macro_bump:        f32,
    mid_scale:         f32,
    mid_bump:          f32,
    fine_scale:        f32,
    fine_bump:         f32,
    rough_mix:         f32,
    mottle:            f32,
    weight_albedo:     f32,
    weight_mineral:    f32,
    weight_rough:      f32,
    weight_ao:         f32,
    weight_normal:     f32,
    surge_amp:         f32,
    surge_width:       f32,
    photometry_gain:   f32,
    sun_tan_radius:    f32,
    sun_dir:           vec3<f32>,
    sun_dir_world:     vec3<f32>,
    hf_size:           vec2<f32>,
    hf_res:            f32,
    csm_far:           f32,
    shadow_cache_on:   f32,
    horizon_march_steps: f32,
    map_texel_size_m:  f32,
    derived_surface_on: f32,
    derived_normal_on:  f32,
    authored_surface_on: f32,
    authored_normal_on:  f32,
    terrain_half_extent: f32,
    morph_start:       f32,
    morph_end:         f32,
    stitch_edges:      vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0)
var<uniform> mat: Material;

// Standard mesh attributes plus the morph target at location 8 (added to the
// layout by ShaderMaterial::specialize when vertex_shader is set).
struct GeoVertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(8) morph_target: vec3<f32>,
    @location(9) morph_normal: vec3<f32>,
    @location(10) edge_mask: vec4<f32>,
};

@vertex
fn vertex(vertex: GeoVertex) -> VertexOutput {
    var out: VertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);

    // Camera distance from the un-morphed world position (big_space rebases both
    // view and mesh into the same render frame → true eye→vertex distance).
    let base_world = mesh_functions::mesh_position_local_to_world(
        world_from_local,
        vec4<f32>(vertex.position, 1.0),
    );
    let dist = distance(base_world.xyz, view.world_position);

    // CDLOD morph: 0 near (own LOD) → 1 far (collapse onto parent lattice). Root
    // tiles pass morph_end <= morph_start → no morph.
    var morph = 0.0;
    if (mat.morph_end > mat.morph_start) {
        morph = smoothstep(mat.morph_start, mat.morph_end, dist);
    }
    // `morph` alone — deliberately. A per-tile term here (the old `reveal` settle)
    // makes two neighbours at the same depth and distance disagree at their shared
    // edge, cracking the seam. Keep this a pure function of world position.
    // Restricted quadtree selection guarantees that a resident neighbour is
    // either the same depth or one level coarser. On the latter boundary the
    // fine edge uses its parent lattice immediately, the exact surface sampled
    // by the coarser tile. Same-depth edges remain untouched.
    let edge_stitch = max(
        max(vertex.edge_mask.x * mat.stitch_edges.x, vertex.edge_mask.y * mat.stitch_edges.y),
        max(vertex.edge_mask.z * mat.stitch_edges.z, vertex.edge_mask.w * mat.stitch_edges.w),
    );
    let m = max(morph, edge_stitch);
    let local_pos = mix(vertex.position, vertex.morph_target, m);
    // Shade the surface we actually DRAW: the position lerps toward the parent
    // lattice, so the normal must lerp with it. Leaving the fine normal here made
    // a fully-morphed tile shade with detail its geometry no longer has — up to
    // ~22 deg of error, flipping N.L negative on some quads and making new LOD
    // tiles appear black.
    let local_normal = normalize(mix(vertex.normal, vertex.morph_normal, m));

    out.world_position = mesh_functions::mesh_position_local_to_world(
        world_from_local,
        vec4<f32>(local_pos, 1.0),
    );
    out.position = position_world_to_clip(out.world_position.xyz);
    out.world_normal = mesh_functions::mesh_normal_local_to_world(local_normal, vertex.instance_index);
#ifdef VERTEX_UVS_A
    out.uv = vertex.uv;
#endif
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    out.instance_index = vertex.instance_index;
#endif
    return out;
}
