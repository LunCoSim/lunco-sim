//! Vector and angle math for scripts, in Rust.
//!
//! Scripts exchange directions and positions as `[x, y, z]` float arrays —
//! whatever `world_pos` and `world_forward` hand back. The operations on them
//! belong here rather than in the rhai prelude for three reasons:
//!
//! * **Correctness.** `acos` is a partial function, and a dot product of two
//!   unit vectors leaves its domain by an ulp whenever the vectors are nearly
//!   identical — the ordinary case of a body holding its heading. In rhai that
//!   surfaces as a NaN that every subsequent comparison reads as false, so a
//!   poisoned accumulator looks like a passing test. Here it is one `clamp` on
//!   `f64`, next to the operation that needs it, and an unmeasurable angle is
//!   returned as `()` instead of as a number.
//! * **Cost.** These run per body per tick. Interpreting `a[0]*b[0] + …` through
//!   rhai's dynamic dispatch to do what is one `glam` call is work the engine
//!   should not be doing at 60 Hz.
//! * **One implementation.** A helper written in the prelude gets copied into
//!   whichever script needs a variant, and the copies drift.
//!
//! Every function is TOTAL: degenerate input yields `()` ("not measurable"),
//! never a NaN. `()` is falsy in the script's own `== ()` idiom, so a caller
//! that forgets to check gets a visible type error rather than silent poison.

use bevy::math::DVec3;
use rhai::{Dynamic, Engine};

/// One numeric element of a script array.
fn scalar(d: &Dynamic) -> Option<f64> {
    d.as_float().ok().or_else(|| d.as_int().ok().map(|i| i as f64))
}

/// Read an `[x, y, z]` script value as a vector.
///
/// Takes `Dynamic`, not `Array`, and that is load-bearing: native functions are
/// dispatched on argument TYPE, while scripts use `()` as "no value" — the miss
/// return of `world_pos` and every helper built on it. Registered against
/// `Array` these would simply not resolve for a `()` argument, and the script
/// would die with "function not found" instead of propagating the miss.
///
/// `None` for anything that is not three finite numbers: a `()`, a wrong-length
/// array, a non-numeric element, or a non-finite component from a degenerate
/// orientation. Rejecting non-finite input HERE keeps every operation below
/// total.
fn to_vec3(d: &Dynamic) -> Option<DVec3> {
    let a = d.read_lock::<rhai::Array>()?;
    if a.len() != 3 {
        return None;
    }
    let mut out = [0.0f64; 3];
    for (slot, value) in out.iter_mut().zip(a.iter()) {
        *slot = scalar(value)?;
    }
    let v = DVec3::from_array(out);
    v.is_finite().then_some(v)
}

/// A vector as an `[x, y, z]` script array.
fn to_array(v: DVec3) -> Dynamic {
    Dynamic::from_array(vec![
        Dynamic::from_float(v.x),
        Dynamic::from_float(v.y),
        Dynamic::from_float(v.z),
    ])
}

