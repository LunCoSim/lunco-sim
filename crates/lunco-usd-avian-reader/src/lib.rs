//! Shared composed OpenUSD physics readers.
//!
//! This package owns the schema-to-Avian reading mechanisms reused by the live
//! physics projector and authored-stage lint facts. It does not install Bevy
//! systems, create entities, apply runtime policy, or own scene lifecycle.

use bevy::math::{DQuat, DVec3};
use lunco_usd_bevy_stage::read::UsdReadObject;
use openusd::sdf::Path as SdfPath;

pub mod collider;
pub mod joint;

/// Read a scalar only when its authored value is readable. `None` is reserved
/// for an omitted attribute, so a wrong USD type cannot become a schema default
/// at a physics boundary.
pub fn read_authored_real(
    reader: &dyn UsdReadObject,
    path: &SdfPath,
    attr: &str,
) -> Result<Option<f64>, ()> {
    match reader.real(path, attr) {
        Some(value) => Ok(Some(value)),
        None if !reader.has_authored_attribute(path, attr) => Ok(None),
        None => Err(()),
    }
}

/// Read an authored vector without treating a malformed value as an omitted
/// override. Physics vectors must also remain finite after the read.
pub fn read_authored_vec3(
    reader: &dyn UsdReadObject,
    path: &SdfPath,
    attr: &str,
) -> Result<Option<DVec3>, ()> {
    if !reader.has_authored_attribute(path, attr) {
        return Ok(None);
    }
    let value = reader
        .vec3_f64(path, attr)
        .map(|v| DVec3::new(v[0], v[1], v[2]))
        .ok_or(())?;
    value.is_finite().then_some(Some(value)).ok_or(())
}

/// Read an authored quaternion, rejecting wrong types, non-finite values and
/// the zero quaternion rather than silently using identity.
pub fn read_authored_quat(
    reader: &dyn UsdReadObject,
    path: &SdfPath,
    attr: &str,
) -> Result<Option<DQuat>, ()> {
    if !reader.has_authored_attribute(path, attr) {
        return Ok(None);
    }
    let q = reader
        .attr_value(path, attr)
        .and_then(|value| value.get::<openusd::gf::Quatf>())
        .ok_or(())?;
    let q = DQuat::from_xyzw(q.x as f64, q.y as f64, q.z as f64, q.w as f64);
    if !q.is_finite() || q.length_squared() <= f64::EPSILON {
        return Err(());
    }
    Ok(Some(q.normalize()))
}

/// Read a boolean while preserving the distinction between an omitted standard
/// default and an authored value of the wrong type.
pub fn read_authored_bool_or_default(
    reader: &dyn UsdReadObject,
    path: &SdfPath,
    attr: &str,
    default: bool,
) -> Result<bool, ()> {
    match reader.boolean(path, attr) {
        Some(value) => Ok(value),
        None if !reader.has_authored_attribute(path, attr) => Ok(default),
        None => Err(()),
    }
}
