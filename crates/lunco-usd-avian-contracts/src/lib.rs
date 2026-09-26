//! Shared USD/Avian runtime contracts.
//!
//! This package contains only ECS carriers, generic projection lifecycle seams,
//! and drive conversion.
//! It deliberately has no USD stage traversal, projection systems, or scene
//! policy. Keeping these types outside the large projection crate lets query,
//! readiness, and co-simulation packages observe the same components without
//! depending on the full USD-to-Avian runtime.

use avian3d::prelude::{JointDamping, MotorModel};
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use openusd::schemas::physics::CollisionApprox;

/// Mesh approximation modes that the Avian adapter can cook from standard
/// `UsdPhysicsMeshCollisionAPI` input.
///
/// OpenUSD defines six tokens. Keeping the four implemented modes in this
/// adapter contract lets authoring queries expose exactly what the runtime can
/// realize, while still parsing and diagnosing the complete standard token set
/// through [`CollisionApprox`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvianMeshApproximation {
    /// Use the source triangles directly; valid only for static or kinematic
    /// bodies under the USD Physics mesh-collision rules.
    TriangleMesh,
    ConvexHull,
    ConvexDecomposition,
    /// An oriented box fitted to the mesh vertices in the mesh's local frame.
    /// The current principal-axis fitting algorithm is deterministic but does
    /// not guarantee the globally minimum-volume box.
    BoundingCube,
}

impl AvianMeshApproximation {
    /// Implemented USD modes, in the stable order used by authoring tools.
    pub const ALL: [Self; 4] = [
        Self::TriangleMesh,
        Self::ConvexHull,
        Self::ConvexDecomposition,
        Self::BoundingCube,
    ];

    pub const fn as_usd_approximation(self) -> CollisionApprox {
        match self {
            Self::TriangleMesh => CollisionApprox::None,
            Self::ConvexHull => CollisionApprox::ConvexHull,
            Self::ConvexDecomposition => CollisionApprox::ConvexDecomposition,
            Self::BoundingCube => CollisionApprox::BoundingCube,
        }
    }

    pub const fn requires_static_or_kinematic_body(self) -> bool {
        matches!(self, Self::TriangleMesh)
    }
}

impl TryFrom<CollisionApprox> for AvianMeshApproximation {
    type Error = CollisionApprox;

    fn try_from(value: CollisionApprox) -> Result<Self, Self::Error> {
        match value {
            CollisionApprox::None => Ok(Self::TriangleMesh),
            CollisionApprox::ConvexHull => Ok(Self::ConvexHull),
            CollisionApprox::ConvexDecomposition => Ok(Self::ConvexDecomposition),
            CollisionApprox::BoundingCube => Ok(Self::BoundingCube),
            unsupported => Err(unsupported),
        }
    }
}

/// Failure to derive the oriented `UsdPhysics` bounding cube from source mesh
/// vertices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoundingCubeFitError {
    FewerThanFourVertices,
    NonFiniteVertex,
    DegenerateExtent,
}

impl std::fmt::Display for BoundingCubeFitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FewerThanFourVertices => f.write_str("needs at least four mesh vertices"),
            Self::NonFiniteVertex => f.write_str("contains a non-finite mesh vertex"),
            Self::DegenerateExtent => f.write_str("has a zero fitted extent on a box axis"),
        }
    }
}

impl std::error::Error for BoundingCubeFitError {}

/// Fit the same local oriented box used by the Avian `boundingCube` cooker.
///
/// The returned corners are the authoritative cooked geometry. Consumers
/// deriving placement bounds must use these corners too, so an oriented box
/// cannot silently become a different axis-aligned box in another projection.
/// The backend fit is deterministic but is not guaranteed to be globally
/// minimum-volume.
pub fn fit_bounding_cube(vertices: &[DVec3]) -> Result<[DVec3; 8], BoundingCubeFitError> {
    if vertices.len() < 4 {
        return Err(BoundingCubeFitError::FewerThanFourVertices);
    }
    if vertices.iter().any(|vertex| !vertex.is_finite()) {
        return Err(BoundingCubeFitError::NonFiniteVertex);
    }

    let (pose, cuboid) = avian3d::parry::utils::obb(vertices);
    let half = cuboid.half_extents;
    if !half.is_finite()
        || half.x <= f64::EPSILON
        || half.y <= f64::EPSILON
        || half.z <= f64::EPSILON
    {
        return Err(BoundingCubeFitError::DegenerateExtent);
    }
    Ok(std::array::from_fn(|index| {
        let bits = index as u8;
        pose.transform_point(DVec3::new(
            if bits & 1 == 0 { -half.x } else { half.x },
            if bits & 2 == 0 { -half.y } else { half.y },
            if bits & 4 == 0 { -half.z } else { half.z },
        ))
    }))
}

