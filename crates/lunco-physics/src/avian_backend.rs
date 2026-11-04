//! Admission checks for the Avian backend representation boundary.
//!
//! The simulation keeps physics poses in `f64`, while Avian's force math and
//! OBVHS spatial backend consume `f32` vectors and AABBs. This module owns the shared
//! contract. Callers remain responsible for their lifecycle decision and
//! diagnostic subject; they must not invent a second representability rule.

use avian3d::dynamics::integrator::VelocityIntegrationData;
use avian3d::dynamics::rigid_body::{RigidBodyQueryReadOnly, forces::AccumulatedLocalAcceleration};
use avian3d::prelude::{Collider, Forces, RigidBody, SimpleCollider, WriteRigidBodyForces};
use bevy::ecs::{
    query::{QueryData, QueryFilter},
    system::lifetimeless::Read,
};
use bevy::math::{DQuat, DVec3, Quat, Vec3};
use bevy::prelude::*;
use lunco_spatial::coords::GridPos;

/// Read native body motion and its dynamic force state without mutation.
/// Static supports have no solver accumulator; dynamic admission requires one.
#[derive(QueryData)]
pub struct AvianForceState {
    pub body: Read<RigidBody>,
    disabled: Has<avian3d::prelude::RigidBodyDisabled>,
    name: Option<Read<Name>>,
    native: RigidBodyQueryReadOnly,
    integration: Option<Read<VelocityIntegrationData>>,
    local: Option<Read<AccumulatedLocalAcceleration>>,
}

impl AvianForceStateItem<'_, '_> {
    pub fn position(&self) -> GridPos {
        GridPos(self.native.position.0)
    }
    pub fn rotation(&self) -> Quat {
        self.native.rotation.0
    }
    pub fn velocity_at_point(&self, point: GridPos) -> Vec3 {
        if self.body.is_static() {
            return Vec3::ZERO;
        }
        let center =
            self.position().0 + (self.rotation() * self.native.center_of_mass.0).as_dvec3();
        self.native.velocity_at_point((point.0 - center).as_vec3())
    }
}

/// One ordered force transaction, with world and body-local quantities explicit.
#[derive(Clone, Copy, Debug)]
pub struct BodyForce {
    pub body: Entity,
    pub force: DVec3,
    pub local_force: DVec3,
    pub point: Option<GridPos>,
    pub torque: DVec3,
    pub wake: bool,
}

#[derive(Debug)]
pub struct ForceAdmissionError {
    pub body: Entity,
    pub detail: String,
}

#[derive(Default)]
pub struct ForceBatchScratch {
    predicted: std::collections::HashMap<Entity, (Vec3, Vec3, Vec3)>,
}

/// Only an enabled dynamic body consumes physical force intervals.
/// Prescribed motion retains its input holds without integrating their effort.
pub fn native_force_receiver_can_integrate(mode: RigidBody, disabled: bool) -> bool {
    mode.is_dynamic() && !disabled
}

/// Validate the native producer schedule before any solver result is committed.
pub(crate) fn install(app: &mut App) {
    use avian3d::dynamics::integrator::IntegrationSystems;
    use avian3d::prelude::{PhysicsSchedule, SolverSystems};
    app.add_systems(
        PhysicsSchedule,
        validate_native_velocity_increments
            .after(IntegrationSystems::UpdateVelocityIncrements)
            .before(SolverSystems::Substep),
    );
    // Each condition is evaluated after admission. PostSubstep still clears the
    // transient accumulators; no invalid solver body is integrated or published.
    app.configure_sets(
        PhysicsSchedule,
        (
            SolverSystems::Substep.run_if(crate::physics_is_live),
            SolverSystems::Restitution.run_if(crate::physics_is_live),
            SolverSystems::ContinuousCollision.run_if(crate::physics_is_live),
            SolverSystems::Finalize.run_if(crate::physics_is_live),
            SolverSystems::StoreContactImpulses.run_if(crate::physics_is_live),
        ),
    );
}

