import bevy_pbr::render::view_transformations::position_world_to_view;
import bevy_pbr::render::{mesh_view_bindings::lights, mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT};
import super::solar_disc::disc_above_skyline;

// Bilinear heightfield fetch via textureLoad (R32Float is non-filterable in
// core WebGPU). `g` is in texel coordinates [0, res-1].
fn hf_height(tex: texture_2d<f32>, g: vec2<f32>, res: i32) -> f32 {
    let gc = clamp(g, vec2(0.0), vec2(f32(res - 1)));
    let i0 = vec2<i32>(floor(gc));
    let i1 = min(i0 + vec2(1, 1), vec2(res - 1, res - 1));
    let f = fract(gc);
    let h00 = textureLoad(tex, i0, 0).r;
    let h10 = textureLoad(tex, vec2(i1.x, i0.y), 0).r;
    let h01 = textureLoad(tex, vec2(i0.x, i1.y), 0).r;
    let h11 = textureLoad(tex, i1, 0).r;
    return mix(mix(h00, h10, f.x), mix(h01, h11, f.x), f.y);
}

// Site coordinates use a horizontal zero skyline. The globe supplies its curved skyline.
fn sun_visibility(tex: texture_2d<f32>, uv: vec2<f32>, sun: vec3<f32>, radius: f32,
    march: vec4<f32>, size: vec2<f32>, res: f32, elevation: f32) -> f32 {
    let horizontal = sun.xz;
    let length_xz = length(horizontal);
    if (length_xz == 0.0) { return disc_above_skyline(elevation, radius, 0.0); }
    let dir = horizontal / length_xz;
    let p0 = uv * size;
    let to_grid = (res - 1.0) / size;
    let receiver = hf_height(tex, p0 * to_grid, i32(res)) + march.y;
    let texel = min(size.x, size.y) / (res - 1.0);
    var t = texel;
    var skyline = 0.0;
    let upper = tan(min(1.5707962, elevation + radius));
    for (var i = 0u; i < u32(march.x); i++) {
        let p = p0 + dir * t;
        if (any(p < vec2(0.0)) || any(p > size)) { break; }
        skyline = max(skyline, (hf_height(tex, p * to_grid, i32(res)) - receiver) / t);
        if (skyline >= upper) { return 0.0; }
        t = t * march.z + texel * march.w;
    }
    return disc_above_skyline(elevation, radius, atan(skyline));
}

fn horizon_shadow_blend(view_depth: f32, csm_far: f32) -> f32 {
    if (csm_far <= 0.0) { return 1.0; }
    return smoothstep(csm_far * 0.9, csm_far, view_depth);
}

// A valid cache is nodal and spans both footprint endpoints.
fn sun_visibility_resolved(cache: texture_2d<f32>, cache_samp: sampler, use_cache: f32,
    height_tex: texture_2d<f32>, uv: vec2<f32>, sun: vec3<f32>, tan_radius: f32,
    march: vec4<f32>, size: vec2<f32>, res: f32, blend: f32) -> f32 {
    if (res < 2.0) { return 1.0; }
    let horizontal = length(sun.xz);
    // Clear day and full night need no trigonometry or terrain lookup nearby.
    if (sun.y <= -horizontal * tan_radius && sun.y <= 0.0) { return 0.0; }
    let whole_disc = sun.y >= horizontal * tan_radius;
    if (blend <= 0.0 && whole_disc) { return 1.0; }
    var sampled = 1.0;
    if (use_cache > 0.5 && blend > 0.0) {
        let dimensions = vec2<f32>(textureDimensions(cache));
        let cache_uv = (clamp(uv, vec2(0.0), vec2(1.0)) * (dimensions - 1.0) + 0.5) / dimensions;
        sampled = textureSampleLevel(cache, cache_samp, cache_uv, 0.0).r;
        if (whole_disc) { return mix(1.0, sampled, blend); }
    }
    let elevation = atan2(sun.y, horizontal);
    let radius = atan(tan_radius);
    let open = disc_above_skyline(elevation, radius, 0.0);
    if (blend <= 0.0) { return open; }
    if (use_cache > 0.5) {
        return open * mix(1.0, sampled, blend);
    }
    let terrain = sun_visibility(height_tex, uv, sun, radius, march, size, res, elevation);
    return mix(open, terrain, blend);
}

fn terrain_horizon_visibility(world_position: vec3<f32>, csm_far: f32,
    cache: texture_2d<f32>, cache_sampler: sampler, use_cache: f32,
    height_map: texture_2d<f32>, uv: vec2<f32>, sun: vec3<f32>, tan_radius: f32,
    march: vec4<f32>, size: vec2<f32>, res: f32) -> f32 {
    var far = 0.0;
    if (csm_far > 0.0 && lights.material_response_light_mask != 0u) {
        let light = &lights.directional_lights[firstTrailingBit(lights.material_response_light_mask)];
        if (((*light).flags & DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u && (*light).num_cascades > 0u) {
            far = (*light).cascades[(*light).num_cascades - 1u].far_bound;
        }
    }
    let depth = -position_world_to_view(world_position).z;
    return sun_visibility_resolved(cache, cache_sampler, use_cache, height_map, uv, sun,
        tan_radius, march, size, res, horizon_shadow_blend(depth, far));
}