#[cfg(test)]
mod bounding_cube_tests {
    use super::{BoundingCubeFitError, fit_bounding_cube};
    use bevy::math::{DQuat, DVec3};

    fn pairwise_distances(points: &[DVec3]) -> Vec<f64> {
        let mut distances = Vec::new();
        for left in 0..points.len() {
            for right in (left + 1)..points.len() {
                distances.push(points[left].distance(points[right]));
            }
        }
        distances.sort_by(f64::total_cmp);
        distances
    }

    #[test]
    fn bounding_cube_fits_rotated_source_geometry() {
        let rotation = DQuat::from_rotation_y(0.63);
        let source: Vec<DVec3> = (0..8)
            .map(|bits| {
                rotation
                    * DVec3::new(
                        if bits & 1 == 0 { -2.0 } else { 2.0 },
                        if bits & 2 == 0 { -1.0 } else { 1.0 },
                        if bits & 4 == 0 { -0.5 } else { 0.5 },
                    )
            })
            .collect();
        let fitted = fit_bounding_cube(&source).expect("non-degenerate box fits");
        let fitted = fitted.as_slice();

        assert_eq!(fitted.len(), 8);
        for (source, fitted) in pairwise_distances(&source)
            .iter()
            .zip(pairwise_distances(fitted))
        {
            assert!((source - fitted).abs() < 1.0e-8);
        }
        let source_aabb_volume = {
            let min = source.iter().copied().reduce(DVec3::min).unwrap();
            let max = source.iter().copied().reduce(DVec3::max).unwrap();
            let size = max - min;
            size.x * size.y * size.z
        };
        assert!(source_aabb_volume > 8.0);
    }

    #[test]
    fn bounding_cube_reports_invalid_source_geometry() {
        assert_eq!(
            fit_bounding_cube(&[DVec3::ZERO, DVec3::X, DVec3::Y]),
            Err(BoundingCubeFitError::FewerThanFourVertices)
        );
        assert_eq!(
            fit_bounding_cube(&[DVec3::ZERO, DVec3::X, DVec3::Y, DVec3::NAN]),
            Err(BoundingCubeFitError::NonFiniteVertex)
        );
        assert_eq!(
            fit_bounding_cube(&[DVec3::ZERO, DVec3::X, DVec3::Y, DVec3::ZERO]),
            Err(BoundingCubeFitError::DegenerateExtent)
        );
    }
}

/// Marks an Avian entity synthesized for the currently mounted USD scene.
///
/// Authored physics prims have their own scene identity. Synthesized wheel
/// joints and world-anchor bodies have no authored identity, so this marker is
/// the explicit ownership fact used by scene teardown and runtime queries.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct ScenePhysicsOwned;

/// Marker for a USD prim whose standard physics projection has completed.
///
/// The Avian projection owns insertion; the generic live-stage bridge owns
/// invalidation when a composed rigid-body schema is added after the entity
/// already exists. Keeping the marker in this contract package avoids making
/// that bridge depend on the full USD-to-Avian projector.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct UsdPhysicsProjected;

/// Re-arm standard USD physics projection for a newly composed rigid-body prim.
///
/// A live reference can add `PhysicsRigidBodyAPI` to an already-existing
/// instance root. Existing Avian bodies are left untouched; only a typeless
/// entity without a rigid body is re-armed for the projection observer.
pub fn invalidate_usd_physics_projection(world: &mut World, entity: Entity) -> bool {
    if world.get::<avian3d::prelude::RigidBody>(entity).is_some() {
        return false;
    }
    let Ok(mut entity_mut) = world.get_entity_mut(entity) else {
        return false;
    };
    entity_mut.remove::<UsdPhysicsProjected>();
    true
}

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
