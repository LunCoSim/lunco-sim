//! Wheel material for the general `ShaderMaterial`.
//!
//! Draws a wheel on a cylinder so its rotation is obvious: tire tread on the
//! rolling surface + bright rim + radial spokes + hubcap on the faces.
//!
//! ## Cap vs. barrel — handled in object space
//! A cylinder has two distinct surfaces that need different treatment:
//!   * the two circular **faces** (caps) → the wheel "disc": spokes, rim, hub;
//!   * the **barrel** (the rolling tread surface) → tire rubber + lugs.
//! Bevy's cylinder UVs only map the *caps* to a centred disc; the barrel UV is
//! (around, height), so a UV-polar pattern is correct on the faces but garbage
//! on the tread. We therefore recover the **mesh-local normal** from the model
//! matrix (Bevy cylinders run along local Y) and branch on it: |n.y|≈1 ⇒ a
//! face (use the cap UV disc), else ⇒ the barrel (tread from the local angle).
//! Object space is mesh-fixed, so everything spins with the wheel.
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

//!@ui      rim_color   color "Rim / spoke colour"
//!@default rim_color   0.98,0.98,1.0
//!@ui      tire_color  color "Tire colour"
//!@default tire_color  0.10,0.10,0.11
//!@ui      spoke_count 1 16   "Spoke count"
//!@default spoke_count 6
//!@ui      tread_lugs  4 48   "Tread lugs"
//!@default tread_lugs  24
//!@ui      spoke_width 0.05 0.9 "Spoke width (of sector)"
//!@default spoke_width 0.35
//!@engine  sun_vis
//!@default sun_vis     1
struct Material {
    rim_color:   vec3<f32>,
    spoke_count: f32,
    tire_color:  vec3<f32>,
    tread_lugs:  f32,
    spoke_width: f32,
    sun_vis:     f32,  // engine-filled: horizon-shadow sun visibility
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0)
var<uniform> mat: Material;

@fragment
fn fragment(input: VertexOutput, @builtin(front_facing) is_front: bool) -> @location(0) vec4<f32> {
    let spoke_count = mat.spoke_count;
    let lug_count   = mat.tread_lugs;
    let spoke_w     = mat.spoke_width;

    let rubber = mat.tire_color;                          // dark tire
    let metal  = mat.rim_color;                           // bright spoke/rim
    let tread  = mix(mat.tire_color, mat.rim_color, 0.22); // subtle lug highlight (reads as rubber)

    // Mesh-local normal: normalize the model matrix' basis columns to recover
    // the pure rotation R, then n_local = Rᵀ · n_world. A Bevy cylinder's axis
    // is local Y, so |n_local.y|≈1 on the circular faces and ≈0 on the barrel.
    let m = mesh_functions::get_world_from_local(input.instance_index);
    let R = mat3x3<f32>(normalize(m[0].xyz), normalize(m[1].xyz), normalize(m[2].xyz));
    let n_local = normalize(transpose(R) * normalize(input.world_normal));

    var color: vec3<f32>;
    if (abs(n_local.y) > 0.5) {
        // ---- Circular face: the wheel disc (UV-polar) ----
        // Bevy maps each cap to a UV disc centred at (0.5,0.5), radius 0.5.
        let c = input.uv - vec2<f32>(0.5, 0.5);
        let r = length(c) * 2.0;                          // 0 at hub, 1 at rim
        let ang = atan2(c.y, c.x) / TAU + 0.5;            // 0..1, mesh-fixed
        if (r > 0.74) {
            // Black tire ring — smooth sidewall (a tire's side has no tread).
            // The rolling tread lives on the barrel branch below.
            color = rubber;
        } else if (r > 0.60) {
            color = metal;                                // rim ring
        } else if (r > 0.22) {
            // Radial spokes — all bright metal. Rotation stays legible from
            // the tread lugs streaming past on the barrel.
            let s = fract(ang * spoke_count);
            let is_spoke = s < spoke_w;
            color = select(rubber, metal, is_spoke);
        } else {
            // Hubcap: bright metal centre.
            color = metal;
        }
    } else {
        // ---- Barrel: the rolling tread surface ----
        // Angle around the axle from the local normal (radial on the barrel),
        // so lugs are mesh-fixed and stream past as the wheel rolls.
        let ang = atan2(n_local.z, n_local.x) / TAU + 0.5;
        let lug = fract(ang * lug_count) < 0.5;
        color = select(rubber, tread, lug);
    }

    // Full scene lighting (real sun direction, shadow maps, ambient) over
    // the procedural albedo — so wheels go dark on the night side and when
    // the horizon system pulls the entity out of the sun's render layer.
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
    pbr_input.material.perceptual_roughness = 0.7;
    pbr_input.material.metallic = 0.0;
    pbr_input.material.reflectance = vec3(0.5);

    var out = pbr_functions::apply_pbr_lighting(pbr_input);
    // Smooth horizon-shadow terminator fade (engine-written visibility);
    // the layer swap that follows is binary, this eases the transition.
    out = vec4(out.rgb * mat.sun_vis, out.a);
    out = pbr_functions::main_pass_post_lighting_processing(pbr_input, out);
    return out;
}
