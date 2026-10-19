//! Volumetric combustion exhaust inside the authored unit-cone envelope.
//! Modelica owns delivered activity, visible length, envelope dimensions and
//! fuel colour. USD wires those values independently for every nozzle. WGSL
//! integrates emission/extinction and advects texture; it owns no engine state.
//!
//! Local y + 0.5 runs from nozzle to downstream tip after the authored 180-degree
//! rotation. Clip the camera ray to the envelope's box, then sample only inside
//! the cone. Starting from the analytic entry (rather than a rasterised surface)
//! keeps oblique views, partial-length burns and a camera inside the envelope
//! consistent. Exactly one proxy face contributes; the proxy itself is invisible.
//! The separate Modelica-driven light remains the owner of scene illumination.

#import bevy_pbr::{
    mesh_functions,
    forward_io::VertexOutput,
    mesh_view_bindings::{globals, view},
}
#import lunco::noise::vnoise

//!@ui      core_color    color "Axial colour (hot core)"
//!@default core_color    6.0,3.5,0.9
//!@ui      throttle      0 1   "Throttle (driven by the engine)"
//!@default throttle      0.0
//!@ui      plume_length_fraction 0 1 "Derived visible length in the authored envelope"
//!@default plume_length_fraction 0.0
//!@ui      throttle_exponent 0.1 1 "Visual width response to throttle"
//!@default throttle_exponent 0.35
//!@ui      edge_color    color "Flank colour (cooler outer gas)"
//!@default edge_color    3.0,1.0,0.12
//!@ui      width_idle    0 1   "Half-width fraction at zero throttle"
//!@default width_idle    0.28
//!@ui      flicker       0 1   "Flicker depth; 0 = steady"
//!@default flicker       1.0
//!@ui      flicker_speed 0 20  "Flicker advection speed along the axis"
//!@default flicker_speed 6.0
//!@ui      flicker_scale 0 40  "Flicker cell count across the plume"
//!@default flicker_scale 7.0
//!@ui      density       0 40  "Emission / extinction gain per unit local depth"
//!@default density       9.0
//!@ui      steps         4 64  "Ray-march samples through the volume"
//!@default steps         24
//!@default fuel_color_r 1.0
//!@default fuel_color_g 1.0
//!@default fuel_color_b 1.0
struct Material {
    fuel_color_r: f32,
    fuel_color_g: f32,
    fuel_color_b: f32,
    core_color:    vec3<f32>,
    throttle:      f32,
    plume_length_fraction: f32,
    throttle_exponent: f32,
    edge_color:    vec3<f32>,
    width_idle:    f32,
    flicker:       f32,
    flicker_speed: f32,
    flicker_scale: f32,
    density:       f32,
    steps:         f32,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0)
var<uniform> mat: Material;

// Avoid infinite slab distances for rays parallel to an envelope face.
fn safe_direction(d: vec3<f32>) -> vec3<f32> {
    return select(vec3<f32>(-1.0), vec3<f32>(1.0), d >= vec3<f32>(0.0))
        * max(abs(d), vec3<f32>(0.00001));
}

@fragment
fn fragment(input: VertexOutput, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let t = clamp(mat.throttle, 0.0, 1.0);
    let len = clamp(mat.plume_length_fraction, 0.0, 1.0);
    if t <= 0.0 || len <= 0.0 { return vec4<f32>(0.0); }

    let local_from_world = mesh_functions::get_local_from_world(input.instance_index);
    let surface = (local_from_world * vec4<f32>(input.world_position.xyz, 1.0)).xyz;
    let eye = (local_from_world * vec4<f32>(view.world_position, 1.0)).xyz;
    let ray = normalize(surface - eye);
    let camera_inside = eye.y > -0.5 && eye.y < 0.5 && length(eye.xz) < 0.5 - eye.y;
    if front == camera_inside { return vec4<f32>(0.0); }

    let inverse_ray = 1.0 / safe_direction(ray);
    let slab0 = (vec3<f32>(-1.0, -0.5, -1.0) - eye) * inverse_ray;
    let slab1 = (vec3<f32>(1.0, -0.5 + len, 1.0) - eye) * inverse_ray;
    let near = min(slab0, slab1);
    let far = max(slab0, slab1);
    let entry = max(0.0, max(near.x, max(near.y, near.z)));
    let exit = min(far.x, min(far.y, far.z));
    if exit <= entry { return vec4<f32>(0.0); }

    let response = pow(t, clamp(mat.throttle_exponent, 0.1, 1.0));
    let width = clamp(mat.width_idle, 0.0, 1.0) + (1.0 - clamp(mat.width_idle, 0.0, 1.0)) * response;
    let count = u32(clamp(mat.steps, 4.0, 64.0));
    let step_length = (exit - entry) / f32(count);
    let fuel = max(vec3<f32>(mat.fuel_color_r, mat.fuel_color_g, mat.fuel_color_b), vec3<f32>(0.0));
    var transmission = 1.0;
    var emission = vec3<f32>(0.0);
    for (var i = 0u; i < count; i += 1u) {
        let p = eye + ray * (entry + (f32(i) + 0.5) * step_length);
        let a = p.y + 0.5;
        if a < 0.0 || a > len { continue; }
        let axial = a / max(0.001, len);
        // A compact throat opens into the envelope, then dilutes to a soft tip.
        // Stay strictly inside the cone so none of its triangular edges show.
        let radius = min(0.92 * (1.0 - a), 0.50 + 0.40 * smoothstep(0.0, 0.38, axial)) * width;
        let radial = length(p.xz) / max(0.001, radius);
        if radial >= 1.0 { continue; }
        let noise = vnoise(vec3<f32>(p.x * mat.flicker_scale, p.z * mat.flicker_scale,
            a * mat.flicker_scale - globals.time * mat.flicker_speed));
        let shimmer = 1.0 - clamp(mat.flicker, 0.0, 1.0) * 0.45 * (1.0 - noise);
        let fringe = 1.0 - smoothstep(0.45, 1.0, radial);
        let tail = 1.0 - smoothstep(0.22, 0.92, axial);
        let gas = exp(-2.5 * radial * radial) * fringe * tail * shimmer;
        let optical_depth = max(0.0, mat.density) * gas * step_length * 0.30 * response;
        let absorbed = 1.0 - exp(-optical_depth);
        let hot = (1.0 - smoothstep(0.0, 0.65, radial)) * (1.0 - 0.65 * axial);
        let tint = mix(mat.edge_color * fuel,
            mat.core_color * mix(fuel, vec3<f32>(1.0), 0.75), hot);
        emission += transmission * absorbed * tint;
        transmission *= 1.0 - absorbed;
    }
    let alpha = 1.0 - transmission;
    if alpha < 0.0001 { return vec4<f32>(0.0); }
    // Add shares Bevy's premultiplied pipeline. Its source RGB is already
    // integrated emission, and zero output alpha preserves the background.
    // Returning straight colour here would cancel extinction at the soft edge.
#ifdef BLEND_PREMULTIPLIED_ALPHA
    return vec4<f32>(0.12 * emission, 0.0);
#else
    return vec4<f32>(0.12 * emission / alpha, alpha);
#endif
}
