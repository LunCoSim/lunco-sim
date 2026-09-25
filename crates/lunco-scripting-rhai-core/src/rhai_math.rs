//! Vector and angle math for scripts, in Rust.
//!
//! The engine supports two deliberate representations: native `DVec3`/`DQuat`
//! values for hot loops, and `[x, y, z]`/`[x, y, z, w]` arrays at JSON/USD/
//! telemetry boundaries and for existing scenarios. Both routes enter this
//! module, so the math is implemented once in glam rather than reimplemented in
//! Rhai. The operations belong here rather than in the Rhai prelude for three
//! reasons:
//!
//! * **Correctness.** `acos` is a partial function, and a dot product of two
//!   unit vectors leaves its domain by an ulp whenever the vectors are nearly
//!   identical — the ordinary case of a body holding its heading. In rhai that
//!   surfaces as a NaN that every subsequent comparison reads as false, so a
//!   poisoned accumulator looks like a passing test. Here it is one `clamp` on
//!   `f64`, next to the operation that needs it, and an unmeasurable angle is
//!   returned as `()` instead of as a number.
//! * **Cost.** These run per body per tick. Interpreting `a[0]*b[0] + …` through
//!   Rhai's dynamic dispatch to do what is one `glam` call is work the engine
//!   should not be doing at 60 Hz; native values avoid that array path entirely.
//! * **One implementation.** A helper written in the prelude gets copied into
//!   whichever script needs a variant, and the copies drift.
//!
//! Array-facing functions retain the scenario contract: degenerate input yields
//! `()` ("not measurable"), never a NaN. Native constructors and operators are
//! fallible and raise a script error for invalid values, so a caller cannot
//! accidentally carry a malformed pose into the simulator. `()` is falsy in
//! the script's own `== ()` idiom, so a caller that forgets to check gets a
//! visible type error rather than silent poison.

use bevy::math::{DQuat, DVec2, DVec3, EulerRot};
pub use lunco_core::DTransform;
use lunco_core::DTransform as CoreDTransform;
use rhai::{Dynamic, Engine, EvalAltResult, Position};

/// Construct a Rhai runtime error for a value that cannot satisfy the native
/// math type's invariant.  Native vectors/quaternions are deliberately
/// fallible at this boundary: an invalid pose must stop the authored program,
/// not become a string, `null`, or a NaN that makes later checks meaningless.
fn invalid_value(message: impl Into<String>) -> Box<EvalAltResult> {
    EvalAltResult::ErrorRuntime(message.into().into(), Position::NONE).into()
}

fn finite_vec3(value: DVec3, label: &str) -> Result<DVec3, Box<EvalAltResult>> {
    value
        .is_finite()
        .then_some(value)
        .ok_or_else(|| invalid_value(format!("{label} must contain only finite values")))
}

fn finite_vec2(value: DVec2, label: &str) -> Result<DVec2, Box<EvalAltResult>> {
    value
        .is_finite()
        .then_some(value)
        .ok_or_else(|| invalid_value(format!("{label} must contain only finite values")))
}

fn finite_quat(value: DQuat, label: &str) -> Result<DQuat, Box<EvalAltResult>> {
    if !value.is_finite() {
        return Err(invalid_value(format!(
            "{label} must contain only finite values"
        )));
    }
    let length_squared = value.length_squared();
    if !length_squared.is_finite() || length_squared == 0.0 {
        return Err(invalid_value(format!(
            "{label} must have a finite, non-zero magnitude"
        )));
    }
    let normalized = value.normalize();
    if !normalized.is_finite() {
        return Err(invalid_value(format!(
            "{label} must normalize to a finite rotation"
        )));
    }
    Ok(normalized)
}

fn finite_transform(
    translation: DVec3,
    rotation: DQuat,
    scale: DVec3,
    label: &str,
) -> Result<DTransform, Box<EvalAltResult>> {
    CoreDTransform::new(
        finite_vec3(translation, &format!("{label} translation"))?,
        finite_quat(rotation, &format!("{label} rotation"))?,
        finite_vec3(scale, &format!("{label} scale"))?,
    )
    .ok_or_else(|| invalid_value(format!("{label} is not a finite transform")))
}

/// One numeric element of a script array.
fn scalar(d: &Dynamic) -> Option<f64> {
    f64_from_dynamic(d.clone()).as_float().ok()
}