/// Lift a binary vector op, returning `()` on degenerate input.
fn binary(f: impl Fn(DVec3, DVec3) -> DVec3 + Send + Sync + 'static)
    -> impl Fn(Dynamic, Dynamic) -> Dynamic + Send + Sync + 'static
{
    move |a, b| match (to_vec3(&a), to_vec3(&b)) {
        (Some(a), Some(b)) => to_array(f(a, b)),
        _ => Dynamic::UNIT,
    }
}

/// A direction vector's length is ~1; anything else means the orientation it
/// came from is degenerate and carries no heading.
fn is_direction(v: DVec3) -> bool {
    let l = v.length();
    (0.5..=2.0).contains(&l)
}

/// Register the math surface on a scripting engine.
pub fn register(engine: &mut Engine) {
    engine.register_fn("vadd", binary(|a, b| a + b));
    engine.register_fn("vsub", binary(|a, b| a - b));
    engine.register_fn("vcross", binary(DVec3::cross));

    engine.register_fn("vscale", |a: Dynamic, k: f64| match to_vec3(&a) {
        Some(v) => to_array(v * k),
        None => Dynamic::UNIT,
    });

    engine.register_fn("vlen", |a: Dynamic| match to_vec3(&a) {
        Some(v) => Dynamic::from_float(v.length()),
        None => Dynamic::UNIT,
    });

    engine.register_fn("vdot", |a: Dynamic, b: Dynamic| match (to_vec3(&a), to_vec3(&b)) {
        (Some(a), Some(b)) => Dynamic::from_float(a.dot(b)),
        _ => Dynamic::UNIT,
    });

    // A zero-length vector has no direction, so it is returned unchanged rather
    // than divided by zero — the script's own convention, kept.
    engine.register_fn("vnorm", |a: Dynamic| match to_vec3(&a) {
        Some(v) => to_array(v.normalize_or_zero()),
        None => Dynamic::UNIT,
    });

    // NaN cannot reach here (`to_vec3` rejects it), but `clamp` is also called
    // directly on script-computed scalars, so it is written to reject rather
    // than pass through: `x < lo || x > hi` is false for NaN and would return it
    // untouched.
    engine.register_fn("clamp", |x: f64, lo: f64, hi: f64| {
        if x.is_nan() { lo } else { x.clamp(lo, hi) }
    });

    // qrot(q, v) — rotate a local vector by an `[x, y, z, w]` quaternion into
    // world space. The prelude derives every world axis (up, right, …) from this
    // one operation plus `world_rotation`, so there is no per-axis host read.
    //
    // A non-unit quaternion is normalised rather than refused: a quaternion
    // arriving from an animation sample or an interpolated pose is unit only to
    // float tolerance, and rejecting it would make the axis helpers report "no
    // orientation" for a body that plainly has one.
    engine.register_fn("qrot", |q: Dynamic, v: Dynamic| {
        let Some(v) = to_vec3(&v) else { return Dynamic::UNIT };
        let Some(qa) = q.read_lock::<rhai::Array>() else { return Dynamic::UNIT };
        if qa.len() != 4 {
            return Dynamic::UNIT;
        }
        let mut c = [0.0f64; 4];
        for (slot, value) in c.iter_mut().zip(qa.iter()) {
            match scalar(value) {
                Some(f) => *slot = f,
                None => return Dynamic::UNIT,
            }
        }
        let quat = bevy::math::DQuat::from_xyzw(c[0], c[1], c[2], c[3]);
        if !quat.is_finite() || quat.length_squared() < 1e-12 {
            return Dynamic::UNIT;
        }
        to_array(quat.normalize() * v)
    });

    // Unsigned angle between two directions, in degrees, or `()` when there is
    // none to measure. `glam`'s `angle_between` does the domain clamp itself.
    engine.register_fn("angle_deg", |a: Dynamic, b: Dynamic| {
        match (to_vec3(&a), to_vec3(&b)) {
            (Some(a), Some(b)) if is_direction(a) && is_direction(b) => {
                Dynamic::from_float(a.angle_between(b).to_degrees())
            }
            _ => Dynamic::UNIT,
        }
    });

    // Signed heading change, positive to the RIGHT — matching the steering
    // convention in `prelude/nav.rhai`, where a positive steer yaws right.
    //
    // PER-TICK DELTAS only. The measure saturates at 180°, so a total swept
    // angle is accumulated from these rather than taken start-to-end: past half
    // a revolution a direct measure folds back and reads as a turn the other
    // way.
    engine.register_fn("yaw_delta_deg", |f0: Dynamic, f1: Dynamic| {
        match (to_vec3(&f0), to_vec3(&f1)) {
            (Some(f0), Some(f1)) if is_direction(f0) && is_direction(f1) => {
                let (a, b) = (f0.normalize(), f1.normalize());
                let mag = a.angle_between(b).to_degrees();
                Dynamic::from_float(if a.cross(b).y > 0.0 { -mag } else { mag })
            }
            _ => Dynamic::UNIT,
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Engine {
        let mut e = Engine::new();
        register(&mut e);
        e
    }

    fn eval(src: &str) -> Dynamic {
        engine().eval::<Dynamic>(src).expect("script should evaluate")
    }

    /// The case that poisoned a whole test run: a body holding its heading, so
    /// the dot product sits exactly on `acos`'s domain edge. It must read as
    /// zero rotation, not as NaN.
    #[test]
    fn identical_headings_measure_zero_not_nan() {
        let d = eval("yaw_delta_deg([0.0, 0.0, 1.0], [0.0, 0.0, 1.0])");
        let v = d.as_float().expect("a measurable angle");
        assert!(v.is_finite(), "identical headings must not produce NaN, got {v}");
        assert!(v.abs() < 1e-9, "identical headings are zero rotation, got {v}");
    }

    /// The sign is taken from the yaw-plane cross product, so reversing the turn
    /// reverses the sign and the magnitude is unchanged. Pinned against literal
    /// vectors because scripts ACCUMULATE these: a flipped sign would cancel a
    /// swept angle to nearly zero instead of failing loudly.
    ///
    /// Which absolute sign means "right" is a property of the engine's forward
    /// axis, not of this function, and is covered end-to-end by
    /// `six_independent_parity` (steer +0.6 yaws right and accumulates positive).
    #[test]
    fn yaw_sign_reverses_with_the_turn() {
        let ccw = eval("yaw_delta_deg([0.0, 0.0, 1.0], [1.0, 0.0, 0.0])")
            .as_float()
            .unwrap();
        let cw = eval("yaw_delta_deg([1.0, 0.0, 0.0], [0.0, 0.0, 1.0])")
            .as_float()
            .unwrap();
        assert!((ccw + 90.0).abs() < 1e-6, "expected -90, got {ccw}");
        assert!((cw - 90.0).abs() < 1e-6, "expected +90, got {cw}");
    }

    /// A degenerate orientation has no heading. `()` says so; a number would be
    /// believed.
    #[test]
    fn degenerate_direction_is_unmeasurable() {
        for src in [
            "yaw_delta_deg([0.0, 0.0, 0.0], [0.0, 0.0, 1.0])",
            "angle_deg([0.0, 0.0, 1.0], [0.0, 0.0, 0.0])",
            "vlen([1.0, 2.0])",
        ] {
            assert!(eval(src).is_unit(), "{src} should be unmeasurable");
        }
    }

    /// Scripts pass `()` for a value that could not be read, and these functions
    /// are dispatched on TYPE. A signature that accepts only arrays does not
    /// resolve for `()` and kills the script with "function not found" — so every
    /// entry point must accept it and propagate the miss.
    #[test]
    fn unit_argument_propagates_instead_of_failing_to_resolve() {
        for src in [
            "yaw_delta_deg((), [0.0, 0.0, 1.0])",
            "yaw_delta_deg([0.0, 0.0, 1.0], ())",
            "angle_deg((), ())",
            "vlen(())",
            "vdot((), [1.0, 0.0, 0.0])",
            "vsub((), [1.0, 0.0, 0.0])",
            "vadd([1.0, 0.0, 0.0], ())",
            "vcross((), ())",
            "vnorm(())",
            "vscale((), 2.0)",
            "qrot((), [1.0, 0.0, 0.0])",
        ] {
            assert!(eval(src).is_unit(), "{src} should return ()");
        }
    }

    /// `clamp` must reject NaN rather than return it — the whole reason the
    /// script-side version was unsafe.
    #[test]
    fn clamp_rejects_nan() {
        let v = eval("clamp(0.0/0.0, -1.0, 1.0)").as_float().unwrap();
        assert!(v.is_finite(), "clamp must not pass NaN through, got {v}");
    }
}
