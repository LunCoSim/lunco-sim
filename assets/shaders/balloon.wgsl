//! Ball / balloon material for the general `ShaderMaterial`.
//!
//! Draws a lat-long checkerboard on a sphere so its rotation is obvious.
//!
//! ## Covers ANY sphere mesh (no UV dependence)
//! The pattern is computed from the **object-space surface direction**, not from
//! the mesh UVs. We recover object space by rotating the interpolated world
//! normal back through the model matrix' rotation. This means:
//!   * it tiles the *entire* sphere with no seams or uncovered patches, even on
//!     an icosphere (whose UVs are distorted) — coverage is uniform by
//!     construction;
//!   * it is still **mesh-fixed** (object space rotates with the geometry), so
//!     the checker spins with the ball and reveals rotation.
//!
//! ## Params
//!   param0 = wedge_count   (longitude cells, default 8)
//!   param1 = band_count    (latitude cells, default 6)
//!   param3 = marker_wedges (default 0 = none; >0 paints that many lead wedges)
//!   color_a / color_b = alternating cells   color_c = marker wedge + poles
//!
//! Edit live (hot-reload) or tweak via `SetObjectProperty { property:"param0".. }`.

#import bevy_pbr::{
    forward_io::VertexOutput,
    pbr_types,
    pbr_functions,
    mesh_bindings::mesh,
    mesh_view_bindings::view,
    mesh_functions,
}

const TAU: f32 = 6.28318530718;
const PI:  f32 = 3.14159265359;

struct ShaderParams {
    color_a: vec4<f32>,
    color_b: vec4<f32>,
    color_c: vec4<f32>,
    params:  vec4<f32>,
    params2: vec4<f32>,
    engine:  vec4<f32>, // engine-written: x = horizon-shadow sun visibility
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0)
var<uniform> mat: ShaderParams;

fn parity(i: f32) -> bool {
    return (i - 2.0 * floor(i * 0.5)) > 0.5;
}

@fragment
fn fragment(input: VertexOutput, @builtin(front_facing) is_front: bool) -> @location(0) vec4<f32> {
    // Object-space surface direction. The model matrix' upper-3x3 columns are
    // the (possibly scaled) basis vectors; normalize them to get the pure
    // rotation R, then d = Rᵀ · n_world is the mesh-local normal. For a sphere
    // centred on its local origin this equals the surface point direction —
    // giving seam-free, fully-covering lat-long coordinates.
    let m = mesh_functions::get_world_from_local(input.instance_index);
    let R = mat3x3<f32>(normalize(m[0].xyz), normalize(m[1].xyz), normalize(m[2].xyz));
    let d = normalize(transpose(R) * normalize(input.world_normal));

    let lon = atan2(d.z, d.x);                 // -PI..PI  (around the equator)
    let lat = asin(clamp(d.y, -1.0, 1.0));     // -PI/2..PI/2 (pole to pole)
    let u = lon / TAU + 0.5;                    // 0..1
    let v = lat / PI + 0.5;                     // 0..1

    let wedge_count = select(mat.params.x, 8.0, mat.params.x < 0.5);
    let band_count  = select(mat.params.y, 6.0, mat.params.y < 0.5);
    let marker      = mat.params.w;            // 0 (unset) → no marker; checker covers all

    let wi = floor(u * wedge_count);
    let bi = floor(v * band_count);
    let checker = parity(wi) != parity(bi);    // XOR → checkerboard
    var color = select(mat.color_a, mat.color_b, checker);

    if (marker > 0.5 && wi < marker) { color = mat.color_c; }  // opt-in lead wedge
    // (no pole caps — the checkerboard runs all the way to the poles; a coloured
    //  cap read as a stray white dot on top.)

    // Full scene lighting (real sun direction, shadow maps, ambient) over
    // the procedural checker — matches the other prop shaders.
    var pbr_input = pbr_types::pbr_input_new();
    pbr_input.flags = mesh[input.instance_index].flags; // keep SHADOW_RECEIVER
    pbr_input.frag_coord = input.position;
    pbr_input.world_position = input.world_position;
    pbr_input.world_normal = pbr_functions::prepare_world_normal(
        normalize(input.world_normal), false, is_front);
    pbr_input.is_orthographic = view.clip_from_view[3].w == 1.0;
    pbr_input.N = pbr_input.world_normal;
    pbr_input.V = pbr_functions::calculate_view(input.world_position, pbr_input.is_orthographic);
    pbr_input.material.base_color = vec4(color.rgb, color.a);
    pbr_input.material.perceptual_roughness = 0.6;
    pbr_input.material.metallic = 0.0;
    pbr_input.material.reflectance = vec3(0.5);

    var out = pbr_functions::apply_pbr_lighting(pbr_input);
    // Smooth horizon-shadow terminator fade (engine-written visibility).
    out = vec4(out.rgb * mat.engine.x, out.a);
    out = pbr_functions::main_pass_post_lighting_processing(pbr_input, out);
    return out;
}
