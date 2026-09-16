//! Shared USD/Avian runtime contracts.
//!
//! This package contains only ECS carriers and their generic drive conversion.
//! It deliberately has no USD stage traversal, projection systems, or scene
//! policy. Keeping these types outside the large projection crate lets query,
//! readiness, and co-simulation packages observe the same components without
//! depending on the full USD-to-Avian runtime.

use avian3d::prelude::{JointDamping, MotorModel};
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;

/// Marks an Avian entity synthesized for the currently mounted USD scene.
///
/// Authored physics prims have their own scene identity. Synthesized wheel
/// joints and world-anchor bodies have no authored identity, so this marker is
/// the explicit ownership fact used by scene teardown and runtime queries.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct ScenePhysicsOwned;

/// Marker for a USD prim awaiting joint creation.
///
/// The USD reader fills this carrier from standard `UsdPhysics` relationships;
/// the projection crate consumes it only after both body entities are live.
#[derive(Component)]
pub struct PendingUsdJoint {
    /// USD path to body0, the anchor/chassis.
    pub body0_path: String,
    /// USD path to body1, the driven body/wheel.
    pub body1_path: String,
    /// Joint axis in the local frame of body0.
    pub axis: DVec3,
    /// Anchor point on body0 in its local frame.
    pub local_pos0: DVec3,
    /// Anchor point on body1 in its local frame.
    pub local_pos1: DVec3,
    /// Joint-frame basis on body0.
    pub local_rot0: DQuat,
    /// Joint-frame basis on body1.
    pub local_rot1: DQuat,
    /// Lower travel limit in the normalized runtime units.
    pub limit_lower: f64,
    /// Upper travel limit in the normalized runtime units.
    pub limit_upper: f64,
    /// Concrete USD joint type.
    pub joint_type: String,
    /// Spherical-joint swing cone half-angles, when authored.
    pub swing_limit: Option<(f64, f64)>,
    /// Authored USD drive, when present.
    pub drive: Option<JointDrive>,
    /// Authored passive relative-velocity damping, when present.
    pub damping: Option<JointDamping>,
}

/// A `UsdPhysicsDriveAPI` drive after USD units have been normalized.
#[derive(Clone, Copy, Default)]
pub struct JointDrive {
    /// Target position in radians or meters.
    pub target_position: Option<f64>,
    /// Target velocity in radians/second or meters/second.
    pub target_velocity: Option<f64>,
    /// Force or torque saturation.
    pub max_force: Option<f64>,
    /// Stiffness in normalized runtime units.
    pub stiffness: Option<f64>,
    /// Damping in normalized runtime units.
    pub damping: Option<f64>,
    /// Authored USD drive type.
    pub drive_type: Option<openusd::schemas::physics::DriveType>,
    /// Authored generalized inertia, when complete mass properties certify it.
    pub generalized_inertia: Option<f64>,
}

impl JointDrive {
    /// Convert the normalized USD drive law into Avian's stable motor model.
    pub fn motor_model(&self) -> Result<MotorModel, lunco_physics::ForceDriveMotorError> {
        if self.stiffness.is_none() && self.damping.is_none() {
            return Ok(DEFAULT_JOINT_DRIVE_MOTOR_MODEL);
        }
        let stiffness = self.stiffness.unwrap_or(0.0);
        let damping = self.damping.unwrap_or(0.0);
        if stiffness == 0.0 && damping == 0.0 {
            return Ok(DEFAULT_JOINT_DRIVE_MOTOR_MODEL);
        }
        match self
            .drive_type
            .unwrap_or(openusd::schemas::physics::DriveType::Force)
        {
            openusd::schemas::physics::DriveType::Acceleration => {
                Ok(MotorModel::AccelerationBased { stiffness, damping })
            }
            openusd::schemas::physics::DriveType::Force => lunco_physics::force_drive_motor_model(
                stiffness,
                damping,
                self.generalized_inertia.unwrap_or(0.0),
            ),
        }
    }

    /// Whether this drive should start enabled.
    pub fn is_active(&self) -> bool {
        self.target_position.is_some()
            || self.target_velocity.is_some()
            || self.stiffness.is_some()
            || self.damping.is_some()
    }
}

/// Stable default for an authored drive with no explicit spring coefficients.
const DEFAULT_JOINT_DRIVE_MOTOR_MODEL: MotorModel = MotorModel::SpringDamper {
    frequency: 3.0,
    damping_ratio: 2.0,
};

/// Marker for a body held Kinematic until its USD joint constraints are ready.
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
pub struct ShouldBeDynamic;

/// Initial velocity authored on a USD body while it waits for dynamic admission.
#[derive(Component, Clone, Copy, Debug)]
pub struct AuthoredInitialVelocity {
    /// World-frame linear velocity, if authored.
    pub linear: Option<DVec3>,
    /// World-frame angular velocity, if authored.
    pub angular: Option<DVec3>,
}

/// Cross-kind lifecycle marker for a joint parked until both bodies are
/// admitted to Avian's solver graph.
#[derive(Component, Clone, Copy, Debug)]
pub struct PendingJointAdmission {
    /// First jointed body.
    pub body0: Entity,
    /// Second jointed body.
    pub body1: Entity,
}
