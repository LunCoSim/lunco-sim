// Shared procedural value noise for the terrain/prop shaders, via naga_oil
// import. Two families, one hash lineage (fract-dot arithmetic hashes — no
// `sin`, so results don't drift across driver polynomial approximations):
//
//   * 3D (`hash13`/`vnoise`/`fbm`) — world-space noise for the native
//     shaders; sampled on a direction vector it is a function on the sphere
//     with no parameterisation, hence no seam.
//   * 2D (`hash12`/`vnoise2d`/`fbm2d`) — the cheaper planar variant for the
//     web shaders. `fbm2d` rotates the domain by the golden angle (≈2.4 rad)
//     each octave: value noise is axis-aligned, so un-rotated octaves stack
//     into diagonal grid streaks that read as static under grazing lunar
//     light; the per-octave rotation decorrelates them into isotropic grain.

#define_import_path lunco::noise

fn hash13(p: vec3<f32>) -> f32 {
    var p3 = fract(p * 0.1031);
    p3 += dot(p3, p3.zyx + 31.32);
    return fract((p3.x + p3.y) * p3.z);
}

fn vnoise(p: vec3<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let n000 = hash13(i);
    let n100 = hash13(i + vec3(1.0, 0.0, 0.0));
    let n010 = hash13(i + vec3(0.0, 1.0, 0.0));
    let n110 = hash13(i + vec3(1.0, 1.0, 0.0));
    let n001 = hash13(i + vec3(0.0, 0.0, 1.0));
    let n101 = hash13(i + vec3(1.0, 0.0, 1.0));
    let n011 = hash13(i + vec3(0.0, 1.0, 1.0));
    let n111 = hash13(i + vec3(1.0, 1.0, 1.0));
    return mix(
        mix(mix(n000, n100, u.x), mix(n010, n110, u.x), u.y),
        mix(mix(n001, n101, u.x), mix(n011, n111, u.x), u.y),
        u.z,
    );
}

// Normalized to ~0..1 regardless of octave count (matches Blender's 0.5-centred
// noise output, which the terrain ramps were authored against).
fn fbm(p: vec3<f32>, octaves: i32, gain: f32) -> f32 {
    var sum = 0.0;
    var amp = 1.0;
    var total = 0.0;
    var q = p;
    for (var o = 0; o < octaves; o++) {
        sum += amp * vnoise(q);
        total += amp;
        amp *= gain;
        q *= 2.0; // lacunarity 2.0, as authored
    }
    return sum / total;
}

fn hash12(p: vec2<f32>) -> f32 {
    var p3 = fract(vec3(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 31.32);
    return fract((p3.x + p3.y) * p3.z);
}

fn vnoise2d(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let n00 = hash12(i);
    let n10 = hash12(i + vec2(1.0, 0.0));
    let n01 = hash12(i + vec2(0.0, 1.0));
    let n11 = hash12(i + vec2(1.0, 1.0));
    return mix(mix(n00, n10, u.x), mix(n01, n11, u.x), u.y);
}

fn fbm2d(p: vec2<f32>, octaves: i32, gain: f32) -> f32 {
    var sum = 0.0;
    var amp = 1.0;
    var total = 0.0;
    var q = p;
    let rc = cos(2.399963);
    let rs = sin(2.399963);
    for (var o = 0; o < octaves; o++) {
        sum += amp * vnoise2d(q);
        total += amp;
        amp *= gain;
        q *= 2.0;
        q = vec2(rc * q.x - rs * q.y, rs * q.x + rc * q.y);
    }
    return sum / total;
}
