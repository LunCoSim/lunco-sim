struct ImageSize { input_size: vec2<u32>, output_size: vec2<u32> }
@group(0) @binding(0) var<storage, read> image_input: array<vec4<f32>>;
@group(0) @binding(1) var<storage, read_write> image_output: array<vec4<f32>>;
@group(0) @binding(2) var<uniform> image_size: ImageSize;
@group(0) @binding(3) var<storage, read> image_parameters: array<f32>;
fn input_pixel(p: vec2<u32>) -> vec4<f32> {
    return image_input[p.y * image_size.input_size.x + p.x];
}
fn parameter(index: u32) -> f32 { return image_parameters[index]; }
@compute @workgroup_size(8, 8)
fn artifact_image_main(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id.xy >= image_size.output_size) { return; }
    let source_pixel = min(id.xy * image_size.input_size / image_size.output_size, image_size.input_size - vec2<u32>(1));
    image_output[id.y * image_size.output_size.x + id.x] = process_pixel(id.xy, image_size.output_size, input_pixel(source_pixel));
}
