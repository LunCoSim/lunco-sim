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
    for (var i = 0; i < 48; i++) {
        let p = p0 + dir * t;
        if (p.x < 0.0 || p.y < 0.0 || p.x > size.x || p.y > size.y) { break; }
        let h = hf_height(tex, p * to_grid, ri);
        // Penumbra width floored at ONE heightfield texel (keep in sync with
        // HeightField::sun_visibility): the physical width `t * tan_sun_r` collapses far
        // below a texel for near casters at grazing sun, quantizing visibility into a hard
        // 0/1 staircase. One texel, not two: the floor also sets how far above the ray an
        // occluder must stand to reach full umbra, and at two texels nothing ever got
        // properly dark.
        let width = max(t * tan_sun_r, texel);
        // How far the terrain rises ABOVE the ray, in penumbra widths — the only thing
        // that may darken a sample. `1 - rise` is fully lit while the ray is clear and
        // fades to 0 once the occluder stands a full width above it.
        //
        // NOT `(ray - h) / width`: that demanded the ray clear the terrain by a whole
        // penumbra width to read lit, so with the 2-texel floor dominating (it does
        // almost everywhere — the physical width is centimetres) even FLAT ground came
        // back ~32% lit. The floor softens the shadow EDGE; it must not dim open ground.
        let rise = (h - (h0 + slope * t)) / width;
        vis = min(vis, 1.0 - rise);
        if (vis <= 0.0) { return 0.0; }
        t = t * 1.18 + texel * 0.5;
        if (t > max_t) { break; }
    }
    // Linear penumbra — no terminal smoothstep (it re-steepens the band).
    return clamp(vis, 0.0, 1.0);
}

// Sun visibility resolved from the **pre-baked shadow cache** (a single
// `textureSampleLevel` lookup) when `use_cache > 0.5`, otherwise the live
// per-pixel ray-march above. The cache (`shadow_cache`, an `R8Unorm` texture)
// is baked on the CPU by `lunco-environment`'s horizon system using the SAME
// `HeightField::sun_visibility` algorithm, refreshed only when the sun's
// terrain-local direction moves past a small threshold — so the expensive
// 48-step march loop runs ~once per minutes-long sun increment instead of
// every pixel every frame.
//
// `textureSampleLevel` with an explicit LOD of 0 (the cache is single-mip) is
// permitted in non-uniform control flow, so this can be called from inside the
// distance-gated `march_blend` branch — unlike `textureSample`, which WebGPU
// restricts to uniform control flow. The `use_cache` guard is itself a uniform
// (`mat.shadow_cache_on`), so the whole branch is uniform-stable.
fn sun_visibility_resolved(
    cache: texture_2d<f32>,
    cache_samp: sampler,
    use_cache: f32,
    height_tex: texture_2d<f32>,
    uv: vec2<f32>,
    sun_local: vec3<f32>,
    tan_sun_r: f32,
    size: vec2<f32>,
    res: f32,
) -> f32 {
    if (use_cache > 0.5) {
        return textureSampleLevel(cache, cache_samp, uv, 0.0).r;
    }
    return sun_visibility(height_tex, uv, sun_local, tan_sun_r, size, res);
}

// Additive shadow fill applied by every consumer AFTER the march multiply:
// display-referred, shaped by the surface albedo so texture/relief survive.
// A multiplicative visibility floor CANNOT light a polar scene — at grazing
// sun even fully LIT flat ground shades to a few percent, so any fraction of
// it is black. This is the deliberate artistic stand-in for earthshine and
// scattered light; on sunlit ground it is a negligible +2%.
const SHADOW_FILL: f32 = 0.26;

// Weight for SHADOW_FILL: 1 in the DEM interior → 0 at the footprint edge.
// The celestial globe tiles the terrain merges into carry NO fill, so a
// uniform fill leaves the patch a visibly lighter square on the unlit globe
// from altitude. Fading it out over the same outer band the geometry
// feathers across (BodyCurvature, radial 0.6→1.0) makes the brightness
// converge with the surface. `uv` is the DEM-global footprint UV.
fn shadow_fill_weight(uv: vec2<f32>) -> f32 {
    let m = length(uv - vec2(0.5)) * 2.0; // 0 centre → 1 at inscribed-disc edge
    return 1.0 - smoothstep(0.6, 1.0, m);
}
