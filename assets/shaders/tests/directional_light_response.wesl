import bevy_pbr::render::{
    forward_io::VertexOutput, mesh_view_bindings::lights, pbr_functions, pbr_types,
};
import super::super::terrain_surface::terrain_set_sun_response;

struct Material { unused: f32, }

// Exercise the real native accumulator and the shared material admission path.
// Both lights face this synthetic normal; neither is hidden by a shadow.
@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    if (lights.n_directional_lights != 2u) {
        return vec4(1.0, 0.0, 0.0, 1.0);
    }
    let mask = lights.material_response_light_mask;
    if (mask != 1u && mask != 2u) { return vec4(1.0, 0.0, 0.0, 1.0); }
    let index = firstTrailingBit(mask);
    let sun = normalize(lights.directional_lights[index].direction_to_light);
    let fill = normalize(lights.directional_lights[1u - index].direction_to_light);
    // The authored fill is green and the Sun is white. This also catches a
    // one-bit mask that names the wrong light after per-view packing.
    let sun_color = lights.directional_lights[index].color.rgb;
    let fill_color = lights.directional_lights[1u - index].color.rgb;
    let authored_sun = sun_color.r > 0.8 * sun_color.g && fill_color.g > 1.5 * fill_color.r;
    var input = pbr_types::pbr_input_new();
    input.world_position = in.world_position;
    input.frag_coord = in.position;
    input.N = normalize(sun + fill);
    input.world_normal = input.N;
    input.V = input.N;
    input.material.base_color = vec4(0.5, 0.5, 0.5, 1.0);
    input.material.perceptual_roughness = 0.8;
    let default_identity = input.directional_light_scale_mask == 0u
        && input.directional_light_scale == 1.0;
    let full = pbr_functions::apply_pbr_lighting(input).rgb;
    input.directional_light_scale_mask = mask;
    input.directional_light_scale = 0.0;
    let without_sun = pbr_functions::apply_pbr_lighting(input).rgb;
    input.directional_light_scale_mask = 3u;
    let indirect = pbr_functions::apply_pbr_lighting(input).rgb;

    input.directional_light_scale_mask = 0u;
    input.directional_light_scale = 1.0;
    terrain_set_sun_response(&input, sun, 0.25, 1.0, 0.0);
    let selected = input.directional_light_scale == 0.25
        && input.directional_light_scale_mask == mask;
    let quarter = pbr_functions::apply_pbr_lighting(input).rgb;
    let expected = without_sun + 0.25 * (full - without_sun);
    let tolerance = 0.00001 * max(vec3(1.0), abs(expected));
    let linear = all(abs(quarter - expected) <= tolerance);
    let both_visible = length(full - without_sun) > 0.00001
        && length(without_sun - indirect) > 0.00001;

    // Missing Sun, negative response and out-of-range visibility are controls.
    terrain_set_sun_response(&input, vec3(0.0), 0.0, 0.0, 1.0);
    let missing_unchanged = input.directional_light_scale == 0.25;
    terrain_set_sun_response(&input, sun, -1.0, 1.0, 0.0);
    let negative_clamped = input.directional_light_scale == 0.0;
    terrain_set_sun_response(&input, sun, 20.0, -1.0, 2.0);
    let hidden = input.directional_light_scale == 0.0;
    terrain_set_sun_response(&input, sun, 20.0, 2.0, 2.0);
    let bounded = input.directional_light_scale == 8.0;
    let valid = authored_sun && default_identity && selected && linear && both_visible && missing_unchanged
        && negative_clamped && hidden && bounded;
    let pass_color = select(vec4(0.0, 1.0, 0.0, 1.0),
        vec4(0.0, 1.0, 1.0, 1.0), in.position.x >= 320.0);
    return select(vec4(1.0, 0.0, 0.0, 1.0), pass_color, valid);
}
