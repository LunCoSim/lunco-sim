//! Render-free authored facts shared by vehicle projection and validation.
//!
//! This package owns the composed-USD readers for specialized vehicle
//! schemas. Runtime ECS admission remains in `lunco-usd-sim`; keeping these
//! readers here lets validation reuse the authoritative interpretation without
//! importing the complete vehicle runtime.

mod lint;
pub mod wheel_params;

pub use lint::{append_gear_drive_facts, append_wheel_attachment_facts};
pub use wheel_params::{
    SuspensionParams, WheelAttachmentBinding, WheelAttachmentTopology, WheelParams,
    collect_wheel_attachment_topology,
};

/// The authored angular drive values of one PhysX gear joint.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GearDriveValues {
    /// Rest angular offset.
    pub rest_offset: f64,
    /// Target angular velocity.
    pub target_velocity: f64,
    /// Drive stiffness.
    pub stiffness: f64,
    /// Drive damping.
    pub damping: f64,
    /// Maximum drive force.
    pub max_force: f64,
}

/// Whether a prim is a PhysX gear joint with an angular drive.
pub fn is_gear_drive(
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    prim: &openusd::sdf::Path,
) -> bool {
    reader.type_name(prim).as_deref() == Some("PhysxPhysicsGearJoint")
        && reader.has_api_schema(prim, "PhysicsDriveAPI:angular")
}

/// Read the finite, non-zero ratio of a PhysX gear joint.
pub fn read_gear_ratio(
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    prim: &openusd::sdf::Path,
) -> Option<f64> {
    reader
        .real(prim, "physxGearJoint:gearRatio")
        .filter(|value| value.is_finite() && *value != 0.0)
}

/// Read and validate the scalar angular drive values of a PhysX gear joint.
pub fn read_gear_drive_values(
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    prim: &openusd::sdf::Path,
) -> Result<GearDriveValues, ()> {
    let read = |name: &str, default: f64, allow_infinity: bool| match reader.real(prim, name) {
        Some(value) if value.is_finite() || (allow_infinity && value == f64::INFINITY) => Ok(value),
        Some(_) => Err(()),
        None if reader.has_authored_attribute(prim, name) => Err(()),
        None => Ok(default),
    };
    let values = GearDriveValues {
        rest_offset: read("drive:angular:physics:targetPosition", 0.0, false)?,
        target_velocity: read("drive:angular:physics:targetVelocity", 0.0, false)?,
        stiffness: read("drive:angular:physics:stiffness", 0.0, false)?,
        damping: read("drive:angular:physics:damping", 0.0, false)?,
        max_force: read("drive:angular:physics:maxForce", f64::INFINITY, true)?,
    };
    if values.stiffness < 0.0 || values.damping < 0.0 || values.max_force < 0.0 {
        return Err(());
    }
    Ok(values)
}

/// Read the authored angular drive realization of a PhysX gear joint.
pub fn read_gear_drive_type(
    reader: &dyn lunco_usd_bevy_stage::read::UsdReadObject,
    prim: &openusd::sdf::Path,
) -> Option<lunco_mobility::DifferentialDriveType> {
    match reader.text(prim, "drive:angular:physics:type") {
        Some(value) if value == "force" => Some(lunco_mobility::DifferentialDriveType::Force),
        Some(value) if value == "acceleration" => {
            Some(lunco_mobility::DifferentialDriveType::Acceleration)
        }
        Some(_) => None,
        None if reader.has_authored_attribute(prim, "drive:angular:physics:type") => None,
        None => Some(lunco_mobility::DifferentialDriveType::Force),
    }
}
