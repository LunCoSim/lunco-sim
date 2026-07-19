//! Panelised spacecraft-bus hull for the general `ShaderMaterial`.
//!
//! Turns a plain collider box (or any hull mesh) into a believable rover body:
//! a panel grid with seams and rivets, an accent stripe, paint that chips to
//! bare metal with wear, and a regolith dust gradient that settles on the lower
//! hull. Everything is procedural in OBJECT space (mesh-fixed — it drives with
//! the rover), reflected live from the `//!@` annotations below: difficulty
//! tiers restyle a rover by overriding `inputs:hull_color`/`inputs:accent_color`
//! instead of `displayColor`.
//!
//! Prop-safe by construction: own `Material` struct, `lunco::pbr_lit` for full
//! scene lighting, and no engine-filled inputs beyond `sun_vis` — so it appears
//! in the prop shader picker and works on any mesh with zero Rust.

#import bevy_pbr::{
    forward_io::VertexOutput,
    mesh_functions,
}
#import lunco::pbr_lit::lit
#import lunco::noise::fbm

const HORIZON_AMBIENT_FLOOR: f32 = 0.22;

//!@ui      hull_color    color "Hull paint"
//!@default hull_color    0.78,0.78,0.80
//!@ui      accent_color  color "Accent stripe"
//!@default accent_color  0.85,0.45,0.10
//!@ui      dust_color    color "Regolith dust"
//!@default dust_color    0.42,0.40,0.38
//!@ui      panel_scale   0.5 8 "Panel grid (per m)"
//!@default panel_scale   2.0
//!@ui      panel_line    0 0.2 "Panel seam width"
//!@default panel_line    0.02
//!@ui      rivet_density 0 1 "Rivet/bolt detail"
//!@default rivet_density 0.5
//!@ui      wear          0 1 "Paint wear"
//!@default wear          0.2
//!@ui      dust_height   0 1 "Dust fade height"
//!@default dust_height   0.35
//!@ui      dust_amount   0 1 "Dust coverage"
//!@default dust_amount   0.4
//!@engine  sun_vis
//!@default sun_vis       1
struct Material {
    hull_color:    vec3<f32>,
    panel_scale:   f32,
    accent_color:  vec3<f32>,
    panel_line:    f32,
    dust_color:    vec3<f32>,
    rivet_density: f32,
    wear:          f32,
    dust_height:   f32,
    dust_amount:   f32,
    sun_vis:       f32,  // engine-filled: horizon-shadow sun visibility
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0)
var<uniform> mat: Material;

@fragment
fn fragment(input: VertexOutput, @builtin(front_facing) is_front: bool) -> @location(0) vec4<f32> {
    // Mesh-local frame (see wheel.wgsl): normalize the model basis to recover
    // the rotation, then take position + normal into object space so the panel
    // grid is stamped on the hull, not on the world.
    let m = mesh_functions::get_world_from_local(input.instance_index);
    let R = mat3x3<f32>(normalize(m[0].xyz), normalize(m[1].xyz), normalize(m[2].xyz));
    let n_local = normalize(transpose(R) * normalize(input.world_normal));
    let p_local = transpose(R) * (input.world_position.xyz - m[3].xyz);

    // Planar panel coordinates: project onto the face plane by dropping the
    // dominant normal axis (box-projection — correct on any box-ish hull,
    // acceptable everywhere else).
    let an = abs(n_local);
    var pc: vec2<f32>;
    if (an.y >= an.x && an.y >= an.z) {
        pc = p_local.xz;
    } else if (an.x >= an.z) {
        pc = p_local.zy;
    } else {
        pc = p_local.xy;
    }

    // --- Panel grid ---
    let g = pc * mat.panel_scale;
    let cell = floor(g);
    let f = fract(g);
    // Distance to the nearest seam, in grid units.
    let seam_d = min(min(f.x, 1.0 - f.x), min(f.y, 1.0 - f.y));
    let seam = 1.0 - smoothstep(mat.panel_line * 0.5, mat.panel_line * 1.5, seam_d);
    // Slight per-panel value variation sells "assembled from parts".
    let panel_tint = 0.92 + 0.08 * fbm(vec3(cell * 0.7, 0.0), 2, 0.5);

    var color = mat.hull_color * panel_tint;

    // --- Accent stripe: a band around the hull at mid-height ---
    let stripe = smoothstep(0.16, 0.14, abs(p_local.y - 0.06));
    color = mix(color, mat.accent_color, stripe * 0.9);

    // --- Rivets: a bolt at each panel corner, inset along both axes ---
    if (mat.rivet_density > 0.0) {
        let corner = abs(f - vec2(0.5, 0.5));            // 0.5 at corners
        let inset = vec2(0.42, 0.42);
        let rd = length(corner - inset);
        let rivet = 1.0 - smoothstep(0.015, 0.03, rd);
        color = mix(color, color * 0.55, rivet * mat.rivet_density);
    }

    // --- Paint wear: noise-chipped near seams and edges → bare metal ---
    let chip_n = fbm(p_local * 9.0, 3, 0.55);
    let chip = smoothstep(1.0 - mat.wear * 0.6, 1.0, chip_n + seam * 0.35);
    let bare_metal = vec3(0.62, 0.63, 0.66);
    color = mix(color, bare_metal, chip);

    // --- Seams darken last so chips can sit on their shoulders ---
    color = mix(color, color * 0.62, seam * (1.0 - chip));

    // --- Regolith dust: settles low on the hull, noise-broken ---
    let dust_fade = 1.0 - smoothstep(-0.2 + mat.dust_height * 0.7, 0.45, p_local.y);
    let dust_n = smoothstep(0.25, 0.7, fbm(p_local * 5.0, 3, 0.5));
    let dust = clamp(mat.dust_amount * dust_fade * (0.4 + 0.6 * dust_n), 0.0, 1.0);
    color = mix(color, mat.dust_color, dust);

    // Dust is matte; chipped metal is glossy; paint sits between.
    let roughness = mix(mix(0.55, 0.3, chip), 0.95, dust);
    let metallic = mix(mix(0.05, 0.75, chip), 0.0, dust);

    var out = lit(input, is_front, color, roughness, metallic, vec3(0.0));
    // Horizon-shadow terminator fade, floored like wheel.wgsl — a hull in
    // grazing shadow is dim, never a black hole.
    let vis = max(mat.sun_vis, HORIZON_AMBIENT_FLOOR);
    return vec4(out.rgb * vis, out.a);
}
