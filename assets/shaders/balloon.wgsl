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
//! Dynamic, self-describing parameters: the engine reflects the `Material`
//! struct (field names → offsets) and the `//!@` annotations straight out of
//! this file. Edit live (hot-reload) or via the Inspector / `SetObjectProperty`.

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

//!@ui      cell_a        color "Cell colour A"
//!@default cell_a        0.2,0.8,0.3
//!@ui      cell_b        color "Cell colour B"
//!@default cell_b        0.12,0.12,0.14
//!@ui      marker_color  color "Marker wedge"
//!@default marker_color  1.0,1.0,1.0
//!@ui      wedge_count   2 24  "Longitude cells"
//!@default wedge_count   8
//!@ui      band_count    2 16  "Latitude cells"
//!@default band_count    6
//!@ui      marker_wedges 0 8   "Lead marker wedges"
//!@default marker_wedges 0
struct Material {
    cell_a:        vec3<f32>,
    wedge_count:   f32,
    cell_b:        vec3<f32>,
    band_count:    f32,
    marker_color:  vec3<f32>,
    marker_wedges: f32,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0)
var<uniform> mat: Material;

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

    let wedge_count = mat.wedge_count;
    let band_count  = mat.band_count;
    let marker      = mat.marker_wedges;

    let wi = floor(u * wedge_count);
    let bi = floor(v * band_count);
    let checker = parity(wi) != parity(bi);    // XOR → checkerboard
    var color = select(mat.cell_a, mat.cell_b, checker);

    if (marker > 0.5 && wi < marker) { color = mat.marker_color; }  // opt-in lead wedge
    // (no pole caps — the checkerboard runs all the way to the poles.)

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
    pbr_input.material.base_color = vec4(color, 1.0);
    pbr_input.material.perceptual_roughness = 0.6;
    pbr_input.material.metallic = 0.0;
    pbr_input.material.reflectance = vec3(0.5);

    var out = pbr_functions::apply_pbr_lighting(pbr_input);
    out = pbr_functions::main_pass_post_lighting_processing(pbr_input, out);
    return out;
}