/// Native constant forces, gravity and timestep scaling have all run here.
/// Reject their final increments before the substep loop consumes them.
fn validate_native_velocity_increments(
    bodies: Query<(Entity, AvianForceState, Option<&crate::PhysicsOrderKey>), crate::Integrable>,
    time: Res<Time<avian3d::prelude::Substeps>>,
    mut physics_time: ResMut<Time<avian3d::prelude::Physics>>,
    mut holds: ResMut<crate::PhysicsHolds>,
    mut faults: ResMut<lunco_core::RuntimeFaults>,
    mut commands: Commands,
) {
    use avian3d::schedule::PhysicsTime;
    let delta = time.delta_secs();
    let invalid = bodies.iter().filter_map(|(entity, state, key)| {
        if !state.body.is_dynamic() {
            return None;
        }
        let (Some(integration), Some(local)) = (state.integration, state.local) else {
            return Some((entity, key, state.name, "dynamic body has no native acceleration accumulators".to_owned()));
        };
        let rotation = state.rotation();
        let local_linear = rotation * local.linear * delta;
        let local_angular = rotation * local.angular * delta;
        let linear = state.native.linear_velocity.0 + local_linear + integration.linear_increment;
        let angular = state.native.angular_velocity.0 + local_angular + integration.angular_increment;
        if integration.linear_increment.is_finite()
            && integration.angular_increment.is_finite()
            && local.linear.is_finite() && local.angular.is_finite()
            && local_linear.is_finite() && local_angular.is_finite()
            && linear.is_finite() && angular.is_finite()
        {
            return None;
        }
        Some((entity, key, state.name, format!(
            "native accumulated velocity increment exceeds finite solver range: linear_increment={:?}, angular_increment={:?}, local_linear_acceleration={:?}, local_angular_acceleration={:?}, substep_seconds={delta}, resulting_linear_velocity={linear:?}, resulting_angular_velocity={angular:?}",
            integration.linear_increment, integration.angular_increment, local.linear, local.angular,
        )))
    }).min_by(|left, right| left.1.map(|key| &key.0).cmp(&right.1.map(|key| &key.0)));
    let Some((entity, _, name, detail)) = invalid else {
        return;
    };
    let subject = name.map_or_else(
        || format!("entity {entity:?}"),
        |name| name.as_str().to_owned(),
    );
    holds.set(crate::PhysicsHolds::SAFETY_FAILURE, true);
    physics_time.pause();
    physics_time.advance_by(std::time::Duration::ZERO);
    if faults.raise(
        "physics-native-integration-range",
        Some(entity),
        subject.clone(),
        detail.clone(),
    ) {
        lunco_core::trigger_runtime_error(
            &mut commands,
            "physics-native-integration-range",
            format!("{subject}: {detail}"),
        );
    }
}

/// Check the exact native products and accumulated sums for the complete batch.
/// The caller holds this read lease until admission, then applies the same ordered
/// batch before any other force producer can mutate its native accumulators.
pub fn prepare_force_batch<F: QueryFilter>(
    bodies: &Query<AvianForceState, F>,
    batch: &[BodyForce],
    scratch: &mut ForceBatchScratch,
) -> Result<(), ForceAdmissionError> {
    scratch.predicted.clear();
    for contribution in batch {
        let state = bodies
            .get(contribution.body)
            .map_err(|_| ForceAdmissionError {
                body: contribution.body,
                detail: format!(
                    "entity {:?}: native force receiver is unavailable",
                    contribution.body
                ),
            })?;
        let error = |detail: String| ForceAdmissionError {
            body: contribution.body,
            detail: state.name.map_or_else(
                || format!("entity {:?}: {detail}", contribution.body),
                |name| format!("{}: {detail}", name.as_str()),
            ),
        };
        if !native_force_receiver_can_integrate(*state.body, state.disabled) {
            return Err(error(
                "native force receiver must be enabled and dynamic".into(),
            ));
        }
        let integration = state
            .integration
            .ok_or_else(|| error("dynamic body has no native velocity accumulator".into()))?;
        let local = state.local.ok_or_else(|| {
            error("dynamic body has no native local acceleration accumulator".into())
        })?;
        if !avian_backend_vector_is_valid(contribution.force)
            || !avian_backend_vector_is_valid(contribution.local_force)
            || !avian_backend_vector_is_valid(contribution.torque)
        {
            return Err(error(format!(
                "force={:?}, local_force={:?}, torque={:?} exceeds native representation",
                contribution.force, contribution.local_force, contribution.torque
            )));
        }
        let rotation = state.rotation();
        let inverse_inertia = state.native.effective_global_angular_inertia().inverse();
        let force = contribution.force.as_vec3();
        let moment = if let Some(point) = contribution.point {
            let center = state.position().0 + (rotation * state.native.center_of_mass.0).as_dvec3();
            if !avian_backend_force_at_point_is_valid(contribution.force, point.0, center) {
                return Err(error(format!(
                    "point={point:?}, center={center:?}, offset_m={:?}, force={:?} exceeds native point-force representation",
                    point.0 - center,
                    contribution.force
                )));
            }
            (point.0 - center).as_vec3().cross(force)
        } else {
            Vec3::ZERO
        };
        let linear = state.native.mass.inverse() * force;
        let local_linear = state.native.mass.inverse() * contribution.local_force.as_vec3();
        // Avian applies point force and explicit torque as separate additions.
        let angular_point = inverse_inertia * moment;
        let angular_torque = inverse_inertia * contribution.torque.as_vec3();
        let increments = scratch.predicted.entry(contribution.body).or_insert((
            integration.linear_increment,
            integration.angular_increment,
            local.linear,
        ));
        increments.0 += linear;
        increments.2 += local_linear;
        increments.1 += angular_point;
        let point_sum_valid = increments.1.is_finite();
        increments.1 += angular_torque;
        let combined_linear = increments.0 + rotation * increments.2;
        let combined_angular = increments.1 + rotation * local.angular;
        if !linear.is_finite()
            || !local_linear.is_finite()
            || !increments.2.is_finite()
            || !angular_point.is_finite()
            || !angular_torque.is_finite()
            || !point_sum_valid
            || !increments.0.is_finite()
            || !increments.1.is_finite()
            || !combined_linear.is_finite()
            || !combined_angular.is_finite()
        {
            return Err(error(format!(
                "native force arithmetic overflows: force={:?}, local_force={:?}, torque={:?}, inverse_mass={}, linear_increment={:?}, local_increment={:?}, angular_increment={:?}",
                contribution.force,
                contribution.local_force,
                contribution.torque,
                state.native.mass.inverse(),
                increments.0,
                increments.2,
                increments.1
            )));
        }
    }
    Ok(())
}