/// Canonicalize the two scalar inputs accepted at a Rhai numeric boundary to
/// the engine's native f64.  This is deliberately a Rust boundary function so
/// scripts do not need string-based type protocols or repeated dynamic method
/// dispatch in numerical policies.
fn f64_from_dynamic(value: Dynamic) -> Dynamic {
    if let Ok(value) = value.as_float() {
        return value
            .is_finite()
            .then_some(Dynamic::from_float(value))
            .unwrap_or(Dynamic::UNIT);
    }
    if let Ok(integer) = value.as_int() {
        let value = integer as f64;
        if value.is_finite() && value as i128 == integer as i128 {
            return Dynamic::from_float(value);
        }
    }
    Dynamic::UNIT
}

/// Accept only a native Rhai f64.  Settings use this stricter boundary so an
/// integer-valued TOML setting cannot silently become an engineering margin.
fn f64_only_dynamic(value: Dynamic) -> Dynamic {
    match value.as_float() {
        Ok(value) if value.is_finite() => Dynamic::from_float(value),
        _ => Dynamic::UNIT,
    }
}

fn array_is_dynamic(value: Dynamic) -> bool {
    value.is_array()
}

fn map_is_dynamic(value: Dynamic) -> bool {
    value.is_map()
}

fn string_is_dynamic(value: Dynamic) -> bool {
    value.is_string()
}

