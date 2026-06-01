//! Wheel material for the general `ShaderMaterial`.
//!
//! Draws a wheel on a cylinder so its rotation is obvious: tire tread + bright
//! rim + radial spokes + hubcap, with one marker spoke for direction.
//!
//! Bevy's `Cylinder` maps each circular cap to a UV disc centred at (0.5,0.5),
//! radius 0.5 — so polar coords from UV give a mesh-fixed wheel face that spins
//! with the wheel (the dominant side view on a rover). UV is mesh-fixed (unlike
//! world_position), so the pattern rotates with the geometry.
//!
//! ## Params
//!   param0 = spoke_count   (default 6)
//!   param1 = tread_lugs    (default 24)
//!   param2 = spoke_width   (0..1 of each sector, default 0.35)
//!   param3 = marker_spokes (default 1)
//!   color_a = spoke / rim (metal)   color_b = tire (rubber)   color_c = marker / hub
//!
//! Edit live (hot-reload) or tweak via `SetObjectProperty { property:"param0".. }`.

#import bevy_pbr::forward_io::VertexOutput

const TAU: f32 = 6.28318530718;

struct ShaderParams {
    color_a: vec4<f32>,
    color_b: vec4<f32>,
    color_c: vec4<f32>,
    params:  vec4<f32>,
    params2: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0)
var<uniform> mat: ShaderParams;

@fragment
fn fragment(input: VertexOutput) -> @location(0) vec4<f32> {
    // Polar coords from UV: r = 0 at hub centre, 1 at the rim (on a cap).
    let c = input.uv - vec2<f32>(0.5, 0.5);
    let r = length(c) * 2.0;
    let ang = atan2(c.y, c.x) / TAU + 0.5;   // 0..1, mesh-fixed

    let spoke_count = select(mat.params.x, 6.0,  mat.params.x < 0.5);
    let lug_count   = select(mat.params.y, 24.0, mat.params.y < 0.5);
    let spoke_w     = select(mat.params.z, 0.35, mat.params.z < 0.0001);
    let marker      = max(mat.params.w, 1.0);

    let rubber = mat.color_b;                              // dark tire
    let metal  = mat.color_a;                              // bright spoke/rim
    let tread  = mix(mat.color_b, mat.color_a, 0.35);      // lug highlight

    var color: vec4<f32>;
    if (r > 1.0) {
        color = rubber;                                    // barrel corners → tire
    } else if (r > 0.80) {
        // Tire tread: dark rubber with bright lugs around the circumference.
        let lug = fract(ang * lug_count) < 0.5;
        color = select(rubber, tread, lug);
    } else if (r > 0.70) {
        color = metal;                                     // bright rim ring
    } else if (r > 0.24) {
        // Radial spokes over a dark hub disc.
        let s = fract(ang * spoke_count);
        color = select(mat.color_b, metal, s < spoke_w);
        if (floor(ang * spoke_count) < marker) { color = mat.color_c; } // marker spoke
    } else {
        color = mat.color_c;                               // hubcap centre
    }

    // Mild normal-based shading so the form reads, without full PBR.
    let n = normalize(input.world_normal);
    let light_dir = normalize(vec3<f32>(0.4, 1.0, 0.6));
    let shade = 0.55 + 0.45 * clamp(dot(n, light_dir), 0.0, 1.0);
    return vec4<f32>(color.rgb * shade, color.a);
}
