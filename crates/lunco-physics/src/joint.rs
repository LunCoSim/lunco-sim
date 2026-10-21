//! Reusable joint mechanics at the Avian boundary.
//!
//! This module contains the physical actuator description and small mechanical
//! projections shared by the USD bridge, mobility realization, and the
//! co-simulation force application system. It does not own port resolution or
//! command transport.

use avian3d::prelude::RevoluteJoint;
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;

/// The port name a revolute joint exposes in both directions.
pub const JOINT_ANGLE_PORT: &str = "angle";

/// The port name a revolute joint exposes for its measured coordinate rate.
pub const JOINT_ANGULAR_VELOCITY_PORT: &str = "angular_velocity";

/// The port name a prismatic joint exposes in both directions.
pub const JOINT_DISPLACEMENT_PORT: &str = "displacement";

/// The port name a prismatic joint exposes for its slide rate (m/s).
pub const JOINT_VELOCITY_PORT: &str = "velocity";

/// The port name a prismatic joint exposes for its completed net force on body2 along the slider axis (N).
pub const JOINT_FORCE_PORT: &str = "force";

/// Completed torque on body2 along a revolute joint's positive hinge axis (N m).
pub const JOINT_TORQUE_PORT: &str = "torque";

/// A solved scalar torque applied across a revolute joint.
///
/// The scalar is a physical torque from an authored runtime port. The joint
/// coordinate sign maps it to equal and opposite world torques on the two
/// bodies; Avian then integrates the bodies and enforces the revolute
/// constraint. This is a reusable mechanical boundary, not a motor model:
/// motors, brakes, hydraulic rotary actuators, and other domains can publish
/// the scalar through the same port surface. The same boundary publishes the
/// solved joint-coordinate speed through its speed endpoint.
#[derive(Component, Debug, Clone, Copy, Reflect)]
#[reflect(Component)]
pub struct JointTorqueActuator {
    /// Runtime port carrying solved torque in the joint's positive coordinate.
    port_entity: Entity,
    /// Runtime port receiving the measured joint-coordinate speed, rad/s.
    speed_port_entity: Entity,
    /// Authored command-surface owner. Mechanical and transform ancestry may
    /// differ, especially for bodies with a USD transform-stack reset.
    pub control_entity: Entity,
    /// Passive opposing torque when the owning vessel's brake command is active.
    pub brake_torque: f64,
    /// Viscous axle friction relative to the housing, N m s/rad.
    pub bearing_damping: f64,
    /// Optional physical heat-rate outputs (W), owned by the projected joint's wheel.
    pub bearing_heat_port: Option<Entity>,
    pub brake_heat_port: Option<Entity>,
    /// Rotational inertia at the actuator boundary, kg m². It bounds the
    /// braking impulse for one fixed step so a brake cannot integrate through
    /// zero and reverse the joint every tick.
    pub rotational_inertia: f64,
    /// Mapping from the joint coordinate to body-2's world axis.
    sign: JointCoordinateSign,
}

/// Orientation of the external effort/motion pair relative to the native joint.
/// The same sign acts on torque and speed, preserving their power product.
#[derive(Debug, Clone, Copy, Reflect)]
pub enum JointCoordinateSign {
    Positive,
    Negative,
}

impl JointCoordinateSign {
    pub const fn factor(self) -> f64 {
        match self {
            Self::Positive => 1.0,
            Self::Negative => -1.0,
        }
    }
}

impl JointTorqueActuator {
    /// Create a conjugate torque/speed boundary with no additional passive load.
    /// Endpoints must be distinct live scalar ports with coherent SI contracts;
    /// the co-simulation owner checks those contracts before applying effort.
    pub fn new(
        torque_endpoint: Entity,
        speed_endpoint: Entity,
        control_entity: Entity,
        sign: JointCoordinateSign,
    ) -> Result<Self, String> {
        let actuator = Self {
            port_entity: torque_endpoint,
            speed_port_entity: speed_endpoint,
            control_entity,
            brake_torque: 0.0,
            bearing_damping: 0.0,
            bearing_heat_port: None,
            brake_heat_port: None,
            rotational_inertia: 0.0,
            sign,
        };
        actuator.validate_parameters()?;
        Ok(actuator)
    }

    pub const fn torque_endpoint(&self) -> Entity {
        self.port_entity
    }
    pub const fn speed_endpoint(&self) -> Entity {
        self.speed_port_entity
    }
    pub const fn coordinate_sign(&self) -> f64 {
        self.sign.factor()
    }

