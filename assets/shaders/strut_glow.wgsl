//! Landing-leg strut material for the general `ShaderMaterial`.
//!
//! Tints the strut from its cold, unloaded metal toward a hot colour by
//! `load_frac` — the strut's own opinion of how hard it is working, driven per
//! leg through `float inputs:load_frac.connect` on the bound gprim, straight off
//! the leg joint's `force` port. The RAMP is a look decision and lives here; the
//! NUMBER is a physics result and does not. Normalisation against the part's
//! rating rides on the WIRE (`lunco:factor:load_frac` on the sink), so re-rating
//! a strut is one number in the scene and no edit to this file.
//!
//! The heat also drives an emissive term, so a loaded strut reads on the night
//! side and in the lander's own shadow — where a base-colour shift alone would
//! be invisible.
//!
//! Dynamic, self-describing parameters: the engine reflects the `Material`
//! struct (field names → offsets) and the `//!@` annotations straight out of
//! this file. Edit live (hot-reload) or via the Inspector / `SetObjectProperty`.

#import bevy_pbr::forward_io::VertexOutput
#import lunco::pbr_lit::lit

//!@ui      base_color  color "Cold strut colour (unloaded)"
//!@default base_color  0.55,0.55,0.57
//!@ui      hot_color   color "Hot colour at full rated load"
//!@default hot_color   0.95,0.16,0.05
//!@ui      load_frac   0 1   "Load fraction (driven by the leg joint)"
//!@default load_frac   0.0
//!@ui      glow        0 8   "Emissive gain at full load"
//!@default glow        3.0
//!@ui      roughness   0 1   "Perceptual roughness"
//!@default roughness   0.45
//!@ui      metallic    0 1   "Metallic"
//!@default metallic    0.7
struct Material {
    base_color: vec3<f32>,
    load_frac:  f32,
    hot_color:  vec3<f32>,
    glow:       f32,
    roughness:  f32,
    metallic:   f32,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0)
var<uniform> mat: Material;

@fragment
fn fragment(input: VertexOutput, @builtin(front_facing) is_front: bool) -> @location(0) vec4<f32> {
    let load = clamp(mat.load_frac, 0.0, 1.0);

    // Albedo walks from cold metal to the hot stop linearly in load.
    let albedo = mix(mat.base_color, mat.hot_color, load);

    // Emission is deliberately NOT linear: squaring keeps a lightly-loaded strut
    // dark (a parked lander sits at its static settle and should not blaze) while
    // the impact transient still spikes visibly. A linear term made every resting
    // leg glow, which reads as a fault light rather than as load.
    let emissive = mat.hot_color * (mat.glow * load * load);

    return lit(input, is_front, albedo, mat.roughness, mat.metallic, emissive);
}
