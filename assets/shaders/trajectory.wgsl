#import bevy_pbr::forward_io::{VertexOutput, FragmentOutput}

struct TrajectoryExtension {
    color: vec4<f32>,
    time: f32,
    pulse_pos: f32,
    pulse_width: f32,
    noise_scale: f32,
    emissive_mult: f32,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(100)
var<uniform> material: TrajectoryExtension;

@fragment
fn fragment(
    mesh: VertexOutput,
) -> FragmentOutput {
    // Solid high-intensity color
    let color_rgb = material.color.rgb * material.emissive_mult;
    
    // Use vertex color alpha for fading (managed by CPU system)
    let alpha = material.color.a * mesh.color.a;
    
    var out: FragmentOutput;
    // We output directly to avoid base_color/lighting interference.
    // Bloom and Tonemapping will still pick this up as it's part of the main pass.
    out.color = vec4<f32>(color_rgb, alpha);
    return out;
}