    /// Structural invariant shared by runtime admission and authoring diagnostics.
    pub fn validate_parameters(&self) -> Result<(), String> {
        if self.port_entity == Entity::PLACEHOLDER
            || self.speed_port_entity == Entity::PLACEHOLDER
            || self.port_entity == self.speed_port_entity
        {
            return Err("external effort requires distinct torque and motion endpoints".into());
        }
        let heat = [self.bearing_heat_port, self.brake_heat_port];
        if heat.into_iter().flatten().any(|endpoint| {
            endpoint == Entity::PLACEHOLDER
                || endpoint == self.port_entity
                || endpoint == self.speed_port_entity
        }) || self.bearing_heat_port.is_some() && self.bearing_heat_port == self.brake_heat_port
        {
            return Err(
                "heat endpoints must be distinct from each other and the effort/motion pair".into(),
            );
        }
        if !self.brake_torque.is_finite()
            || self.brake_torque < 0.0
            || !self.bearing_damping.is_finite()
            || self.bearing_damping < 0.0
            || !self.rotational_inertia.is_finite()
            || self.rotational_inertia < 0.0
            || (self.brake_torque > 0.0 && self.rotational_inertia == 0.0)
        {
            return Err("external effort requires finite nonnegative passive parameters and positive inertia when braking".into());
        }
        Ok(())
    }

    pub fn validate_joint(
        &self,
        joint: Option<&RevoluteJoint>,
        disabled: bool,
    ) -> Result<(), String> {
        self.validate_parameters()?;
        let Some(joint) = joint else {
            return Err("external torque requires a revolute joint".into());
        };
        if disabled || joint.motor.enabled {
            return Err(
                "external effort requires an enabled joint with its native motor disabled".into(),
            );
        }
        if joint.body1 == joint.body2 {
            return Err("external effort requires two distinct bodies".into());
        }
        Ok(())
    }
}

/// Return the world-space axis of a revolute joint's positive coordinate.
///
/// Avian defines the coordinate axis in the joint frame: `hinge_axis` is
/// rotated by `frame1.basis` before it is expressed in body 1's world
/// orientation. Keeping that projection here makes every mechanical boundary
/// use the solver's actual axis, including a runtime steering change to the
/// joint frame.
pub fn revolute_hinge_axis_world(joint: &RevoluteJoint, body1_rotation: DQuat) -> Option<DVec3> {
    let local_axis = joint.local_hinge_axis1()?;
    (body1_rotation * local_axis.as_dvec3()).try_normalize()
}

/// Measure the signed revolute coordinate and its positive world-space axis.
///
/// Both authored joint bases participate in the measurement. The perpendicular
/// frame vectors follow Avian's motor convention; `atan2` retains f64 precision
/// for small angles. Unresolved or invalid rigid frames have no measurement.
pub fn revolute_coordinate(
    joint: &RevoluteJoint,
    body1_rotation: DQuat,
    body2_rotation: DQuat,
) -> Option<(f64, DVec3)> {
    let basis1 = joint.local_basis1()?.as_dquat();
    let basis2 = joint.local_basis2()?.as_dquat();
    if ![body1_rotation, body2_rotation, basis1, basis2]
        .into_iter()
        .all(crate::avian_backend_rotation_is_valid)
        || !joint.hinge_axis.is_finite()
        || !joint.hinge_axis.is_normalized()
    {
        return None;
    }
    angular_frame_coordinate(
        body1_rotation * basis1,
        body2_rotation * basis2,
        joint.hinge_axis.as_dvec3(),
    )
}

pub(crate) fn angular_frame_coordinate(
    frame1: DQuat,
    frame2: DQuat,
    local_axis: DVec3,
) -> Option<(f64, DVec3)> {
    if ![frame1, frame2]
        .into_iter()
        .all(crate::avian_backend_rotation_is_valid)
        || !local_axis.is_finite()
        || !local_axis.is_normalized()
    {
        return None;
    }
    let axis = (frame1 * local_axis).try_normalize()?;
    let perpendicular = local_axis.any_orthonormal_vector();
    let first = frame1 * perpendicular;
    let second = frame2 * perpendicular;
    let angle = first.cross(second).dot(axis).atan2(first.dot(second));
    angle.is_finite().then_some((angle, axis))
}

/// Return the first revolute joint in `root`'s subtree.
pub fn joint_angle_holder(world: &World, root: Entity) -> Option<Entity> {
    let mut stack = vec![root];
    while let Some(entity) = stack.pop() {
        if world.get::<RevoluteJoint>(entity).is_some() {
            return Some(entity);
        }
        if let Some(children) = world.get::<Children>(entity) {
            stack.extend(children.iter());
        }
    }
    None
}

/// Resolve the physical brake torque for one fixed step.
///
/// The authored value is a torque limit. Near zero speed, applying that full
/// limit for an entire step would overshoot the stop and reverse the joint;
/// the impulse needed to reach zero is the physically valid bound for this
/// discrete projection.
#[inline]
pub fn bounded_brake_torque(
    max_torque: f64,
    rotational_inertia: f64,
    coordinate_speed: f64,
    dt: f64,
) -> f64 {
    if !max_torque.is_finite()
        || max_torque <= 0.0
        || !rotational_inertia.is_finite()
        || rotational_inertia <= 0.0
        || !coordinate_speed.is_finite()
        || !dt.is_finite()
        || dt <= 0.0
    {
        return 0.0;
    }
    -coordinate_speed.signum() * max_torque.min(rotational_inertia * coordinate_speed.abs() / dt)
}

