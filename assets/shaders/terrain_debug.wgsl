//! Dedicated streamed-terrain diagnostic material.
//!
//! This file is selected by the terrain diagnostic tool. It is deliberately a
//! separate material from `terrain_geomorph.wgsl`: diagnostic colouring must not
//! add branches, uniforms, texture reads, or material variants to production
//! lunar rendering.
//!
//! `mode`: 1 = slope hazard, 2 = CDLOD depth. `opacity` controls diagnostic
//! colour intensity. The vertex stage is the same CDLOD morph and edge-stitch
//! contract as the production material, so inspecting the stream does not alter
//! its topology or hide cracks.

#import bevy_pbr::{
    mesh_functions,
    view_transformations::position_world_to_clip,
    forward_io::VertexOutput,
    mesh_view_bindings::view,
}
#import lunco::transfer::{slope_hazard_color, slope_of}

//!@ui      mode        1 2   "Diagnostic (1 slope, 2 LOD depth)"
//!@default mode        1
//!@ui      opacity     0 1   "Diagnostic colour intensity"
//!@default opacity     1.0
//!@ui      safe_rad    0 1.57 "Safe slope (rad)"
//!@default safe_rad    0.2617994
//!@ui      cliff_rad   0 1.57 "Cliff slope (rad)"
//!@default cliff_rad   0.5235988
//!@ui      lod         0 12  "CDLOD depth"
//!@default lod         0
//!@default morph_start  1.0e20
//!@default morph_end    1.0e21
//!@default stitch_edges 0,0,0,0
struct Material {
    mode:         f32,
    opacity:      f32,
    safe_rad:     f32,
    cliff_rad:    f32,
    lod:          f32,
    morph_start:  f32,
    morph_end:    f32,
    stitch_edges: vec4<f32>,
}
@group(#{MATERIAL_BIND_GROUP}) @binding(0)
var<uniform> material: Material;

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
    let base_world = mesh_functions::mesh_position_local_to_world(
        world_from_local,
        vec4<f32>(vertex.position, 1.0),
    );
    let dist = distance(base_world.xyz, view.world_position);
    var morph = 0.0;
    if (material.morph_end > material.morph_start) {
        morph = smoothstep(material.morph_start, material.morph_end, dist);
    }
    let edge_stitch = max(
        max(vertex.edge_mask.x * material.stitch_edges.x,
            vertex.edge_mask.y * material.stitch_edges.y),
        max(vertex.edge_mask.z * material.stitch_edges.z,
            vertex.edge_mask.w * material.stitch_edges.w),
    );
    let m = max(morph, edge_stitch);
    let local_pos = mix(vertex.position, vertex.morph_target, m);
    let local_normal = normalize(mix(vertex.normal, vertex.morph_normal, m));
    out.world_position = mesh_functions::mesh_position_local_to_world(
        world_from_local,
        vec4<f32>(local_pos, 1.0),
    );
    out.position = position_world_to_clip(out.world_position.xyz);
    out.world_normal = mesh_functions::mesh_normal_local_to_world(
        local_normal,
        vertex.instance_index,
    );
#ifdef VERTEX_UVS_A
    out.uv = vertex.uv;
#endif
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    out.instance_index = vertex.instance_index;
#endif
    return out;
}

fn lod_color(lod: u32) -> vec3<f32> {
    switch (lod % 8u) {
        case 0u: { return vec3<f32>(0.20, 0.40, 1.00); }
        case 1u: { return vec3<f32>(0.20, 0.85, 0.95); }
        case 2u: { return vec3<f32>(0.20, 0.90, 0.35); }
        case 3u: { return vec3<f32>(0.85, 0.95, 0.20); }
        case 4u: { return vec3<f32>(1.00, 0.60, 0.15); }
        case 5u: { return vec3<f32>(1.00, 0.25, 0.20); }
        case 6u: { return vec3<f32>(0.95, 0.30, 0.85); }
        default: { return vec3<f32>(0.90, 0.90, 0.90); }
    }
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> @location(0) vec4<f32> {
    let n = normalize(in.world_normal);
    var colour = vec3<f32>(0.5, 0.5, 0.5);
    if (material.mode < 1.5) {
        colour = slope_hazard_color(
            slope_of(n), material.safe_rad, material.cliff_rad);
    } else {
        colour = lod_color(u32(material.lod + 0.5));
    }
    let shade = 0.45 + 0.55 * clamp(n.y, 0.0, 1.0);
    let neutral = vec3<f32>(0.5, 0.5, 0.5) * shade;
    return vec4<f32>(mix(neutral, colour * shade, material.opacity), 1.0);
}
