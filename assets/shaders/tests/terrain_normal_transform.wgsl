// Terrain normals must survive a world/local round trip under nonuniform scale.
#import bevy_pbr::forward_io::VertexOutput
#import lunco::terrain::{terrain_detail_normal_to_local, terrain_detail_normal_to_world}
struct Material { unused: f32, }

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let expected = normalize(vec3(0.2 + in.uv.x, 0.4 + in.uv.y, 1.0));
    let world = terrain_detail_normal_to_world(expected, in.instance_index);
    let actual = terrain_detail_normal_to_local(world, in.instance_index);
    let valid = dot(actual, expected) > 0.99999;
    return select(vec4(1.0, 0.0, 0.0, 1.0), vec4(0.0, 1.0, 0.0, 1.0), valid);
}
