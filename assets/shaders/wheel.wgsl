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
    mesh_types,
    mesh_view_bindings::view,
    mesh_functions,
}
#import lunco::noise::fbm

const TAU: f32 = 6.28318530718;

//!@ui      rim_color   color "Rim / spoke colour"
//!@default rim_color   0.12,0.13,0.14
//!@ui      tire_color  color "Tire colour"
//!@default tire_color  0.05,0.05,0.06
//!@ui      spoke_count 1 16   "Spoke count"
//!@default spoke_count 6
//!@ui      tread_lugs  4 48   "Tread lugs"
//!@default tread_lugs  24
//!@ui      spoke_width 0.05 0.9 "Spoke width (of sector)"
//!@default spoke_width 0.35
//!@ui      dust_color  color "Regolith dust"
//!@default dust_color  0.42,0.40,0.38
//!@ui      lug_depth   0 1 "Lug relief depth"
//!@default lug_depth   0.6
//!@ui      wear        0 1 "Tread wear"
//!@default wear        0.15
//!@ui      dust_amount 0 1 "Dust coverage"
//!@default dust_amount 0.35
struct Material {
    rim_color:   vec3<f32>,
    spoke_count: f32,
    tire_color:  vec3<f32>,
    tread_lugs:  f32,
    spoke_width: f32,
    dust_color:  vec3<f32>,
    lug_depth:   f32,
    wear:        f32,
    dust_amount: f32,
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
    var is_metal: f32 = 0.0;
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
            is_metal = 0.0;
        } else if (r > 0.60) {
            color = metal;                                // rim ring
            is_metal = 1.0;
        } else if (r > 0.22) {
            // Radial spokes — all bright metal. Rotation stays legible from
            // the tread lugs streaming past on the barrel.
            let s = fract(ang * spoke_count);
            let is_spoke = s < spoke_w;
            color = select(rubber, metal, is_spoke);
            is_metal = select(0.0, 1.0, is_spoke);
        } else {
            // Hubcap: bright metal centre.
            color = metal;
            is_metal = 1.0;
        }
    } else {
        // ---- Barrel: the rolling tread surface ----
        // Angle around the axle from the local normal (radial on the barrel),
        // so lugs are mesh-fixed and stream past as the wheel rolls.
        let ang = atan2(n_local.z, n_local.x) / TAU + 0.5;
        // Lug PROFILE instead of a hard on/off stripe: `wear` rounds and
        // flattens it toward bald (a worn tire's lug↔groove contrast fades),
        // `lug_depth` fakes relief — groove valleys pick up contact AO.
        let s = abs(fract(ang * lug_count) - 0.5) * 2.0;      // 0 valley … 1 lug top
        let edge = mix(0.25, 0.6, mat.wear);                   // wear rounds the shoulder
        let lug_m = smoothstep(0.5 - edge * 0.5, 0.5 + edge * 0.5, s)
            * (1.0 - mat.wear * 0.85);                         // wear flattens contrast
        color = mix(rubber, tread, lug_m);
        color *= 1.0 - mat.lug_depth * 0.35 * (1.0 - lug_m);   // valley AO
        is_metal = 0.0;
    }

    // Regolith dust coating — noise-masked in OBJECT space so it spins with
    // the wheel. Strongest where the tire touches soil (lug tops / lower
    // sidewall reads too fine-grained to distinguish here; a mesh-fixed patchy
    // coat sells it).
    if (mat.dust_amount > 0.0) {
        let p_local = transpose(R) * (input.world_position.xyz - m[3].xyz);
        let dust_n = fbm(p_local * 7.0, 3, 0.5);
        let dust_m = mat.dust_amount * smoothstep(0.30, 0.75, dust_n);
        color = mix(color, mat.dust_color, dust_m);
        is_metal = mix(is_metal, 0.0, dust_m);
    }

    // Delegate illumination and shadowing to Bevy's native PBR path. Custom
    // materials are still ordinary shadow receivers; enforce that bit because
    // this shader owns the PbrInput rather than Bevy's stock fragment entry.
    var pbr_input = pbr_types::pbr_input_new();
    pbr_input.flags = mesh[input.instance_index].flags | mesh_types::MESH_FLAGS_SHADOW_RECEIVER_BIT;
    pbr_input.frag_coord = input.position;
    pbr_input.world_position = input.world_position;
    pbr_input.world_normal = pbr_functions::prepare_world_normal(
        normalize(input.world_normal), false, is_front);
    pbr_input.is_orthographic = view.clip_from_view[3].w == 1.0;
    pbr_input.N = pbr_input.world_normal;
    pbr_input.V = pbr_functions::calculate_view(input.world_position, pbr_input.is_orthographic);
    pbr_input.material.base_color = vec4(color, 1.0);
    pbr_input.material.perceptual_roughness = mix(0.85, 0.35, is_metal);
    pbr_input.material.metallic = mix(0.0, 0.80, is_metal);
    pbr_input.material.reflectance = vec3(0.5);

    var out = pbr_functions::apply_pbr_lighting(pbr_input);
    out = pbr_functions::main_pass_post_lighting_processing(pbr_input, out);
    return out;
}
