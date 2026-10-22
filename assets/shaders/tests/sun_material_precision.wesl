import bevy_pbr::render::mesh_view_bindings::lights;
import bevy_pbr::render::{mesh_functions, view_transformations::position_world_to_clip};
@if((PREPASS_PIPELINE))
import bevy_pbr::prepass::io::{Vertex, VertexOutput};
@if((!(PREPASS_PIPELINE)))
import bevy_pbr::render::forward_io::{Vertex, VertexOutput};

// GPU comparison against the native light, independent of material update tolerances.
//!@engine sun_dir
//!@engine sun_dir_world
//!@engine sun_tan_radius
struct Material {
    sun_dir: vec3<f32>,
    sun_tan_radius: f32,
    sun_dir_world: vec3<f32>,
}
@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> mat: Material;

// The numerical probe keeps LOD positions unchanged. Both stages share this
// Material definition and use Bevy's native mesh/view transformations.
@vertex
fn vertex(in: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let transform = mesh_functions::get_world_from_local(in.instance_index);
    out.world_position = mesh_functions::mesh_position_local_to_world(transform, vec4(in.position, 1.0));
    out.position = position_world_to_clip(out.world_position.xyz);
    @if((UNCLIPPED_DEPTH_ORTHO_EMULATION)) out.unclipped_depth = out.position.z;
    @if((UNCLIPPED_DEPTH_ORTHO_EMULATION)) out.position.z = min(out.position.z, 1.0);
    @if((VERTEX_UVS_A)) out.uv = in.uv;
    @if((PREPASS_PIPELINE) && (NORMAL_PREPASS_OR_DEFERRED_PREPASS)) out.world_normal = mesh_functions::mesh_normal_local_to_world(in.normal, in.instance_index);
    @if((!(PREPASS_PIPELINE))) out.world_normal = mesh_functions::mesh_normal_local_to_world(in.normal, in.instance_index);
    @if((PREPASS_PIPELINE) && (MOTION_VECTOR_PREPASS)) out.previous_world_position = mesh_functions::mesh_position_local_to_world(
        mesh_functions::get_previous_world_from_local(in.instance_index), vec4(in.position, 1.0));
    @if((VERTEX_OUTPUT_INSTANCE_INDEX)) out.instance_index = in.instance_index;
    return out;
}

@fragment
fn fragment() -> @location(0) vec4<f32> {
    let native_day = lights.directional_lights[0].direction_to_light.y > 0.0;
    let valid = (mat.sun_dir.y > 0.0) == native_day
        && (mat.sun_dir_world.y > 0.0) == native_day && length(mat.sun_dir) > 0.5;
    return select(vec4(1.0, 0.0, 0.0, 1.0), vec4(0.0, 1.0, 1.0, 1.0), valid);
}
