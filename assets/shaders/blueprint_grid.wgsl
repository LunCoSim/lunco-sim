// Blueprint grid — a **self-describing, PBR-lit** ShaderMaterial.
//
// This is the dynamic-shader replacement for the bespoke
// `ExtendedMaterial<StandardMaterial, BlueprintExtension>` (blueprint.rs). It
// draws a Cartesian major/minor grid on the world XZ plane (the flat ground /
// blueprint floor), then lights it through bevy's full PBR via the shared
// `lunco::pbr_lit` mode — so it gets shadows/ambient/tonemapping like any lit
// surface, while its params are reflected straight from the `Material` struct
// below (no Rust struct, editable live in the Inspector).
//
// Author from USD with: primvars:materialType = "shader",
// primvars:shaderPath = "shaders/blueprint_grid.wgsl", and any of the params
// below as primvars (e.g. primvars:surface_color = (0.02, 0.08, 0.2)).

#import bevy_pbr::forward_io::VertexOutput
#import lunco::pbr_lit::lit

//!@ui      surface_color color           "Surface colour"
//!@default surface_color 0.02,0.08,0.22
//!@ui      line_color    color           "Grid line colour"
//!@default line_color    0.20,0.55,0.95
//!@ui      major_spacing 0.1 200          "Major spacing (m)"
//!@default major_spacing 10
//!@ui      minor_spacing 0.05 100         "Minor spacing (m)"
//!@default minor_spacing 1
//!@ui      major_width   0.5 8            "Major line width (px)"
//!@default major_width   2
//!@ui      minor_width   0.5 8            "Minor line width (px)"
//!@default minor_width   1
//!@ui      minor_fade    0 1              "Minor line strength"
//!@default minor_fade    0.5
//!@ui      roughness     0 1              "Roughness"
//!@default roughness     0.9
//!@ui      metallic      0 1              "Metallic"
//!@default metallic      0
struct Material {
    surface_color: vec3<f32>,
    line_color: vec3<f32>,
    major_spacing: f32,
    minor_spacing: f32,
    major_width: f32,
    minor_width: f32,
    minor_fade: f32,
    roughness: f32,
    metallic: f32,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0)
var<uniform> mat: Material;

// Pixel-width anti-aliased grid line mask for spacing `spacing`, line `width`
// in pixels, given world-space position `pos` and its per-pixel derivative.
fn grid_mask(pos: vec2<f32>, world_per_px: vec2<f32>, spacing: f32, width: f32) -> f32 {
    let dist = vec2<f32>(
        abs(fract(pos.x / spacing - 0.5) - 0.5) * spacing,
        abs(fract(pos.y / spacing - 0.5) - 0.5) * spacing,
    );
    let px = min(dist.x / max(world_per_px.x, 1e-6), dist.y / max(world_per_px.y, 1e-6));
    return 1.0 - smoothstep(0.0, width, px);
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> @location(0) vec4<f32> {
    let pos = in.world_position.xz;
    let world_per_px = abs(fwidth(pos));

    let major_m = grid_mask(pos, world_per_px, mat.major_spacing, mat.major_width);
    let minor_m = grid_mask(pos, world_per_px, mat.minor_spacing, mat.minor_width)
        * mat.minor_fade * (1.0 - major_m);
    let mask = max(major_m, minor_m);

    let albedo = mix(mat.surface_color, mat.line_color, mask);
    return lit(in, is_front, albedo, mat.roughness, mat.metallic, vec3<f32>(0.0));
}
