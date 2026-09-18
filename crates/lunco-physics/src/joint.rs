//! Reusable Avian revolute-joint mechanics.
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

/// The port name a prismatic joint exposes in both directions.
pub const JOINT_DISPLACEMENT_PORT: &str = "displacement";

/// The port name a prismatic joint exposes for its slide rate (m/s).
pub const JOINT_VELOCITY_PORT: &str = "velocity";

/// The port name a prismatic joint exposes for its axial reaction force (N).
pub const JOINT_FORCE_PORT: &str = "force";

/// A solved scalar torque applied across a revolute joint.
///
/// The scalar is a physical torque from an authored runtime port. The joint
/// coordinate sign maps it to equal and opposite world torques on the two
/// bodies; Avian then integrates the bodies and enforces the revolute
/// constraint. This is a reusable mechanical boundary, not a motor model:
/// motors, brakes, hydraulic rotary actuators, and other domains can publish
/// the scalar through the same port surface. The same boundary publishes the
/// solved joint-coordinate speed through `speed_port_entity`.
#[derive(Component, Debug, Clone, Copy, Reflect)]
#[reflect(Component, Default)]
pub struct JointTorqueActuator {
    /// Runtime port carrying solved torque in the joint's positive coordinate.
    pub port_entity: Entity,
    /// Runtime port receiving the measured joint-coordinate speed, rad/s.
    pub speed_port_entity: Entity,
    /// Passive opposing torque when the owning vessel's brake command is active.
    pub brake_torque: f64,
    /// Rotational inertia at the actuator boundary, kg m². It bounds the
    /// braking impulse for one fixed step so a brake cannot integrate through
    /// zero and reverse the joint every tick.
    pub rotational_inertia: f64,
    /// Mapping from the joint coordinate to body-2's world axis.
    pub drive_sign: f64,
}

impl Default for JointTorqueActuator {
    fn default() -> Self {
        Self {
            port_entity: Entity::PLACEHOLDER,
            speed_port_entity: Entity::PLACEHOLDER,
            brake_torque: 0.0,
            rotational_inertia: 0.0,
            drive_sign: 1.0,
        }
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
    (body1_rotation * local_axis).try_normalize()
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
    use super::bounded_brake_torque;

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
