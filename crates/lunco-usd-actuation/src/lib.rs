//! Composed USD actuation readers for co-simulation projections.
//!
//! Actuator descriptions are physical USD intent. This package reads the
//! standard composed reader surface and produces the generic co-simulation
//! actuator components; controller policy and command propagation remain in
//! `lunco-cosim`.

use bevy::prelude::*;
use lunco_cosim_core::{ForceActuator, TorqueActuator};
use lunco_usd_bevy_stage::read::UsdReadObject;
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
            "[usd-actuation] force actuator {} has no PhysicsRigidBodyAPI ancestor",
            actuator_path
        );
        return None;
    };
    let Some(relative) =
        lunco_usd_bevy_stage::transform_in_body_frame(reader, &body_path, actuator_path)
    else {
        warn!(
            "[usd-actuation] force actuator {} could not derive its body-frame transform",
            actuator_path
        );
        return None;
    };
    let Some(direction_local) =
        actuator_direction_in_body(reader, actuator_path, FORCE_DIRECTION_ATTR, relative)
    else {
        warn!(
            "[usd-actuation] force actuator {actuator_path} has no valid body-frame {FORCE_DIRECTION_ATTR}"
        );
        return None;
    };
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
    let Some(body_path) = actuator_body_path(reader, actuator_path) else {
        warn!(
            "[usd-actuation] torque actuator {actuator_path} has no PhysicsRigidBodyAPI ancestor"
        );
        return None;
    };
    let Some(relative) =
        lunco_usd_bevy_stage::transform_in_body_frame(reader, &body_path, actuator_path)
    else {
        warn!(
            "[usd-actuation] torque actuator {actuator_path} could not derive its body-frame transform"
        );
        return None;
    };
    let Some(axis_local) =
        actuator_direction_in_body(reader, actuator_path, TORQUE_AXIS_ATTR, relative)
    else {
        warn!(
            "[usd-actuation] torque actuator {actuator_path} has no valid body-frame {TORQUE_AXIS_ATTR}"
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

/// Directions are authored in the actuator prim frame. Compose the stage basis
/// and relative mount rotation before publishing the native unit-vector field.
fn actuator_direction_in_body(
    reader: &dyn UsdReadObject,
    actuator_path: &SdfPath,
    attribute: &str,
    relative: lunco_core::DTransform,
) -> Option<bevy::math::DVec3> {
    let authored = bevy::math::DVec3::from_array(lunco_usd_bevy_stage::read::read_vec3_f64(
        reader,
        actuator_path,
        attribute,
    )?);
    let direction = authored.try_normalize()?;
    let convention = lunco_usd_bevy_stage::stage_convention(reader).ok()?;
    let canonical_direction = convention.dir_d(direction);
    (relative.rotation * canonical_direction).try_normalize()
}
