// A nodal shadow grid must interpolate at the CPU bake's endpoint coordinates.
#import bevy_pbr::forward_io::VertexOutput
#import lunco::horizon::sun_visibility_resolved
struct Material { unused: f32, }
@group(#{MATERIAL_BIND_GROUP}) @binding(6) var cache: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(7) var cache_sampler: sampler;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let dimensions = vec2<i32>(textureDimensions(cache));
    // Cover nodes, fractional samples, and both footprint borders on each axis.
    let probe = floor(fract(in.uv * 16.0) * 38.0);
    let uv = select(((probe - 5.0) / 32.0) * 1.5 - 0.25,
        (probe - 1.0) / vec2<f32>(dimensions - 1), probe < vec2(5.0));
    let g = clamp(uv, vec2(0.0), vec2(1.0)) * vec2<f32>(dimensions - 1);
    let lo = vec2<i32>(floor(g));
    let hi = min(lo + 1, dimensions - 1);
    let f = fract(g);
    let expected = mix(
        mix(textureLoad(cache, lo, 0).r, textureLoad(cache, vec2(hi.x, lo.y), 0).r, f.x),
        mix(textureLoad(cache, vec2(lo.x, hi.y), 0).r, textureLoad(cache, hi, 0).r, f.x), f.y);
    let actual = sun_visibility_resolved(cache, cache_sampler, 1.0, cache, uv,
        vec3(0.0, 1.0, 0.0), 0.0046, 48.0, vec2(100.0), 4.0);
    // Allow two 8-bit code values for hardware filtering precision.
    let valid = all(dimensions == vec2(4, 3)) && abs(actual - expected) < 2.0 / 255.0;
    return select(vec4(1.0, 0.0, 0.0, 1.0), vec4(0.0, 1.0, 0.0, 1.0), valid);
}
