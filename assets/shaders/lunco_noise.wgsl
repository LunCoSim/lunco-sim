// Shared procedural value noise for the terrain/prop shaders, via naga_oil
// import. Both dimensionalities use Bevy's integer PCG hash for lattice values.
// Floating-point arithmetic is reserved for interpolation between those values.
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

#import bevy_pbr::utils::rand_u

fn hash13(p: vec3<f32>) -> f32 {
    // Callers supply floor(p) plus integer corner offsets. Hash the integer
    // identity so neighbouring cells obtain exactly the same shared corner,
    // independent of floating-point contraction/reassociation. A float fract-dot
    // hash magnifies rounding at kilometre-scale grain coordinates into seams.
    let cell = bitcast<vec3<u32>>(vec3<i32>(p));
    var state = cell.x * 1597334677u ^ cell.y * 3812015801u ^ cell.z * 2798796415u;
    // The upper 24 bits convert exactly to f32 and keep the value below one.
    return f32(rand_u(&state) >> 8u) * (1.0 / 16777216.0);
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

/// Cubic value noise and its analytic gradient, packed as `(value, d/dx, d/dy, d/dz)`.
/// The derivatives use the same smoothstep interpolant as `vnoise`, so bump
/// mapping can shade the same field with one octave evaluation instead of
/// finite-differencing three complete FBM stacks.
fn vnoise_gradient(p: vec3<f32>) -> vec4<f32> {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let du = 6.0 * f * (1.0 - f);
    let n000 = hash13(i);
    let n100 = hash13(i + vec3(1.0, 0.0, 0.0));
    let n010 = hash13(i + vec3(0.0, 1.0, 0.0));
    let n110 = hash13(i + vec3(1.0, 1.0, 0.0));
    let n001 = hash13(i + vec3(0.0, 0.0, 1.0));
    let n101 = hash13(i + vec3(1.0, 0.0, 1.0));
    let n011 = hash13(i + vec3(0.0, 1.0, 1.0));
    let n111 = hash13(i + vec3(1.0, 1.0, 1.0));

    let x00 = mix(n000, n100, u.x);
    let x10 = mix(n010, n110, u.x);
    let x01 = mix(n001, n101, u.x);
    let x11 = mix(n011, n111, u.x);
    let y0 = mix(x00, x10, u.y);
    let y1 = mix(x01, x11, u.y);
    let value = mix(y0, y1, u.z);

    let dx0 = mix(n100 - n000, n110 - n010, u.y);
    let dx1 = mix(n101 - n001, n111 - n011, u.y);
    let dx = mix(dx0, dx1, u.z) * du.x;
    let dy0 = (x10 - x00) * du.y;
    let dy1 = (x11 - x01) * du.y;
    let dy = mix(dy0, dy1, u.z);
    let dz = (y1 - y0) * du.z;
    return vec4(value, dx, dy, dz);
}

/// Quintic value noise for smooth sky fields. Its value, first derivative and
/// second derivative are continuous at the lattice planes. Terrain and prop
/// layers use the cubic interpolant and its matching analytic gradient above.
fn vnoise_quintic(p: vec3<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
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

/// Terrain FBM rotates the domain about +Y by the golden angle each octave.
/// This decorrelates the axes of successive value-noise grids under grazing
/// illumination. Non-terrain `fbm` callers use the unrotated domain above.
fn fbm_rot(p: vec3<f32>, octaves: i32, gain: f32) -> f32 {
    var sum = 0.0;
    var amp = 1.0;
    var total = 0.0;
    var q = p;
    let rc = cos(2.399963);
    let rs = sin(2.399963);
    for (var o = 0; o < octaves; o++) {
        sum += amp * vnoise(q);
        total += amp;
        amp *= gain;
        q *= 2.0;
        q = vec3(rc * q.x - rs * q.z, q.y, rs * q.x + rc * q.z);
    }
    return sum / total;
}

/// Resolution weight for a noise period. `footprint` is noise units per pixel;
/// `filter_window` gives the cutoff and transition width in pixels per period.
fn noise_footprint_weight(footprint: f32, filter_window: vec2<f32>) -> f32 {
    return saturate((1.0 / max(footprint, 1e-6) - filter_window.x) / filter_window.y);
}

/// `fbm_rot` with its analytic gradient in the original input coordinate
/// system. Each octave applies a 2x scale and one fixed XZ rotation; the
/// accumulated inverse rotation and scale apply the chain rule without extra
/// noise samples. Each octave filters its own footprint. Faded values converge
/// to the noise mean without renormalizing the surviving octave amplitudes.
fn fbm_rot_gradient(
    p: vec3<f32>, octaves: i32, gain: f32, footprint: f32, filter_window: vec2<f32>,
) -> vec4<f32> {
    var sum = 0.0;
    var gradient = vec3<f32>(0.0);
    var amp = 1.0;
    var total = 0.0;
    var octave_scale = 1.0;
    var rot_c = 1.0;
    var rot_s = 0.0;
    var q = p;
    let rc = cos(2.399963);
    let rs = sin(2.399963);
    let count = max(octaves, 1);
    for (var o = 0; o < count; o++) {
        let weight = noise_footprint_weight(footprint * octave_scale, filter_window);
        var sample = vec4(0.5, 0.0, 0.0, 0.0);
        if (weight > 0.0) {
            sample = vnoise_gradient(q);
        }
        sum += amp * (0.5 + weight * (sample.x - 0.5));
        let local_gradient = sample.yzw;
        let input_gradient = vec3(
            rot_c * local_gradient.x + rot_s * local_gradient.z,
            local_gradient.y,
            -rot_s * local_gradient.x + rot_c * local_gradient.z,
        ) * octave_scale;
        gradient += amp * weight * input_gradient;
        total += amp;
        amp *= gain;
        octave_scale *= 2.0;
        q *= 2.0;
        q = vec3(rc * q.x - rs * q.z, q.y, rs * q.x + rc * q.z);
        let next_c = rc * rot_c - rs * rot_s;
        rot_s = rs * rot_c + rc * rot_s;
        rot_c = next_c;
    }
    return vec4(sum / total, gradient / total);
}

fn hash12(p: vec2<f32>) -> f32 {
    return hash13(vec3(p.x, 0.0, p.y));
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

/// Planar value noise and its analytic gradient, packed as `(value, d/dx, d/dy)`.
fn vnoise2d_gradient(p: vec2<f32>) -> vec3<f32> {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let du = 6.0 * f * (1.0 - f);
    let n00 = hash12(i);
    let n10 = hash12(i + vec2(1.0, 0.0));
    let n01 = hash12(i + vec2(0.0, 1.0));
    let n11 = hash12(i + vec2(1.0, 1.0));
    let x0 = mix(n00, n10, u.x);
    let x1 = mix(n01, n11, u.x);
    let value = mix(x0, x1, u.y);
    let dx = mix(n10 - n00, n11 - n01, u.y) * du.x;
    let dy = (x1 - x0) * du.y;
    return vec3(value, dx, dy);
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

/// `fbm2d` with the same octave-footprint and gradient contract as the 3D path.
fn fbm2d_gradient(
    p: vec2<f32>, octaves: i32, gain: f32, footprint: f32, filter_window: vec2<f32>,
) -> vec3<f32> {
    var sum = 0.0;
    var gradient = vec2<f32>(0.0);
    var amp = 1.0;
    var total = 0.0;
    var octave_scale = 1.0;
    var rot_c = 1.0;
    var rot_s = 0.0;
    var q = p;
    let rc = cos(2.399963);
    let rs = sin(2.399963);
    let count = max(octaves, 1);
    for (var o = 0; o < count; o++) {
        let weight = noise_footprint_weight(footprint * octave_scale, filter_window);
        var sample = vec3(0.5, 0.0, 0.0);
        if (weight > 0.0) {
            sample = vnoise2d_gradient(q);
        }
        sum += amp * (0.5 + weight * (sample.x - 0.5));
        let local_gradient = sample.yz;
        let input_gradient = vec2(
            rot_c * local_gradient.x + rot_s * local_gradient.y,
            -rot_s * local_gradient.x + rot_c * local_gradient.y,
        ) * octave_scale;
        gradient += amp * weight * input_gradient;
        total += amp;
        amp *= gain;
        octave_scale *= 2.0;
        q *= 2.0;
        q = vec2(rc * q.x - rs * q.y, rs * q.x + rc * q.y);
        let next_c = rc * rot_c - rs * rot_s;
        rot_s = rs * rot_c + rc * rot_s;
        rot_c = next_c;
    }
    return vec3(sum / total, gradient / total);
}