/// Read a native `Vec3` or `[x, y, z]` script value as a vector.
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
pub fn to_vec3(d: &Dynamic) -> Option<DVec3> {
    if let Some(v) = d.clone().try_cast::<DVec3>() {
        return v.is_finite().then_some(v);
    }
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

/// Read a native `Vec2` or `[x, y]` script value as a vector.
pub fn to_vec2(d: &Dynamic) -> Option<DVec2> {
    if let Some(v) = d.clone().try_cast::<DVec2>() {
        return v.is_finite().then_some(v);
    }
    let a = d.read_lock::<rhai::Array>()?;
    if a.len() != 2 {
        return None;
    }
    let mut out = [0.0f64; 2];
    for (slot, value) in out.iter_mut().zip(a.iter()) {
        *slot = scalar(value)?;
    }
    let v = DVec2::from_array(out);
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

fn vec2_to_array(v: DVec2) -> Dynamic {
    Dynamic::from_array(vec![Dynamic::from_float(v.x), Dynamic::from_float(v.y)])
}

/// Put the engine's native vector into a Rhai value without an intermediate
/// array.  Arrays remain the wire/telemetry representation; this is the
/// explicit lowering used by `world_pos3`, `vec3_array`, and telemetry.
pub fn to_native(v: DVec3) -> Dynamic {
    Dynamic::from(v)
}

fn quat_to_array(q: DQuat) -> Dynamic {
    Dynamic::from_array(vec![
        Dynamic::from_float(q.x),
        Dynamic::from_float(q.y),
        Dynamic::from_float(q.z),
        Dynamic::from_float(q.w),
    ])
}

fn quat_from_array(d: &Dynamic) -> Option<DQuat> {
    let a = d.read_lock::<rhai::Array>()?;
    if a.len() != 4 {
        return None;
    }
    let mut c = [0.0f64; 4];
    for (slot, value) in c.iter_mut().zip(a.iter()) {
        *slot = scalar(value)?;
    }
    let q = DQuat::from_xyzw(c[0], c[1], c[2], c[3]);
    let length_squared = q.length_squared();
    if !q.is_finite() || !length_squared.is_finite() || length_squared == 0.0 {
        return None;
    }
    let normalized = q.normalize();
    normalized.is_finite().then_some(normalized)
}

fn to_quat(d: &Dynamic) -> Option<DQuat> {
    if let Some(q) = d.clone().try_cast::<DQuat>() {
        let length_squared = q.length_squared();
        if !q.is_finite() || !length_squared.is_finite() || length_squared == 0.0 {
            return None;
        }
        let normalized = q.normalize();
        return normalized.is_finite().then_some(normalized);
    }
    quat_from_array(d)
}

fn native_add(a: DVec3, b: DVec3) -> Result<DVec3, Box<EvalAltResult>> {
    finite_vec3(a + b, "vector sum")
}

fn native_sub(a: DVec3, b: DVec3) -> Result<DVec3, Box<EvalAltResult>> {
    finite_vec3(a - b, "vector difference")
}

fn native_scale(a: DVec3, scalar: f64) -> Result<DVec3, Box<EvalAltResult>> {
    if !scalar.is_finite() {
        return Err(invalid_value("vector scale must be finite"));
    }
    finite_vec3(a * scalar, "scaled vector")
}

fn native_cross(a: DVec3, b: DVec3) -> Result<DVec3, Box<EvalAltResult>> {
    finite_vec3(a.cross(b), "vector cross product")
}

fn native_dot(a: DVec3, b: DVec3) -> Result<f64, Box<EvalAltResult>> {
    let value = a.dot(b);
    value
        .is_finite()
        .then_some(value)
        .ok_or_else(|| invalid_value("vector dot product must be finite"))
}

/// Whether a vector has a measurable direction.  This is a representation
/// invariant, not a requirement tolerance: finite and non-zero is enough for
/// the native f64 operations below to define a direction.
fn native_vec3_is_valid(value: DVec3) -> bool {
    value.is_finite() && value.length_squared().is_finite() && value.length_squared() > 0.0
}

fn native_vec3_is_native(_: DVec3) -> bool {
    true
}

fn native_vec3_component(value: DVec3, index: i64) -> Result<f64, Box<EvalAltResult>> {
    match index {
        0 => Ok(value.x),
        1 => Ok(value.y),
        2 => Ok(value.z),
        _ => Err(invalid_value("Vec3 component index must be 0, 1, or 2")),
    }
}

fn native_quat_is_valid(value: DQuat) -> bool {
    let length_squared = value.length_squared();
    value.is_finite() && length_squared.is_finite() && length_squared > 0.0
}

fn native_transform_is_finite(transform: DTransform) -> bool {
    let rotation_length_squared = transform.rotation.length_squared();
    transform.translation.is_finite()
        && transform.rotation.is_finite()
        && rotation_length_squared.is_finite()
        && transform.scale.is_finite()
        && rotation_length_squared > 0.0
}

/// Cosine of the angle between two finite, non-zero vectors.  The clamp is
/// kept beside the dot/normalization operation because round-off can place an
/// otherwise valid cosine just outside acos' domain.
fn native_cosine(a: DVec3, b: DVec3) -> Result<f64, Box<EvalAltResult>> {
    if !native_vec3_is_valid(a) || !native_vec3_is_valid(b) {
        return Err(invalid_value(
            "vector cosine requires finite, non-zero vectors",
        ));
    }
    let denominator = a.length() * b.length();
    if !denominator.is_finite() || denominator <= 0.0 {
        return Err(invalid_value("vector cosine has no finite denominator"));
    }
    let cosine = (a.dot(b) / denominator).clamp(-1.0, 1.0);
    cosine
        .is_finite()
        .then_some(cosine)
        .ok_or_else(|| invalid_value("vector cosine must be finite"))
}

fn native_angle_rad(a: DVec3, b: DVec3) -> Result<f64, Box<EvalAltResult>> {
    native_cosine(a, b).and_then(|cosine| {
        let angle = cosine.acos();
        angle
            .is_finite()
            .then_some(angle)
            .ok_or_else(|| invalid_value("vector angle must be finite"))
    })
}

fn native_normalize(a: DVec3) -> Result<DVec3, Box<EvalAltResult>> {
    if !a.is_finite() {
        return Err(invalid_value("vector must contain only finite values"));
    }
    let length = a.length();
    if length == 0.0 {
        return Err(invalid_value("cannot normalize a zero-length vector"));
    }
    finite_vec3(a / length, "normalized vector")
}

fn native_rotate(q: DQuat, v: DVec3) -> Result<DVec3, Box<EvalAltResult>> {
    let q = finite_quat(q, "quaternion")?;
    finite_vec3(q * v, "rotated vector")
}

fn native_quat_mul(a: DQuat, b: DQuat) -> Result<DQuat, Box<EvalAltResult>> {
    let a = finite_quat(a, "left quaternion")?;
    let b = finite_quat(b, "right quaternion")?;
    finite_quat(a * b, "quaternion product")
}

fn native_quat_inverse(q: DQuat) -> Result<DQuat, Box<EvalAltResult>> {
    let q = finite_quat(q, "quaternion")?;
    finite_quat(q.inverse(), "quaternion inverse")
}

fn native_quat_from_euler_xyz_deg(v: DVec3) -> Result<DQuat, Box<EvalAltResult>> {
    finite_vec3(v, "Euler rotation")?;
    finite_quat(
        DQuat::from_euler(
            EulerRot::XYZ,
            v.x.to_radians(),
            v.y.to_radians(),
            v.z.to_radians(),
        ),
        "Euler rotation",
    )
}

fn native_quat_to_euler_xyz_deg(q: DQuat) -> Result<DVec3, Box<EvalAltResult>> {
    let q = finite_quat(q, "quaternion")?;
    let (x, y, z) = q.to_euler(EulerRot::XYZ);
    finite_vec3(
        DVec3::new(x.to_degrees(), y.to_degrees(), z.to_degrees()),
        "Euler rotation",
    )
}

fn native_transform_compose(
    parent: DTransform,
    local: DTransform,
) -> Result<DTransform, Box<EvalAltResult>> {
    finite_transform(
        parent.translation + parent.rotation * (parent.scale * local.translation),
        parent.rotation * local.rotation,
        parent.scale * local.scale,
        "composed transform",
    )
}

fn native_transform_apply_point(
    transform: DTransform,
    point: DVec3,
) -> Result<DVec3, Box<EvalAltResult>> {
    finite_vec3(
        transform.translation + transform.rotation * (transform.scale * point),
        "transformed point",
    )
}

/// Lift a binary vector op, returning `()` on degenerate input.
fn binary(
    f: impl Fn(DVec3, DVec3) -> DVec3 + Send + Sync + 'static,
) -> impl Fn(Dynamic, Dynamic) -> Dynamic + Send + Sync + 'static {
    move |a, b| match (to_vec3(&a), to_vec3(&b)) {
        (Some(a), Some(b)) => {
            let result = f(a, b);
            if result.is_finite() {
                to_array(result)
            } else {
                Dynamic::UNIT
            }
        }
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
    engine
        .register_fn("f64_from", f64_from_dynamic)
        .register_fn("f64_only", f64_only_dynamic)
        .register_fn("array_is", array_is_dynamic)
        .register_fn("map_is", map_is_dynamic)
        .register_fn("string_is", string_is_dynamic);

    // Native glam values are the hot-loop representation.  They are registered
    // under stable script names, while the underlying types stay the same
    // `bevy::math` values used by the simulator (no second tuple implementation
    // and no per-operation array round-trip).  There are intentionally getters
    // but no setters: construction and every operation validate finiteness so a
    // script cannot mutate a valid pose into a NaN behind the bridge's back.
    register_vec2(engine);

    engine
        .register_type_with_name::<DVec3>("Vec3")
        .register_type_with_name::<DQuat>("Quat")
        .register_type_with_name::<DTransform>("Transform")
        .register_fn("vec3", |x: f64, y: f64, z: f64| {
            finite_vec3(DVec3::new(x, y, z), "vec3")
        })
        .register_fn("vec3_from", |value: Dynamic| {
            to_vec3(&value).ok_or_else(|| invalid_value("expected a finite Vec3 or [x, y, z]"))
        })
        .register_fn("vec3_zero", || DVec3::ZERO)
        .register_fn("vec3_array", to_array)
        .register_fn("vec3_is_finite", |v: DVec3| v.is_finite())
        .register_fn("vec3_is_valid", native_vec3_is_valid)
        .register_fn("vec3_is_native", native_vec3_is_native)
        .register_get("x", |v: &mut DVec3| v.x)
        .register_get("y", |v: &mut DVec3| v.y)
        .register_get("z", |v: &mut DVec3| v.z)
        .register_fn("quat", |x: f64, y: f64, z: f64, w: f64| {
            finite_quat(DQuat::from_xyzw(x, y, z, w), "quat")
        })
        .register_fn("quat_from", |value: Dynamic| {
            to_quat(&value).ok_or_else(|| invalid_value("expected a finite Quat or [x, y, z, w]"))
        })
        .register_fn("quat_identity", || DQuat::IDENTITY)
        .register_fn("quat_array", quat_to_array)
        .register_fn("quat_is_finite", |q: DQuat| q.is_finite())
        .register_fn("quat_is_valid", native_quat_is_valid)
        .register_fn("quat_from_euler_xyz_deg", native_quat_from_euler_xyz_deg)
        .register_fn("quat_to_euler_xyz_deg", native_quat_to_euler_xyz_deg)
        .register_fn("quat_inverse", native_quat_inverse)
        .register_get("x", |q: &mut DQuat| q.x)
        .register_get("y", |q: &mut DQuat| q.y)
        .register_get("z", |q: &mut DQuat| q.z)
        .register_get("w", |q: &mut DQuat| q.w);

    engine
        .register_fn(
            "transform",
            |translation: DVec3, rotation: DQuat, scale: DVec3| {
                finite_transform(translation, rotation, scale, "transform")
            },
        )
        .register_fn("transform_identity", || DTransform::IDENTITY)
        .register_fn("transform_is_finite", native_transform_is_finite)
        .register_fn("transform_compose", native_transform_compose)
        .register_fn("transform_apply_point", native_transform_apply_point)
        .register_get("translation", |transform: &mut DTransform| {
            transform.translation
        })
        .register_get("rotation", |transform: &mut DTransform| transform.rotation)
        .register_get("scale", |transform: &mut DTransform| transform.scale);

    // The familiar names are overloaded for native values as well as the
    // legacy arrays.  Existing scripts continue to exchange arrays, while new
    // scripts can keep values native from `world_pos3` through a complete
    // geometry or control calculation.
    engine
        .register_fn("+", native_add)
        .register_fn("-", native_sub)
        .register_fn("*", native_scale)
        .register_fn("*", |scalar: f64, vector: DVec3| {
            native_scale(vector, scalar)
        })
        .register_fn("*", native_quat_mul)
        .register_fn("vadd", native_add)
        .register_fn("vsub", native_sub)
        .register_fn("vscale", native_scale)
        .register_fn("vcross", native_cross)
        .register_fn("vdot", native_dot)
        .register_fn("vcosine", native_cosine)
        .register_fn("vangle_rad", native_angle_rad)
        .register_fn("vcomponent", native_vec3_component)
        .register_fn("vlen", |v: DVec3| {
            let length = v.length();
            length
                .is_finite()
                .then_some(length)
                .ok_or_else(|| invalid_value("vector length must be finite"))
        })
        .register_fn("vlen_squared", |v: DVec3| {
            let length_squared = v.length_squared();
            length_squared
                .is_finite()
                .then_some(length_squared)
                .ok_or_else(|| invalid_value("squared vector length must be finite"))
        })
        .register_fn("vnorm", native_normalize)
        .register_fn("qrot", native_rotate);

    engine.register_fn("vadd", binary(|a, b| a + b));
    engine.register_fn("vsub", binary(|a, b| a - b));
    engine.register_fn("vcross", binary(DVec3::cross));

    engine.register_fn("vscale", |a: Dynamic, k: f64| match to_vec3(&a) {
        Some(v) if k.is_finite() => {
            let result = v * k;
            if result.is_finite() {
                to_array(result)
            } else {
                Dynamic::UNIT
            }
        }
        _ => Dynamic::UNIT,
    });

    engine.register_fn("vlen", |a: Dynamic| match to_vec3(&a) {
        Some(v) => {
            let length = v.length();
            if length.is_finite() {
                Dynamic::from_float(length)
            } else {
                Dynamic::UNIT
            }
        }
        _ => Dynamic::UNIT,
    });

    engine.register_fn("vlen_squared", |a: Dynamic| match to_vec3(&a) {
        Some(v) => {
            let length_squared = v.length_squared();
            if length_squared.is_finite() {
                Dynamic::from_float(length_squared)
            } else {
                Dynamic::UNIT
            }
        }
        _ => Dynamic::UNIT,
    });

    engine.register_fn("vdot", |a: Dynamic, b: Dynamic| {
        match (to_vec3(&a), to_vec3(&b)) {
            (Some(a), Some(b)) => {
                let value = a.dot(b);
                if value.is_finite() {
                    Dynamic::from_float(value)
                } else {
                    Dynamic::UNIT
                }
            }
            _ => Dynamic::UNIT,
        }
    });

    engine.register_fn("vec3_is_valid", |value: Dynamic| {
        to_vec3(&value).is_some_and(native_vec3_is_valid)
    });

    engine.register_fn("vec3_is_native", |value: Dynamic| {
        value.clone().try_cast::<DVec3>().is_some()
    });

    engine.register_fn("vec2_is_native", |value: Dynamic| {
        value.clone().try_cast::<DVec2>().is_some()
    });

    engine.register_fn("vec3_is_finite", |value: Dynamic| {
        to_vec3(&value).is_some_and(|value| value.is_finite())
    });

    engine.register_fn("quat_is_valid", |value: Dynamic| {
        value
            .clone()
            .try_cast::<DQuat>()
            .is_some_and(native_quat_is_valid)
    });

    engine.register_fn("transform_is_finite", |value: Dynamic| {
        value
            .clone()
            .try_cast::<DTransform>()
            .is_some_and(native_transform_is_finite)
    });

    engine.register_fn("vcomponent", |value: Dynamic, index: i64| {
        to_vec3(&value)
            .and_then(|value| native_vec3_component(value, index).ok())
            .map(Dynamic::from_float)
            .unwrap_or(Dynamic::UNIT)
    });

    engine.register_fn("vcosine", |a: Dynamic, b: Dynamic| {
        match (to_vec3(&a), to_vec3(&b)) {
            (Some(a), Some(b)) => native_cosine(a, b)
                .map(Dynamic::from_float)
                .unwrap_or(Dynamic::UNIT),
            _ => Dynamic::UNIT,
        }
    });

    engine.register_fn("vangle_rad", |a: Dynamic, b: Dynamic| {
        match (to_vec3(&a), to_vec3(&b)) {
            (Some(a), Some(b)) => native_angle_rad(a, b)
                .map(Dynamic::from_float)
                .unwrap_or(Dynamic::UNIT),
            _ => Dynamic::UNIT,
        }
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
    engine.register_fn(
        "clamp",
        |x: f64, lo: f64, hi: f64| {
            if x.is_nan() { lo } else { x.clamp(lo, hi) }
        },
    );

    // qrot(q, v) — rotate a local vector by a native `Quat` (or an
    // `[x, y, z, w]` quaternion) into world space. The prelude derives every
    // world axis (up, right, …) from this one operation plus
    // `world_rotation_quat`, so there is no per-axis host read. The generic
    // overload intentionally accepts mixed native/array arguments too; this
    // keeps old scripts interoperable while preserving the array result at
    // that compatibility boundary.
    //
    // A non-unit quaternion is normalised rather than refused: a quaternion
    // arriving from an animation sample or an interpolated pose is unit only to
    // float tolerance, and rejecting it would make the axis helpers report "no
    // orientation" for a body that plainly has one.
    engine.register_fn("qrot", |q: Dynamic, v: Dynamic| {
        let Some(v) = to_vec3(&v) else {
            return Dynamic::UNIT;
        };
        let Some(quat) = to_quat(&q) else {
            return Dynamic::UNIT;
        };
        let result = quat * v;
        if result.is_finite() {
            to_array(result)
        } else {
            Dynamic::UNIT
        }
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

    crate::rhai_assembly::register(engine);
    crate::rhai_engineering::register(engine);
    crate::rhai_geometry::register(engine);
}

/// Register the common f64 two-vector type and its minimal arithmetic API.
/// SysML's standard `VectorValues::CartesianTwoVectorValue` projects to this
/// same Bevy/glam value used by profile geometry and script calculations.
pub fn register_vec2(engine: &mut Engine) {
    engine
        .register_type_with_name::<DVec2>("Vec2")
        .register_fn("vec2", |x: f64, y: f64| {
            finite_vec2(DVec2::new(x, y), "vec2")
        })
        .register_fn("vec2_from", |value: Dynamic| {
            to_vec2(&value).ok_or_else(|| invalid_value("expected a finite Vec2 or [x, y]"))
        })
        .register_fn("vec2_zero", || DVec2::ZERO)
        .register_fn("vec2_array", vec2_to_array)
        .register_fn("vec2_is_finite", |v: DVec2| v.is_finite())
        .register_get("x", |v: &mut DVec2| v.x)
        .register_get("y", |v: &mut DVec2| v.y)
        .register_fn("+", |a: DVec2, b: DVec2| finite_vec2(a + b, "vector sum"))
        .register_fn("-", |a: DVec2, b: DVec2| {
            finite_vec2(a - b, "vector difference")
        })
        .register_fn("*", |a: DVec2, scalar: f64| {
            if !scalar.is_finite() {
                return Err(invalid_value("vector scale must be finite"));
            }
            finite_vec2(a * scalar, "scaled vector")
        })
        .register_fn("v2add", |a: DVec2, b: DVec2| {
            finite_vec2(a + b, "vector sum")
        })
        .register_fn("v2sub", |a: DVec2, b: DVec2| {
            finite_vec2(a - b, "vector difference")
        })
        .register_fn("v2scale", |a: DVec2, scalar: f64| {
            if !scalar.is_finite() {
                return Err(invalid_value("vector scale must be finite"));
            }
            finite_vec2(a * scalar, "scaled vector")
        })
        .register_fn("v2dot", |a: DVec2, b: DVec2| {
            let value = a.dot(b);
            value
                .is_finite()
                .then_some(value)
                .ok_or_else(|| invalid_value("vector dot product must be finite"))
        })
        .register_fn("v2len", |v: DVec2| {
            let value = v.length();
            value
                .is_finite()
                .then_some(value)
                .ok_or_else(|| invalid_value("vector length must be finite"))
        })
        .register_fn("v2norm", |v: DVec2| {
            let length = v.length();
            if !length.is_finite() || length == 0.0 {
                return Err(invalid_value("cannot normalize a zero-length Vec2"));
            }
            finite_vec2(v / length, "normalized vector")
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
        engine()
            .eval::<Dynamic>(src)
            .expect("script should evaluate")
    }

    /// The case that poisoned a whole test run: a body holding its heading, so
    /// the dot product sits exactly on `acos`'s domain edge. It must read as
    /// zero rotation, not as NaN.
    #[test]
    fn identical_headings_measure_zero_not_nan() {
        let d = eval("yaw_delta_deg([0.0, 0.0, 1.0], [0.0, 0.0, 1.0])");
        let v = d.as_float().expect("a measurable angle");
        assert!(
            v.is_finite(),
            "identical headings must not produce NaN, got {v}"
        );
        assert!(
            v.abs() < 1e-9,
            "identical headings are zero rotation, got {v}"
        );
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
            "vlen_squared(())",
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

    #[test]
    fn native_vec3_uses_glam_without_array_round_trip() {
        let d = eval("let p = vec3(1.0, 2.0, 3.0); vec3_array(vadd(p, vec3(2.0, 0.0, -1.0)))");
        let a = d.into_array().expect("native vector must lower explicitly");
        assert_eq!(a.len(), 3);
        assert_eq!(a[0].as_float().unwrap(), 3.0);
        assert_eq!(a[1].as_float().unwrap(), 2.0);
        assert_eq!(a[2].as_float().unwrap(), 2.0);
    }

    #[test]
    fn native_vector_validity_and_cosine_are_f64_primitives() {
        let d = eval(
            "let a = vec3(1.0, 0.0, 0.0); \
             let b = vec3(0.0, 1.0, 0.0); \
             [vec3_is_valid(a), vec3_is_valid(vec3_zero()), \
              vec3_is_valid([1.0, 0.0, 0.0]), \
              vcosine(a, b), vcosine([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]), \
              f64_only(vcosine(a, b)) != ()]",
        );
        let values = d
            .into_array()
            .expect("vector primitives must return values");
        assert!(values[0].as_bool().unwrap());
        assert!(!values[1].as_bool().unwrap());
        assert!(values[2].as_bool().unwrap());
        assert!(values[3].as_float().unwrap().abs() < 1e-15);
        assert!(values[4].as_float().unwrap().abs() < 1e-15);
        assert!(values[5].as_bool().unwrap());
    }

    #[test]
    fn numeric_boundary_converters_are_explicit_and_f64_based() {
        let d = eval(
            "[f64_from(3), f64_only(3), f64_only(3.0), array_is([1.0, 2.0]), map_is(#{}), map_is([]), string_is(\"x\"), string_is(1)]",
        );
        let values = d.into_array().expect("numeric boundary must return values");
        assert_eq!(values[0].as_float().unwrap(), 3.0);
        assert!(
            values[1].is_unit(),
            "integer must not pass f64-only boundary"
        );
        assert_eq!(values[2].as_float().unwrap(), 3.0);
        assert!(values[3].as_bool().unwrap());
        assert!(values[4].as_bool().unwrap());
        assert!(!values[5].as_bool().unwrap());
        assert!(values[6].as_bool().unwrap());
        assert!(!values[7].as_bool().unwrap());
    }

    #[test]
    fn native_vec3_properties_and_quat_rotation_are_typed() {
        let d = eval("let p = vec3(1.0, 2.0, 3.0); [p.x, p.y, p.z]");
        let a = d.into_array().expect("vector properties must be readable");
        assert_eq!(a[0].as_float().unwrap(), 1.0);
        assert_eq!(a[2].as_float().unwrap(), 3.0);

        let d = eval("vec3_array(qrot(quat(0.0, 0.0, 0.0, 1.0), vec3(4.0, 5.0, 6.0)))");
        let a = d
            .into_array()
            .expect("quaternion rotation must stay native");
        assert_eq!(a[0].as_float().unwrap(), 4.0);
        assert_eq!(a[2].as_float().unwrap(), 6.0);
    }

    #[test]
    fn native_quaternion_euler_conversion_is_shared_with_usd_xyz() {
        let d = eval(
            "let q = quat_from_euler_xyz_deg(vec3(10.0, 20.0, 30.0)); \
             vec3_array(quat_to_euler_xyz_deg(q))",
        );
        let a = d.into_array().expect("Euler conversion must return Vec3");
        assert!((a[0].as_float().unwrap() - 10.0).abs() < 1e-9);
        assert!((a[1].as_float().unwrap() - 20.0).abs() < 1e-9);
        assert!((a[2].as_float().unwrap() - 30.0).abs() < 1e-9);
    }

    #[test]
    fn native_transform_keeps_pose_typed_and_composes_in_f64() {
        let d = eval(
            "let parent = transform(vec3(10.0, 0.0, 0.0), quat_identity(), vec3(2.0, 2.0, 2.0)); \
             let local = transform(vec3(1.0, 2.0, 3.0), quat_identity(), vec3(1.0, 1.0, 1.0)); \
             let composed = transform_compose(parent, local); \
             [transform_is_finite(composed), composed.translation.x, composed.translation.y, composed.translation.z]",
        );
        let values = d
            .into_array()
            .expect("transform result must stay typed until fields are read");
        assert!(values[0].as_bool().unwrap());
        assert_eq!(values[1].as_float().unwrap(), 12.0);
        assert_eq!(values[2].as_float().unwrap(), 4.0);
        assert_eq!(values[3].as_float().unwrap(), 6.0);
    }

    #[test]
    fn native_transform_applies_points_before_render_lowering() {
        let d = eval(
            "let pose = transform(vec3(4.0, 5.0, 6.0), quat_identity(), vec3(2.0, 3.0, 4.0)); \
             vec3_array(transform_apply_point(pose, vec3(1.0, 1.0, 1.0)))",
        );
        let values = d
            .into_array()
            .expect("point application must lower explicitly");
        assert_eq!(values[0].as_float().unwrap(), 6.0);
        assert_eq!(values[1].as_float().unwrap(), 8.0);
        assert_eq!(values[2].as_float().unwrap(), 10.0);
    }

    #[test]
    fn native_constructors_reject_non_finite_and_degenerate_values() {
        assert!(engine().eval::<Dynamic>("vec3(0.0/0.0, 0.0, 0.0)").is_err());
        assert!(
            engine()
                .eval::<Dynamic>("quat(0.0, 0.0, 0.0, 0.0)")
                .is_err()
        );
        assert!(engine().eval::<Dynamic>("vnorm(vec3_zero())").is_err());
    }

    #[test]
    fn native_values_are_recognised_by_array_math_without_coercion_in_script() {
        assert!(!eval("quat_identity() == ()").as_bool().unwrap());
        let d = eval("vlen(vec3(3.0, 4.0, 0.0))");
        assert_eq!(d.as_float().unwrap(), 5.0);
        let d = eval("vlen([3.0, 4.0, 0.0])");
        assert_eq!(d.as_float().unwrap(), 5.0);
        let d = eval("vlen_squared(vec3(3.0, 4.0, 0.0))");
        assert_eq!(d.as_float().unwrap(), 25.0);
        let d = eval("vlen_squared([3.0, 4.0, 0.0])");
        assert_eq!(d.as_float().unwrap(), 25.0);

        let d = eval("qrot(quat_identity(), [4.0, 5.0, 6.0])");
        let a = d.into_array().expect("mixed native/array qrot must work");
        assert_eq!(a[0].as_float().unwrap(), 4.0);
    }

    #[test]
    fn standard_scalar_math_remains_rhai_owned_and_rust_backed() {
        let d = eval(
            "[sin(PI() / 2.0), cos(0.0), exp(0.0), sqrt(9.0), atan(1.0, 1.0), hypot(3.0, 4.0)]",
        );
        let values = d.into_array().expect("standard math returns an array");
        let expected = [1.0, 1.0, 1.0, 3.0, std::f64::consts::FRAC_PI_4, 5.0];
        for (value, expected) in values.iter().zip(expected) {
            assert!((value.as_float().unwrap() - expected).abs() < 1e-12);
        }
    }
}
