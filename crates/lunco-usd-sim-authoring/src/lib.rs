//! Render-free authored facts shared by vehicle projection and validation.
//!
//! This package owns the composed-USD readers for specialized vehicle
//! schemas. Runtime ECS admission remains in `lunco-usd-sim`; keeping these
//! readers here lets validation reuse the authoritative interpretation without
//! importing the complete vehicle runtime.

mod lint;
pub mod wheel_params;
mod wheel_visual;

pub use lint::{append_gear_drive_facts, append_wheel_attachment_facts};
pub use wheel_params::{
    SuspensionParams, WheelAttachmentBinding, WheelAttachmentTopology, WheelParams,
    collect_wheel_attachment_topology, collect_wheel_attachment_topology_from_paths,
};
pub use wheel_visual::wheel_render_root;

/// The normalized SI angular drive values of one PhysX gear joint.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GearDriveValues {
    /// Standard force or acceleration interpretation.
    pub drive_type: lunco_mobility::DifferentialDriveType,
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
    let drive = lunco_usd_avian_reader::joint::read_drive(reader, prim, "angular")?.ok_or(())?;
    drive.motor_model().map_err(|_| ())?;
    let drive_type = match drive
        .drive_type
        .unwrap_or(openusd_schemas::physics::DriveType::Force)
    {
        openusd_schemas::physics::DriveType::Force => lunco_mobility::DifferentialDriveType::Force,
        openusd_schemas::physics::DriveType::Acceleration => {
            lunco_mobility::DifferentialDriveType::Acceleration
        }
    };
    Ok(GearDriveValues {
        drive_type,
        rest_offset: drive.target_position.unwrap_or(0.0),
        target_velocity: drive.target_velocity.unwrap_or(0.0),
        stiffness: drive.stiffness.unwrap_or(0.0),
        damping: drive.damping.unwrap_or(0.0),
        max_force: drive.max_force.unwrap_or(f64::INFINITY),
    })
}
