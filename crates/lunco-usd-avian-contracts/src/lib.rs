//! Shared USD/Avian runtime contracts.
//!
//! This package contains only ECS carriers, generic projection lifecycle seams,
//! and drive conversion.
//! It deliberately has no USD stage traversal, projection systems, or scene
//! policy. Keeping these types outside the large projection crate lets query,
//! readiness, and co-simulation packages observe the same components without
//! depending on the full USD-to-Avian runtime.

use avian3d::prelude::{Collider, JointDamping, MotorModel};
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use openusd_schemas::physics::CollisionApprox;

/// Composed USD prim actually cooked into this native collider. A rigid body
/// owns its colliders through Avian's `ColliderOf` relationship; provenance is
/// attached to the admitted collider, including internally decomposed meshes.
#[derive(Component, Clone, Debug)]
pub struct UsdColliderSource(pub String);

/// Read the source of the actual admitted contact collider. Internal mesh
/// triangles and convex-decomposition hulls share their owning USD prim.
pub fn collider_source(world: &World, collider: Entity) -> Option<&str> {
    world.get::<Collider>(collider)?;
    world
        .get::<UsdColliderSource>(collider)
        .map(|source| source.0.as_str())
}

/// Mesh approximation modes that the Avian adapter can cook from standard
/// `UsdPhysicsMeshCollisionAPI` input.
///
/// OpenUSD defines six tokens. Keeping the three implemented modes in this
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
}

/// Geometry representation produced by one implemented USD mesh mode.
///
/// This describes the adapter output rather than the USD token. Keeping the
/// distinction explicit lets authoring tools explain both what was selected
/// and what the solver will receive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvianCookedMeshGeometry {
    TriangleMesh,
    ConvexHull,
    ConvexHullCompound,
}

impl AvianCookedMeshGeometry {
    pub const fn as_token(self) -> &'static str {
        match self {
            Self::TriangleMesh => "triangle_mesh",
            Self::ConvexHull => "convex_hull",
            Self::ConvexHullCompound => "convex_hull_compound",
        }
    }
}

/// Rigid-body classes compatible with a cooked mesh mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvianMeshBodySupport {
    AnyRigidBody,
    StaticOrKinematicOnly,
}

impl AvianMeshBodySupport {
    pub const fn as_token(self) -> &'static str {
        match self {
            Self::AnyRigidBody => "any_rigid_body",
            Self::StaticOrKinematicOnly => "static_or_kinematic_only",
        }
    }
}

/// Complete runtime contract for an implemented USD mesh approximation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AvianMeshApproximationCapability {
    pub approximation: AvianMeshApproximation,
    pub geometry: AvianCookedMeshGeometry,
    pub body_support: AvianMeshBodySupport,
}

impl AvianMeshApproximation {
    /// Implemented USD modes, in the stable order used by authoring tools.
    pub const ALL: [Self; 3] = [
        Self::TriangleMesh,
        Self::ConvexHull,
        Self::ConvexDecomposition,
    ];

    pub const fn as_usd_approximation(self) -> CollisionApprox {
        match self {
            Self::TriangleMesh => CollisionApprox::None,
            Self::ConvexHull => CollisionApprox::ConvexHull,
            Self::ConvexDecomposition => CollisionApprox::ConvexDecomposition,
        }
    }

    pub const fn requires_static_or_kinematic_body(self) -> bool {
        matches!(
            self.capability().body_support,
            AvianMeshBodySupport::StaticOrKinematicOnly
        )
    }

    /// Runtime geometry and body restrictions used by both projection and
    /// authoring capability queries.
    pub const fn capability(self) -> AvianMeshApproximationCapability {
        let (geometry, body_support) = match self {
            Self::TriangleMesh => (
                AvianCookedMeshGeometry::TriangleMesh,
                AvianMeshBodySupport::StaticOrKinematicOnly,
            ),
            Self::ConvexHull => (
                AvianCookedMeshGeometry::ConvexHull,
                AvianMeshBodySupport::AnyRigidBody,
            ),
            Self::ConvexDecomposition => (
                AvianCookedMeshGeometry::ConvexHullCompound,
                AvianMeshBodySupport::AnyRigidBody,
            ),
        };
        AvianMeshApproximationCapability {
            approximation: self,
            geometry,
            body_support,
        }
    }
}

impl TryFrom<CollisionApprox> for AvianMeshApproximation {
    type Error = CollisionApprox;

    fn try_from(value: CollisionApprox) -> Result<Self, Self::Error> {
        match value {
            CollisionApprox::None => Ok(Self::TriangleMesh),
            CollisionApprox::ConvexHull => Ok(Self::ConvexHull),
            CollisionApprox::ConvexDecomposition => Ok(Self::ConvexDecomposition),
            unsupported => Err(unsupported),
        }
    }
}

#[cfg(test)]
mod mesh_approximation_capability_tests {
    use super::{AvianMeshApproximation, AvianMeshBodySupport};

    #[test]
    fn supported_usd_tokens_round_trip_through_one_capability_contract() {
        for approximation in AvianMeshApproximation::ALL {
            let capability = approximation.capability();
            assert_eq!(capability.approximation, approximation);
            assert_eq!(
                AvianMeshApproximation::try_from(approximation.as_usd_approximation()),
                Ok(approximation)
            );
            assert_eq!(
                approximation.requires_static_or_kinematic_body(),
                matches!(
                    capability.body_support,
                    AvianMeshBodySupport::StaticOrKinematicOnly
                )
            );
        }
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
    pub drive_type: Option<openusd_schemas::physics::DriveType>,
}

impl JointDrive {
    /// Convert the normalized USD drive law into Avian's stable motor model.
    pub fn motor_model(&self) -> Result<MotorModel, lunco_physics::ForceDriveMotorError> {
        if [self.target_position, self.target_velocity]
            .into_iter()
            .flatten()
            .any(|value| !value.is_finite())
            || self
                .max_force
                .is_some_and(|value| value.is_nan() || value < 0.0)
        {
            return Err(lunco_physics::ForceDriveMotorError::InvalidParameters);
        }
        let stiffness = self.stiffness.unwrap_or(0.0);
        let damping = self.damping.unwrap_or(0.0);
        if !stiffness.is_finite() || !damping.is_finite() || stiffness < 0.0 || damping < 0.0 {
            return Err(lunco_physics::ForceDriveMotorError::InvalidParameters);
        }
        match self
            .drive_type
            .unwrap_or(openusd_schemas::physics::DriveType::Force)
        {
            openusd_schemas::physics::DriveType::Acceleration => {
                Ok(MotorModel::AccelerationBased { stiffness, damping })
            }
            openusd_schemas::physics::DriveType::Force => {
                lunco_physics::force_drive_motor_model(stiffness, damping)
            }
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
