//! Composed USD actuation readers for co-simulation projections.
//!
//! Actuator descriptions are physical USD intent. This package reads the
//! standard composed reader surface and produces the generic co-simulation
//! actuator components; controller policy and command propagation remain in
//! `lunco-cosim`.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use bevy::prelude::*;
use lunco_cosim::{ForceActuator, TorqueActuator};
use lunco_usd_bevy_core::read::UsdReadObject;
use openusd::sdf::Path as SdfPath;

const FORCE_ACTUATOR_API: &str = "LunCoForceActuatorAPI";
const FORCE_DIRECTION_ATTR: &str = "lunco:forceActuator:direction";
const FORCE_MAX_ATTR: &str = "lunco:forceActuator:maxForce";
const TORQUE_ACTUATOR_API: &str = "LunCoTorqueActuatorAPI";
const TORQUE_AXIS_ATTR: &str = "lunco:torqueActuator:axis";
const TORQUE_MAX_ATTR: &str = "lunco:torqueActuator:maxTorque";

fn actuator_body_path(reader: &dyn UsdReadObject, actuator_path: &SdfPath) -> Option<SdfPath> {
    let mut current = actuator_path.parent();
    while let Some(path) = current {
        if path.is_abs_root() {
            return None;
        }
        if reader.has_api_schema(&path, "PhysicsRigidBodyAPI") {
            return Some(path);
        }
        current = path.parent();
    }
    None
}

/// Read a force actuator's generic USD description.
pub fn force_actuator_from_usd(
    reader: &dyn UsdReadObject,
    actuator_path: &SdfPath,
) -> Option<ForceActuator> {
    if !reader.has_api_schema(actuator_path, FORCE_ACTUATOR_API) {
        return None;
    }
    let Some(body_path) = actuator_body_path(reader, actuator_path) else {
        warn!(
            "[usd-actuation] force actuator {} has no PhysicsRigidBodyAPI ancestor; actuator ignored",
            actuator_path
        );
        return None;
    };
    let Some(relative) =
        lunco_usd_bevy_core::transform_in_body_frame(reader, &body_path, actuator_path)
    else {
        warn!(
            "[usd-actuation] force actuator {} could not derive its body-frame transform",
            actuator_path
        );
        return None;
    };
    let direction = reader
        .attr_value(actuator_path, FORCE_DIRECTION_ATTR)
        .and_then(|value| {
            value.clone().get::<[f32; 3]>().or_else(|| {
                value
                    .get::<[f64; 3]>()
                    .map(|v| [v[0] as f32, v[1] as f32, v[2] as f32])
            })
        })
        .map(Vec3::from_array)
        .filter(|v| v.is_finite() && v.length_squared() > f32::EPSILON);
    let Some(direction_in_prim_frame) = direction else {
        warn!(
            "[usd-actuation] force actuator {} has no finite non-zero {}",
            actuator_path, FORCE_DIRECTION_ATTR
        );
        return None;
    };
    let direction_local = relative.rotation * direction_in_prim_frame;
    if !direction_local.is_finite() || direction_local.length_squared() <= f32::EPSILON {
        warn!(
            "[usd-actuation] force actuator {} produced an invalid body-frame direction",
            actuator_path
        );
        return None;
    }
    let Some(max_force_n) = reader
        .real(actuator_path, FORCE_MAX_ATTR)
        .filter(|v| v.is_finite() && *v > 0.0)
    else {
        warn!(
            "[usd-actuation] force actuator {} has no positive {}",
            actuator_path, FORCE_MAX_ATTR
        );
        return None;
    };
    Some(ForceActuator {
        local_position: relative.translation,
        direction_local,
        max_force_n,
    })
}

/// Read a torque actuator's generic USD description.
pub fn torque_actuator_from_usd(
    reader: &dyn UsdReadObject,
    actuator_path: &SdfPath,
) -> Option<TorqueActuator> {
    if !reader.has_api_schema(actuator_path, TORQUE_ACTUATOR_API) {
        return None;
    }
    if actuator_body_path(reader, actuator_path).is_none() {
        warn!(
            "[usd-actuation] torque actuator {} has no PhysicsRigidBodyAPI ancestor; actuator ignored",
            actuator_path
        );
        return None;
    }
    let axis = reader
        .attr_value(actuator_path, TORQUE_AXIS_ATTR)
        .and_then(|value| {
            value.clone().get::<[f32; 3]>().or_else(|| {
                value
                    .get::<[f64; 3]>()
                    .map(|v| [v[0] as f32, v[1] as f32, v[2] as f32])
            })
        })
        .map(Vec3::from_array)
        .filter(|v| v.is_finite() && v.length_squared() > f32::EPSILON);
    let Some(axis_local) = axis else {
        warn!(
            "[usd-actuation] torque actuator {} has no finite non-zero {}",
            actuator_path, TORQUE_AXIS_ATTR
        );
        return None;
    };
    let Some(max_torque_nm) = reader
        .real(actuator_path, TORQUE_MAX_ATTR)
        .filter(|v| v.is_finite() && *v > 0.0)
    else {
        warn!(
            "[usd-actuation] torque actuator {} has no positive {}",
            actuator_path, TORQUE_MAX_ATTR
        );
        return None;
    };
    Some(TorqueActuator {
        axis_local,
        max_torque_nm,
    })
}
