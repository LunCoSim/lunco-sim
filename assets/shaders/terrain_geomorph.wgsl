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
    mesh_view_bindings::view,
}
#ifdef PREPASS_PIPELINE
#import bevy_pbr::prepass_io::VertexOutput
#else
#import bevy_pbr::forward_io::VertexOutput
#endif
// Motion-vector view flags also occur on forward pipelines, whose output and
// view bindings contain no previous-position data.
#ifdef PREPASS_PIPELINE
#ifdef MOTION_VECTOR_PREPASS
#import bevy_pbr::prepass_bindings::previous_view_uniforms
#endif
#endif

// Keep this layout byte-for-byte compatible with terrain_layered.wgsl. The
// fragment stage owns the reflected UI/default/engine annotations; this stage
// only needs the final morph fields, but the uniform is shared by both stages.
struct Material {
    albedo:            vec3<f32>,
    micro_scale:       f32,
    micro_bump:        f32,
    micro_albedo:      f32,
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
    morph_start:       f32,
    sun_dir_world:     vec3<f32>,
    morph_end:         f32,
    hf_size:           vec2<f32>,
    hf_res:            f32,
    terrain_geometry_on: f32,
    csm_far:           f32,
    shadow_cache_on:   f32,
    horizon_march_steps: f32,
    map_texel_size_m:  f32,
    derived_surface_on: f32,
    derived_normal_on:  f32,
    authored_surface_on: f32,
    authored_normal_on:  f32,
    terrain_half_extent: f32,
    site_weight_albedo: f32,
    site_weight_rough: f32,
    site_weight_ao: f32,
    site_blend_widths_m: vec4<f32>,
    stitch_edges:      vec4<f32>,
    site_base_color: vec4<f32>,
    site_photometry: vec4<f32>,
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

// The same morph/stitch law is used by the color, depth, normal, and shadow
// passes. `eye` is the viewing camera, including in Bevy's shadow views.
fn morph_weight(vertex: GeoVertex, world_position: vec3<f32>, eye: vec3<f32>) -> f32 {
    let dist = distance(world_position, eye);
    // CDLOD morph: 0 near (own LOD) → 1 far (collapse onto parent lattice). Root
    // tiles pass morph_end <= morph_start → no morph.
    var morph = 0.0;
    if (mat.morph_end > mat.morph_start) {
        morph = smoothstep(mat.morph_start, mat.morph_end, dist);
    }
    // Restricted quadtree selection guarantees that a resident neighbour is
    // either the same depth or one level coarser. On the latter boundary the
    // fine edge uses its parent lattice immediately, the exact surface sampled
    // by the coarser tile. Same-depth edges remain untouched.
    let edge_stitch = max(
        max(vertex.edge_mask.x * mat.stitch_edges.x, vertex.edge_mask.y * mat.stitch_edges.y),
        max(vertex.edge_mask.z * mat.stitch_edges.z, vertex.edge_mask.w * mat.stitch_edges.w),
    );
    return max(morph, edge_stitch);
}

@vertex
fn vertex(vertex: GeoVertex) -> VertexOutput {
    var out: VertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    let base_world = mesh_functions::mesh_position_local_to_world(
        world_from_local, vec4(vertex.position, 1.0));
    // Shadow-view origins are light cameras. Bevy carries the viewing camera's
    // LOD origin separately so every cascade deforms the same visible surface.
    let m = morph_weight(vertex, base_world.xyz, view.lod_view_world_position);
    let local_pos = mix(vertex.position, vertex.morph_target, m);
    // Position and shading normal follow the same parent-lattice interpolation.
    let local_normal = normalize(mix(vertex.normal, vertex.morph_normal, m));

    out.world_position = mesh_functions::mesh_position_local_to_world(
        world_from_local,
        vec4<f32>(local_pos, 1.0),
    );
    out.position = position_world_to_clip(out.world_position.xyz);
#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
    out.unclipped_depth = out.position.z;
    out.position.z = min(out.position.z, 1.0);
#endif
#ifdef PREPASS_PIPELINE
#ifdef NORMAL_PREPASS_OR_DEFERRED_PREPASS
    out.world_normal = mesh_functions::mesh_normal_local_to_world(local_normal, vertex.instance_index);
#endif
#else
    out.world_normal = mesh_functions::mesh_normal_local_to_world(local_normal, vertex.instance_index);
#endif
#ifdef PREPASS_PIPELINE
#ifdef MOTION_VECTOR_PREPASS
    // Recover the previous camera origin from its rigid view transform, then
    // evaluate the same morph band in the previous render frame.
    let previous_view = previous_view_uniforms.view_from_world;
    let previous_eye = -vec3(
        dot(previous_view[0].xyz, previous_view[3].xyz),
        dot(previous_view[1].xyz, previous_view[3].xyz),
        dot(previous_view[2].xyz, previous_view[3].xyz));
    let previous_transform = mesh_functions::get_previous_world_from_local(vertex.instance_index);
    let previous_base = mesh_functions::mesh_position_local_to_world(
        previous_transform, vec4(vertex.position, 1.0));
    let previous_m = morph_weight(vertex, previous_base.xyz, previous_eye);
    out.previous_world_position = mesh_functions::mesh_position_local_to_world(
        previous_transform, vec4(mix(vertex.position, vertex.morph_target, previous_m), 1.0));
#endif
#endif
#ifdef VERTEX_UVS_A
    out.uv = vertex.uv;
#endif
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    out.instance_index = vertex.instance_index;
#endif
    return out;
}
