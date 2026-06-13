// Heightfield sun-shadow ray-march, shared by every terrain shader
// (regolith.wgsl, terrain_shadow.wgsl) via naga_oil import. The CPU mirror
// (`HeightField::sun_visibility` in lunco-environment) implements the SAME
// algorithm so per-entity shading agrees with per-pixel terrain shading.
//
// Physically-scaled soft shadows: at each march step the occlusion factor is
//   (ray_height - terrain_height) / (distance * tan(sun_angular_radius))
// so the penumbra is razor-sharp next to the casting crest and widens
// linearly with caster distance — the real behaviour of sun shadows.

#define_import_path lunco::horizon

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

// Sun visibility 0..1 at a surface point.
//   uv        — planar [0,1]² over the heightfield footprint
//   sun_local — unit to-sun direction in terrain-local space
//   tan_sun_r — tan of the sun's angular RADIUS (≈0.0046 for Sol at 1 AU)
//   size      — heightfield world extent (metres)
//   res       — heightfield texel resolution (side length)
fn sun_visibility(
    tex: texture_2d<f32>,
    uv: vec2<f32>,
    sun_local: vec3<f32>,
    tan_sun_r: f32,
    size: vec2<f32>,
    res: f32,
) -> f32 {
    if (res < 2.0) { return 1.0; } // no heightfield bound
    let horiz = vec2(sun_local.x, sun_local.z);
    let hl = length(horiz);
    if (sun_local.y <= 0.0) { return 0.0; }     // sun below horizontal
    if (hl < 1e-4) { return 1.0; }              // sun at zenith
    let dir = horiz / hl;
    let slope = sun_local.y / hl;               // ray dh per metre travelled
    let ri = i32(res);
    let to_grid = (res - 1.0) / size;
    let p0 = uv * size;
    // Small lift against self-shadow acne on the start texel.
    let h0 = hf_height(tex, p0 * to_grid, ri) + 0.35;
    let texel = min(size.x, size.y) / (res - 1.0);
    let max_t = length(size) * 1.42;
    var vis = 1.0;
    var t = texel;
    for (var i = 0; i < 96; i++) {
        let p = p0 + dir * t;
        if (p.x < 0.0 || p.y < 0.0 || p.x > size.x || p.y > size.y) { break; }
        let h = hf_height(tex, p * to_grid, ri);
        let occ = (h0 + slope * t - h) / (t * tan_sun_r);
        vis = min(vis, occ);
        if (vis <= 0.0) { return 0.0; }
        t = t * 1.18 + texel * 0.5;
        if (t > max_t) { break; }
    }
    let v = clamp(vis, 0.0, 1.0);
    return v * v * (3.0 - 2.0 * v);
}
