import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput;
import bevy_pbr::render::utils::rand_u;

// Camera image interface. Depth is native opaque axial depth in metres;
// noise is an illustrative normalized image model, not calibrated hardware.
struct CameraImage {
    view_from_clip: mat4x4<f32>,
    viewport: vec4<f32>,
    range: vec4<f32>,
    noise: vec4<f32>,
    timing: vec4<u32>,
}
@group(0) @binding(0) var color_texture: texture_2d<f32>;
@group(0) @binding(1) var color_sampler: sampler;
@group(0) @binding(2) var<uniform> image: CameraImage;
@if(CAMERA_MULTISAMPLED) @group(0) @binding(3) var depth_texture: texture_depth_multisampled_2d;
@if(!CAMERA_MULTISAMPLED) @group(0) @binding(3) var depth_texture: texture_depth_2d;

fn random_value(pixel: vec2<f32>, stream: f32) -> f32 {
    let p=vec2<u32>(pixel);
    var state=(p.x*1597334677u) ^ (p.y*3812015801u) ^ image.timing.x
        ^ (image.timing.y*2798796415u) ^ bitcast<u32>(stream);
    return f32(rand_u(&state)>>8u)*(1.0/16777216.0);
}
fn gaussian(pixel: vec2<f32>, stream: f32) -> f32 {
    let a = max(random_value(pixel, stream), 0.0000001);
    let b = random_value(pixel, stream+31.0);
    return sqrt(-2.0*log(a))*cos(6.28318530718*b);
}
@fragment
fn fragment(input: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let dimensions = textureDimensions(color_texture);
    let pixel = min(vec2<u32>(input.uv*vec2<f32>(dimensions)), dimensions-vec2<u32>(1u));
    var color = textureSampleLevel(color_texture,color_sampler,input.uv,0.0).rgb;
    let view_uv=(input.position.xy-image.viewport.xy)/image.viewport.zw;
    if any(view_uv<vec2<f32>(0.0)) || any(view_uv>vec2<f32>(1.0)) {return vec4<f32>(color,1.0);}
    if image.range.z > 0.5 {
        let depth = textureLoad(depth_texture,vec2<i32>(pixel),0);
        let view = image.view_from_clip*vec4<f32>(view_uv*vec2<f32>(2.0,-2.0)+vec2<f32>(-1.0,1.0),depth,1.0);
        color = vec3<f32>(0.0);
        if depth > 0.0 && abs(view.w) > 0.00000001 {
            let metres = -view.z/view.w;
            let distance = clamp((metres-image.range.x)/(image.range.y-image.range.x),0.0,1.0);
            // A muted red/amber/green ramp preserves shape without neon clipping.
            let near_color = vec3<f32>(0.95,0.08,0.10);
            let middle_color = vec3<f32>(1.0,0.85,0.08);
            let far_color = vec3<f32>(0.08,0.75,0.35);
            color = select(mix(near_color,middle_color,distance*2.0),
                mix(middle_color,far_color,(distance-0.5)*2.0),distance>0.5);
        }
    }
    let p = vec2<f32>(pixel);
    let read = gaussian(p, 0.0)*image.noise.x;
    let shot = gaussian(p, 97.0)*image.noise.y*sqrt(max(color,vec3<f32>(0.0)));
    let row = gaussian(vec2<f32>(0.0,p.y),211.0)*image.noise.z;
    color = clamp(color+vec3<f32>(read+row)+shot,vec3<f32>(0.0),vec3<f32>(1.0));
    if random_value(p,373.0) < image.noise.w {color=vec3<f32>(0.0);}
    if image.timing.z > 1u {
        let levels = f32(image.timing.z-1u);
        color = round(color*levels)/levels;
    }
    return vec4<f32>(color,1.0);
}