/// Apply a batch already admitted against the same native state.
pub fn apply_force_batch<F: QueryFilter>(
    bodies: &mut Query<Forces, F>,
    batch: &[BodyForce],
) -> Result<(), ForceAdmissionError> {
    for contribution in batch {
        if !bodies.contains(contribution.body) {
            return Err(ForceAdmissionError {
                body: contribution.body,
                detail: "admitted native force receiver retired before application".into(),
            });
        }
    }
    for contribution in batch {
        let mut forces = bodies
            .get_mut(contribution.body)
            .expect("batch receivers checked before application");
        if contribution.wake {
            if let Some(point) = contribution.point {
                forces.apply_force_at_point(contribution.force.as_vec3(), point.0);
            } else {
                forces.apply_force(contribution.force.as_vec3());
            }
            forces.apply_local_force(contribution.local_force.as_vec3());
            forces.apply_torque(contribution.torque.as_vec3());
        } else {
            let mut forces = forces.non_waking();
            if let Some(point) = contribution.point {
                forces.apply_force_at_point(contribution.force.as_vec3(), point.0);
            } else {
                forces.apply_force(contribution.force.as_vec3());
            }
            forces.apply_local_force(contribution.local_force.as_vec3());
            forces.apply_torque(contribution.torque.as_vec3());
        }
    }
    Ok(())
}

/// Check a vector before it crosses into an Avian backend value.
#[inline]
pub fn avian_backend_vector_is_valid(value: DVec3) -> bool {
    value.is_finite() && value.as_vec3().is_finite()
}

/// Check the native force and moment before Avian narrows a world-point offset.
/// Absolute world coordinates remain f64; the solver computes its moment from
/// the f32 offset and force, so both operands and that product must be finite.
#[inline]
pub fn avian_backend_force_at_point_is_valid(force: DVec3, point: DVec3, center: DVec3) -> bool {
    let offset = point - center;
    point.is_finite()
        && center.is_finite()
        && avian_backend_vector_is_valid(force)
        && avian_backend_vector_is_valid(offset)
        && offset.as_vec3().cross(force.as_vec3()).is_finite()
}

/// Check a point before it crosses into an Avian backend value.
#[inline]
pub fn avian_backend_point_is_valid(point: DVec3) -> bool {
    avian_backend_vector_is_valid(point)
}

/// Check Avian's rigid rotation precondition using Glam's unit tolerance.
#[inline]
pub fn avian_backend_rotation_is_valid(rotation: DQuat) -> bool {
    rotation.is_finite() && rotation.as_quat().is_finite() && rotation.is_normalized()
}

/// Check a pose before it crosses into Avian.
#[inline]
pub fn avian_backend_pose_is_valid(position: DVec3, rotation: DQuat) -> bool {
    avian_backend_point_is_valid(position) && avian_backend_rotation_is_valid(rotation)
}

