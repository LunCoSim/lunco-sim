//! Reusable joint mechanics at the Avian boundary.
//!
//! This module contains the physical actuator description and small mechanical
//! projections shared by the USD bridge, mobility realization, and the
//! co-simulation force application system. It does not own port resolution or
//! command transport.

use avian3d::prelude::{AngularMotor, MotorModel, RevoluteJoint};
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;

/// A changed drive setpoint is an external input to the constrained island.
/// Wake its dynamic endpoints so Avian's solver can consume the new motor target.
pub fn wake_sleeping_joint_bodies(world: &mut World, bodies: [Entity; 2]) {
    use avian3d::prelude::{RigidBody, Sleeping};
    for body in bodies {
        let sleeping_dynamic = world
            .get::<RigidBody>(body)
            .is_some_and(RigidBody::is_dynamic)
            && world.get::<Sleeping>(body).is_some();
        if sleeping_dynamic
            && let Err(error) = <avian3d::dynamics::solver::islands::WakeBody as Command>::apply(
                avian3d::dynamics::solver::islands::WakeBody(body),
                world,
            )
        {
            warn!(
                "[physics] joint motor changed but dynamic body {body:?} could not be woken: {error}"
            );
        }
    }
}

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
    /// Authored rotational inertia at the actuator boundary, kg m². The native
    /// brake solve uses the bodies' admitted inertias and joint/contact rows.
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
            || self.brake_torque > f64::from(f32::MAX)
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

    /// Native zero-speed brake. The largest finite implicit damping realizes
    /// the velocity-constraint limit; authored torque remains the hard bound.
    /// Narrowing is explicit here at Avian's f32 motor boundary and checked by
    /// validate_parameters, never applied to canonical authored values.
    pub fn passive_brake_motor(&self) -> AngularMotor {
        AngularMotor::new(MotorModel::AccelerationBased {
            stiffness: 0.0,
            damping: f32::MAX,
        })
        .with_target_velocity(0.0)
        .with_max_torque(self.brake_torque as f32)
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
        let owned_brake = self.brake_torque > 0.0 && joint.motor == self.passive_brake_motor();
        if disabled || (joint.motor.enabled && !owned_brake) {
            return Err(
                "external effort requires an enabled joint without a competing native motor".into(),
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

#[cfg(test)]
mod tests {
    use super::revolute_coordinate;
    use avian3d::prelude::RevoluteJoint;
    use bevy::{
        math::{DQuat, DVec3, EulerRot},
        prelude::*,
    };

    #[test]
    fn native_zero_speed_motor_holds_a_loaded_roller_on_ground() {
        use avian3d::prelude::*;
        use bevy::time::TimeUpdateStrategy;
        use std::time::Duration;
        let mut app = App::new();
        app.init_resource::<Assets<Mesh>>()
            .add_message::<AssetEvent<Mesh>>()
            .add_plugins((MinimalPlugins, PhysicsPlugins::default()))
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
                1.0 / 60.0,
            )))
            .insert_resource(Gravity(Vec3::new(0.0, -1.62, 0.2)));
        app.finish();
        app.cleanup();
        app.world_mut().resource_mut::<SubstepCount>().0 = 32;
        app.world_mut()
            .resource_mut::<avian3d::dynamics::solver::xpbd::JointVelocitySolverConfig>()
            .iterations = core::num::NonZeroU8::new(128).unwrap();
        app.world_mut().spawn((
            RigidBody::Static,
            Collider::cuboid(100.0, 1.0, 100.0),
            Transform::from_xyz(0.0, -0.5, 0.0),
            Friction::new(0.6),
        ));
        let carrier = app
            .world_mut()
            .spawn((
                RigidBody::Dynamic,
                NoAutoMass,
                Mass(100.0),
                // A translating axle test rig supplies an attitude reaction;
                // a single top-heavy wheel/frame pair would tip as a rigid body.
                LockedAxes::ROTATION_LOCKED,
                Collider::sphere(0.1),
                Transform::from_xyz(0.0, 1.0, 0.0),
                SleepingDisabled,
            ))
            .id();
        let roller = app
            .world_mut()
            .spawn((
                RigidBody::Dynamic,
                NoAutoMass,
                Mass(20.0),
                AngularInertia::new(Vec3::splat(2.0)),
                Collider::sphere(0.5),
                Transform::from_xyz(0.0, 0.5, 0.0),
                Friction::new(0.6),
                SleepingDisabled,
            ))
            .id();
        let mut actuator = super::JointTorqueActuator::new(
            carrier,
            roller,
            carrier,
            super::JointCoordinateSign::Positive,
        )
        .unwrap();
        actuator.brake_torque = 1500.0;
        actuator.rotational_inertia = 2.0;
        let hinge = app
            .world_mut()
            .spawn((
                RevoluteJoint::new(carrier, roller)
                    .with_hinge_axis(Vec3::X)
                    .with_anchor(bevy::math::DVec3::new(0.0, 0.5, 0.0))
                    .with_motor(actuator.passive_brake_motor()),
                JointCollisionDisabled,
            ))
            .id();
        for _ in 0..120 {
            app.update();
        }
        let start = app.world().get::<Position>(carrier).unwrap().0;
        for _ in 0..600 {
            app.update();
        }
        let end = app.world().get::<Position>(carrier).unwrap().0;
        let omega = app.world().get::<AngularVelocity>(roller).unwrap().0;
        println!(
            "LOADED_ROLLER drift_m={} omega={omega:?}",
            (end - start).length()
        );
        assert!(
            (end - start).length() < 0.001,
            "a braked roller must stay within one millimetre over ten seconds"
        );
        app.world_mut()
            .get_mut::<RevoluteJoint>(hinge)
            .unwrap()
            .motor
            .enabled = false;
        for _ in 0..180 {
            app.update();
        }
        let released = app.world().get::<Position>(carrier).unwrap().0;
        println!("RELEASED_ROLLER distance_m={}", (released - end).length());
        assert!(
            (released - end).length() > 0.05,
            "the same unfrozen rig rolls when its brake is released"
        );
    }

    #[test]
    fn native_velocity_motor_keeps_one_torque_budget_across_contact_passes() {
        use avian3d::prelude::*;
        use bevy::time::TimeUpdateStrategy;
        use std::time::Duration;
        for (target, cap) in [(1.0, 1.0), (-1.0, 1.0), (1.0, 10.0)] {
            let mut app = App::new();
            app.init_resource::<Assets<Mesh>>()
                .add_message::<AssetEvent<Mesh>>()
                .add_plugins((MinimalPlugins, PhysicsPlugins::default()))
                .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
                    1.0 / 60.0,
                )))
                .insert_resource(Gravity(Vec3::ZERO));
            app.finish();
            app.cleanup();
            app.world_mut().resource_mut::<SubstepCount>().0 = 32;
            app.world_mut()
                .resource_mut::<avian3d::dynamics::solver::xpbd::JointVelocitySolverConfig>()
                .iterations = core::num::NonZeroU8::new(128).unwrap();
            let frame = app
                .world_mut()
                .spawn((RigidBody::Static, Transform::default()))
                .id();
            let rotor = app
                .world_mut()
                .spawn((
                    RigidBody::Dynamic,
                    NoAutoMass,
                    Mass(1.0),
                    NoAutoAngularInertia,
                    AngularInertia::new(Vec3::splat(2.0)),
                    Transform::default(),
                    SleepingDisabled,
                ))
                .id();
            let hinge = app
                .world_mut()
                .spawn((
                    RevoluteJoint::new(frame, rotor)
                        .with_hinge_axis(Vec3::X)
                        .with_motor(
                            AngularMotor::new(MotorModel::ForceBased {
                                stiffness: 0.0,
                                damping: 200.0,
                            })
                            .with_target_velocity(target)
                            .with_max_torque(cap),
                        ),
                    JointForces::default(),
                ))
                .id();
            let mut maximum: f32 = 0.0;
            for _ in 0..30 {
                app.update();
                maximum = maximum.max(
                    app.world()
                        .get::<JointForces>(hinge)
                        .unwrap()
                        .motor_force()
                        .abs(),
                );
            }
            assert!(
                maximum <= cap * 1.0001,
                "native effort exceeded its torque cap: {maximum} > {cap}"
            );
            assert!(
                maximum > cap * 0.1,
                "the native motor must actually transmit effort"
            );
            assert!(app.world().get::<AngularVelocity>(rotor).unwrap().x * target > 0.0);
        }
    }

    #[test]
    fn external_effort_accepts_only_its_capped_passive_native_brake() {
        use super::{JointCoordinateSign, JointTorqueActuator};
        let mut world = bevy::prelude::World::new();
        let body1 = world.spawn_empty().id();
        let body2 = world.spawn_empty().id();
        let mut actuator =
            JointTorqueActuator::new(body1, body2, body1, JointCoordinateSign::Positive).unwrap();
        actuator.brake_torque = 1500.0;
        actuator.rotational_inertia = 2.0;
        let mut joint = RevoluteJoint::new(body1, body2).with_motor(actuator.passive_brake_motor());
        assert!(actuator.validate_joint(Some(&joint), false).is_ok());
        assert_eq!(joint.motor.max_torque, 1500.0);
        joint.motor.target_velocity = 1.0;
        assert!(actuator.validate_joint(Some(&joint), false).is_err());
        joint.motor = actuator.passive_brake_motor();
        joint.motor.max_torque = 1501.0;
        assert!(actuator.validate_joint(Some(&joint), false).is_err());
        actuator.brake_torque = f64::INFINITY;
        assert!(actuator.validate_parameters().is_err());
    }

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
}
