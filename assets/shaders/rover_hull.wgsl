//! Panelised spacecraft-bus hull for the general `ShaderMaterial`.
//!
//! Turns a plain collider box (or any hull mesh) into a believable rover body:
//! a panel grid with seams and rivets, an accent stripe, paint that chips to
//! bare metal with wear, and a regolith dust gradient that settles on the lower
//! hull. Everything is procedural in OBJECT space (mesh-fixed — it drives with
//! the rover), reflected live from the `//!@` annotations below.
//!
//! **The base paint is the prim's own `primvars:displayColor`.** `display_color`
//! is engine-filled from the standard USD attribute, so a rover authors its
//! livery exactly once, in the place every other tool already looks — and the
//! same authoring works whether the prim renders through plain PBR or through
//! this shader. Difficulty tiers and per-rover liveries are plain
//! `over "Chassis" { color3f[] primvars:displayColor = [(r,g,b)] }` overrides.
//! A `Shader` prim that authors `inputs:display_color` explicitly still wins.
//!
//! Prop-safe by construction: own `Material` struct, `lunco::pbr_lit` for full
//! scene lighting, and only prop-fillable engine inputs (`display_color`,
//! material inputs — so it appears in the prop shader picker and works on any mesh
//! with zero Rust.

#import bevy_pbr::{
    forward_io::VertexOutput,
    mesh_functions,
}
#import lunco::pbr_lit::lit
#import lunco::noise::fbm

//!@engine  display_color
//!@default display_color 0.25,0.26,0.28
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
//!@default wear          0.08
//!@ui      dust_height   0 1 "Dust fade height"
//!@default dust_height   0.35
//!@ui      dust_amount   0 1 "Dust coverage"
//!@default dust_amount   0.15
struct Material {
    // engine-filled: the prim's `primvars:displayColor` (element 0)
    display_color: vec3<f32>,
    panel_scale:   f32,
    accent_color:  vec3<f32>,
    panel_line:    f32,
    dust_color:    vec3<f32>,
    rivet_density: f32,
    wear:          f32,
    dust_height:   f32,
    dust_amount:   f32,
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

    // NORMALIZED object height, −0.5 … +0.5 regardless of how the hull is scaled.
    //
    // `p_local` is in METRES (R is the pure rotation; the scale stays in the
    // point), so any threshold written against it silently means something
    // different on every hull. The chassis is 0.3 m tall ⇒ `p_local.y` spans
    // only ±0.15, and the stripe/dust constants below — authored for a unit
    // cube — swallowed it whole: the accent band covered the entire TOP HALF
    // (so every rover's deck rendered the same accent colour no matter its
    // livery) and the dust gradient never reached its upper edge (so full dust
    // washed the whole hull grey). Dividing by the model matrix's per-axis
    // scale puts the height back in unit-cube terms, where those constants mean
    // what they say. The panel grid deliberately stays in metres — its
    // parameter is panels PER METRE.
    let obj_scale = vec3<f32>(length(m[0].xyz), length(m[1].xyz), length(m[2].xyz));
    let h = p_local.y / max(obj_scale.y, 1e-4);

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

    var color = mat.display_color * panel_tint;

    // --- Accent stripe: a band around the hull at mid-height ---
    // Narrow band on the hull SIDES, in normalized height — a stripe, not a
    // repaint of the upper hull.
    let stripe = smoothstep(0.10, 0.06, abs(h - 0.10));
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
    //
    // `wear` scales the chips' OPACITY as well as their extent. Bare metal is a
    // light neutral, so a chip mask that reaches full strength averages the
    // paint toward grey and destroys the livery — a red hull rendered pink and
    // a yellow one tan, with every rover in the scene converging on the same
    // washed non-colour. The vehicle's IDENTITY is its paint; wear is a detail
    // on top of it, so it can tint but never replace.
    let chip_n = fbm(p_local * 9.0, 3, 0.55);
    let chip_mask = smoothstep(1.0 - mat.wear * 0.6, 1.0, chip_n + seam * 0.35);
    let chip = chip_mask * clamp(mat.wear * 1.4, 0.0, 0.65);
    let bare_metal = vec3(0.62, 0.63, 0.66);
    color = mix(color, bare_metal, chip);

    // --- Seams darken last so chips can sit on their shoulders ---
    color = mix(color, color * 0.62, seam * (1.0 - chip));

    // --- Regolith dust: settles low on the hull, noise-broken ---
    let dust_fade = 1.0 - smoothstep(-0.5 + mat.dust_height * 0.5, 0.45, h);
    let dust_n = smoothstep(0.25, 0.7, fbm(p_local * 5.0, 3, 0.5));
    // Capped for the same reason as the chips: dust settles ON the paint, it is
    // not a repaint. Above ~0.55 coverage every hull reads as the same grey.
    let dust = clamp(mat.dust_amount * dust_fade * (0.4 + 0.6 * dust_n), 0.0, 0.55);
    color = mix(color, mat.dust_color, dust);

    // Dust is matte; chipped metal is glossy; paint sits between.
    let roughness = mix(mix(0.55, 0.3, chip), 0.95, dust);
    let metallic = mix(mix(0.05, 0.75, chip), 0.0, dust);

    return lit(input, is_front, color, roughness, metallic, vec3(0.0));
}
