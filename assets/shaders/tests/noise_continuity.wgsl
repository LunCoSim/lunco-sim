// GPU regression for the shared value/gradient noise at lattice boundaries.
// Green is the numerical contract; red identifies a discontinuity or a
// disagreement between the scalar and analytic-gradient implementations.
#import bevy_pbr::forward_io::VertexOutput
#import lunco::noise::{vnoise, vnoise_gradient, vnoise2d, vnoise2d_gradient}

struct Material {
    unused: f32,
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let cell = floor(in.uv * 64.0);
    let origin = vec3(-6800.0 + cell.x, 1234.0, -6800.0 + cell.y);
    let epsilon = 0.002;
    var valid = true;
    for (var axis = 0u; axis < 3u; axis++) {
        var p = origin + vec3(0.37, 0.43, 0.61);
        p[axis] = origin[axis];
        var delta = vec3(0.0);
        delta[axis] = epsilon;
        let left = vnoise_gradient(p - delta);
        let right = vnoise_gradient(p + delta);
        // The cubic interpolant has a continuous value and first derivative.
        // This tolerance includes f32 coordinate spacing at these magnitudes.
        valid = valid && abs(left.x - right.x) < 0.015
            && length(left.yzw - right.yzw) < 0.06
            && abs(vnoise(p) - vnoise_gradient(p).x) < 0.0001;
    }
    for (var axis = 0u; axis < 2u; axis++) {
        var p = origin.xz + vec2(0.37, 0.61);
        p[axis] = origin.xz[axis];
        var delta = vec2(0.0);
        delta[axis] = epsilon;
        let left = vnoise2d_gradient(p - delta);
        let right = vnoise2d_gradient(p + delta);
        valid = valid && abs(left.x - right.x) < 0.015
            && length(left.yz - right.yz) < 0.06
            && abs(vnoise2d(p) - vnoise2d_gradient(p).x) < 0.0001;
    }
    return select(vec4(1.0, 0.0, 0.0, 1.0), vec4(0.0, 1.0, 0.0, 1.0), valid);
}
