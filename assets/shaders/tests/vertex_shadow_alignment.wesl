import bevy_pbr::render::{mesh_functions, view_transformations::position_world_to_clip, mesh_view_bindings::view};
@if((PREPASS_PIPELINE))
import bevy_pbr::prepass::io::{Vertex, VertexOutput};
@if((!(PREPASS_PIPELINE)))
import bevy_pbr::render::forward_io::{Vertex, VertexOutput};

// Camera-dependent deformation must agree in the visible and shadow passes.
// The inactive avatar supplies a different shared shadow origin; cascades must
// resolve the owning camera rather than that origin or the light's position.

//!@default offset 3,0,0
//!@default lod_origin 1.5,9,-10
//!@default lod_weight 0
struct Material { offset: vec3<f32>, lod_origin: vec3<f32>, lod_weight: f32, }
@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> mat: Material;

@vertex
fn vertex(in: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let transform = mesh_functions::get_world_from_local(in.instance_index);
    let morph = mat.lod_weight * clamp(distance(view.lod_view_world_position, mat.lod_origin), 0.0, 1.0);
    let local_position = in.position + mat.offset * (1.0 - morph);
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
