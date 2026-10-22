import bevy_pbr::render::{mesh_functions, view_transformations::position_world_to_clip};
@if((PREPASS_PIPELINE))
import bevy_pbr::prepass::io::{Vertex, VertexOutput};
@if((!(PREPASS_PIPELINE)))
import bevy_pbr::render::forward_io::{Vertex, VertexOutput};

// A rigid offset must affect both the visible mesh and its native shadow.

//!@default offset 3,0,0
struct Material { offset: vec3<f32>, }
@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> mat: Material;

@vertex
fn vertex(in: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let transform = mesh_functions::get_world_from_local(in.instance_index);
    let local_position = in.position + mat.offset;
    out.world_position = mesh_functions::mesh_position_local_to_world(transform, vec4(local_position, 1.0));
    out.position = position_world_to_clip(out.world_position.xyz);
    @if((UNCLIPPED_DEPTH_ORTHO_EMULATION)) out.unclipped_depth = out.position.z;
    @if((UNCLIPPED_DEPTH_ORTHO_EMULATION)) out.position.z = min(out.position.z, 1.0);
    @if((VERTEX_UVS_A)) out.uv = in.uv;
    @if((PREPASS_PIPELINE) && (NORMAL_PREPASS_OR_DEFERRED_PREPASS)) out.world_normal = mesh_functions::mesh_normal_local_to_world(in.normal, in.instance_index);
    @if((!(PREPASS_PIPELINE))) out.world_normal = mesh_functions::mesh_normal_local_to_world(in.normal, in.instance_index);
    @if((PREPASS_PIPELINE) && (MOTION_VECTOR_PREPASS)) out.previous_world_position = mesh_functions::mesh_position_local_to_world(
        mesh_functions::get_previous_world_from_local(in.instance_index), vec4(local_position, 1.0));
    @if((VERTEX_OUTPUT_INSTANCE_INDEX)) out.instance_index = in.instance_index;
    return out;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    return vec4(0.0, 0.0, 1.0, 1.0);
}