/// Check the AABB Avian's collider tree will store and grow.
#[inline]
pub fn avian_backend_aabb_is_valid(min: DVec3, max: DVec3) -> bool {
    avian_backend_point_is_valid(min)
        && avian_backend_point_is_valid(max)
        && min.cmple(max).all()
        && min.as_vec3().cmple(max.as_vec3()).all()
}

/// Check a collider's local shape bounds before ECS insertion.
#[inline]
pub fn avian_backend_collider_shape_is_valid(collider: &Collider) -> bool {
    let aabb = collider.aabb(DVec3::ZERO, bevy::math::Quat::IDENTITY, 0.0);
    avian_backend_aabb_is_valid(aabb.min.as_dvec3(), aabb.max.as_dvec3())
}

/// Check the structural precondition for a Parry compound child.
///
/// Parry deliberately rejects nested composite shapes when constructing a
/// `Compound`. Composite colliders remain valid as standalone Avian colliders;
/// this predicate is only for callers that are about to aggregate a child into
/// another compound.
#[inline]
pub fn avian_backend_collider_is_leaf(collider: &Collider) -> bool {
    collider.shape_scaled().as_composite_shape().is_none()
}

/// Return the backend shape kind for a diagnostic at an admission boundary.
#[inline]
pub fn avian_backend_collider_shape_kind(collider: &Collider) -> String {
    format!("{:?}", collider.shape_scaled().shape_type())
}

#[cfg(test)]
mod tests {
    use super::{
        avian_backend_aabb_is_valid, avian_backend_collider_is_leaf,
        avian_backend_collider_shape_is_valid, avian_backend_point_is_valid,
        avian_backend_pose_is_valid, avian_backend_rotation_is_valid,
    };
    use avian3d::prelude::Collider;
    use bevy::math::{DQuat, DVec3};

    #[test]
    fn point_force_admission_checks_native_offset_and_moment() {
        use super::avian_backend_force_at_point_is_valid as valid;
        assert!(valid(DVec3::Y * 10.0, DVec3::X * 100000001.0, DVec3::ZERO));
        assert!(!valid(
            DVec3::Y * 1.0e31,
            DVec3::X * 100000001.0,
            DVec3::ZERO
        ));
        assert!(!valid(DVec3::Y, DVec3::X * 1.0e100, DVec3::ZERO));
        assert!(valid(DVec3::Y, DVec3::X * 1.0e100, DVec3::X * 1.0e100));
    }

    #[test]
    fn backend_admission_rejects_nonrepresentable_geometry_and_rotation() {
        assert!(avian_backend_point_is_valid(DVec3::ZERO));
        assert!(!avian_backend_point_is_valid(DVec3::new(
            f64::MAX,
            0.0,
            0.0
        )));
        assert!(avian_backend_rotation_is_valid(DQuat::IDENTITY));
        assert!(!avian_backend_rotation_is_valid(DQuat::from_xyzw(
            0.0, 0.0, 0.0, 2.0
        )));
        assert!(!avian_backend_pose_is_valid(
            DVec3::ZERO,
            DQuat::from_xyzw(0.0, 0.0, 0.0, 0.5)
        ));
        assert!(avian_backend_rotation_is_valid(
            DQuat::from_rotation_y(70.0_f64.to_radians())
                .as_quat()
                .as_dquat()
        ));
        assert!(!avian_backend_rotation_is_valid(DQuat::from_xyzw(
            0.0, 0.0, 0.0, 0.0
        )));
        assert!(avian_backend_pose_is_valid(DVec3::ZERO, DQuat::IDENTITY));
        assert!(avian_backend_aabb_is_valid(DVec3::ZERO, DVec3::ONE));
        assert!(!avian_backend_aabb_is_valid(DVec3::ONE, DVec3::ZERO));
        assert!(!avian_backend_aabb_is_valid(
            DVec3::ZERO,
            DVec3::new(f64::MAX, 0.0, 0.0)
        ));
        assert!(avian_backend_collider_shape_is_valid(&Collider::cuboid(
            1.0, 1.0, 1.0
        )));
        assert!(avian_backend_collider_is_leaf(&Collider::cuboid(
            1.0, 1.0, 1.0
        )));
        assert!(!avian_backend_collider_is_leaf(&Collider::compound(vec![
            (
                DVec3::ZERO,
                bevy::math::Quat::IDENTITY,
                Collider::cuboid(1.0, 1.0, 1.0)
            ),
        ])));
    }
}