#[cfg(test)]
mod tests {
    use super::{bounded_brake_torque, revolute_coordinate};
    use avian3d::prelude::RevoluteJoint;
    use bevy::{
        math::{DQuat, DVec3, EulerRot},
        prelude::*,
    };

    #[test]
    fn effort_pair_requires_distinct_endpoints_and_exclusive_native_authority() {
        use super::{JointCoordinateSign, JointTorqueActuator};
        let mut world = World::new();
        let endpoints = [world.spawn_empty().id(), world.spawn_empty().id()];
        assert!(
            JointTorqueActuator::new(
                endpoints[0],
                endpoints[0],
                endpoints[0],
                JointCoordinateSign::Positive
            )
            .is_err()
        );
        assert!(
            JointTorqueActuator::new(
                Entity::PLACEHOLDER,
                endpoints[1],
                endpoints[0],
                JointCoordinateSign::Positive
            )
            .is_err()
        );
        let coupling = JointTorqueActuator::new(
            endpoints[0],
            endpoints[1],
            endpoints[0],
            JointCoordinateSign::Negative,
        )
        .unwrap();
        assert_eq!(coupling.coordinate_sign(), -1.0);
        let mut joint = RevoluteJoint::new(endpoints[0], endpoints[1]);
        assert!(coupling.validate_joint(Some(&joint), false).is_ok());
        joint.motor.enabled = true;
        assert!(
            coupling
                .validate_joint(Some(&joint), false)
                .unwrap_err()
                .contains("native motor")
        );
        joint.motor.enabled = false;
        assert!(coupling.validate_joint(Some(&joint), true).is_err());
        assert!(coupling.validate_joint(None, false).is_err());
    }

    #[test]
    fn revolute_coordinate_uses_both_joint_frames() {
        let mut world = World::new();
        let first = world.spawn_empty().id();
        let second = world.spawn_empty().id();
        let basis1 = DQuat::from_rotation_z(0.4);
        let basis2 = DQuat::from_rotation_x(-0.6);
        let body1 = DQuat::from_euler(EulerRot::XYZ, 0.2, -0.7, 0.3);
        let joint = RevoluteJoint::new(first, second)
            .with_hinge_axis(Vec3::Y)
            .with_local_basis1(basis1.as_quat())
            .with_local_basis2(basis2.as_quat());
        let basis1 = joint.local_basis1().unwrap().as_dquat();
        let basis2 = joint.local_basis2().unwrap().as_dquat();
        let expected_axis = (body1 * basis1 * DVec3::Y).normalize();
        for angle in [0.0, 2e-10, -2e-10, 0.3, -0.3, 3.0, -3.0] {
            let body2 = body1 * basis1 * DQuat::from_rotation_y(angle) * basis2.inverse();
            for sign in [-1.0, 1.0] {
                let (measured, axis) = revolute_coordinate(&joint, body1, body2 * sign).unwrap();
                assert!((measured - angle).abs() < 2e-15, "{measured} {angle}");
                assert!((axis - expected_axis).length() < 1e-15);
            }
        }
        for invalid in [
            DQuat::NAN,
            DQuat::from_xyzw(0.0, 0.0, 0.0, 0.0),
            body1 * 2.0,
        ] {
            assert!(revolute_coordinate(&joint, invalid, body1).is_none());
            assert!(revolute_coordinate(&joint, body1, invalid).is_none());
        }
        let invalid_axis = joint.clone().with_hinge_axis(Vec3::ZERO);
        assert!(revolute_coordinate(&invalid_axis, body1, body1).is_none());
    }

    #[test]
    fn bounded_brake_torque_stops_without_reversing_in_one_step() {
        let torque = bounded_brake_torque(100.0, 2.0, 0.25, 0.1);
        assert_eq!(torque, -5.0);

        let torque = bounded_brake_torque(100.0, 2.0, -0.25, 0.1);
        assert_eq!(torque, 5.0);
    }

    #[test]
    fn bounded_brake_torque_respects_authored_limit_away_from_zero() {
        let torque = bounded_brake_torque(100.0, 2.0, 20.0, 0.1);
        assert_eq!(torque, -100.0);
    }

    #[test]
    fn bounded_brake_torque_rejects_invalid_boundary_inputs() {
        for (max_torque, inertia, speed, dt) in [
            (0.0, 2.0, 1.0, 0.1),
            (100.0, 0.0, 1.0, 0.1),
            (100.0, 2.0, f64::NAN, 0.1),
            (100.0, 2.0, 1.0, 0.0),
        ] {
            assert_eq!(bounded_brake_torque(max_torque, inertia, speed, dt), 0.0);
        }
    }
}
