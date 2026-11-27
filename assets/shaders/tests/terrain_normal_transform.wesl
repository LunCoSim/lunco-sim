import bevy_pbr::render::forward_io::VertexOutput;
import super::super::terrain_surface::{terrain_detail_normal_to_local, terrain_detail_normal_to_world, terrain_authored_normal_weight, terrain_relief_residual};

// Terrain normals must survive a world/local round trip under nonuniform scale.
struct Material { unused: f32, }

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let expected = normalize(vec3(0.2 + in.uv.x, 0.4 + in.uv.y, 1.0));
    let world = terrain_detail_normal_to_world(expected, in.instance_index);
    let actual = terrain_detail_normal_to_local(world, in.instance_index);
    let close = terrain_authored_normal_weight(0.8, 1.0, 0.5, 0.0);
    let transition = terrain_authored_normal_weight(0.8, 1.0, 1.5, 0.0);
    let far = terrain_authored_normal_weight(0.8, 1.0, 3.0, 0.0);
    let filtered = terrain_authored_normal_weight(0.8, 1.0, 0.5, 0.2);
    let no_geometry = terrain_authored_normal_weight(0.8, 0.0, 0.5, 0.2);
    let up = vec3(0.0, 1.0, 0.0);
    let authored = normalize(vec3(0.2, 1.0, -0.1));
    let fine = normalize(vec3(-0.3, 1.0, 0.4));
    let same_band = terrain_relief_residual(authored, fine, fine);
    let added_band = terrain_relief_residual(authored, fine, up);
    let expected_band = normalize(vec3(-0.1, 1.0, 0.3));
    let valid = dot(same_band, authored) > 0.99999
        && dot(added_band, expected_band) > 0.99999
        && dot(actual, expected) > 0.99999 && close == 0.0
        && abs(transition - 0.4) < 0.00001 && abs(far - 0.8) < 0.00001
        && abs(filtered - 0.4) < 0.00001 && abs(no_geometry - 0.8) < 0.00001;
    return select(vec4(1.0, 0.0, 0.0, 1.0), vec4(0.0, 1.0, 0.0, 1.0), valid);
}
