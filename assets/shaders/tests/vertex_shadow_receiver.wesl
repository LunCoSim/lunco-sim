import bevy_pbr::render::{forward_io::VertexOutput, mesh_functions, shadows, mesh_view_bindings::view};

// Green: the vacated position is lit. Cyan: the displaced caster casts here.
// Red is a shadow/visible-geometry disagreement; gray is outside the probes.
struct Material { unused: f32, }

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let p = (mesh_functions::get_local_from_world(in.instance_index) * in.world_position).xyz;
    let view_z = dot(vec4(view.view_from_world[0].z, view.view_from_world[1].z,
        view.view_from_world[2].z, view.view_from_world[3].z), in.world_position);
    let visibility = shadows::fetch_directional_shadow(0u, in.world_position,
        in.world_normal, view_z, in.position.xy);
    let old_position = abs(p.x) < 0.25 && abs(p.z + 2.0) < 0.25;
    let new_position = abs(p.x - 3.0) < 0.25 && abs(p.z + 2.0) < 0.25;
    if (old_position) {
        return select(vec4(1.0, 0.0, 0.0, 1.0), vec4(0.0, 1.0, 0.0, 1.0), visibility > 0.9);
    }
    if (new_position) {
        return select(vec4(1.0, 0.0, 0.0, 1.0), vec4(0.0, 1.0, 1.0, 1.0), visibility < 0.1);
    }
    return vec4(vec3(0.2 + 0.3 * visibility), 1.0);
}
