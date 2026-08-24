//! Shared tire-contact realization for Avian-backed wheel bodies.
//!
//! A jointed wheel still uses Avian for normal contact, penetration, and the
//! chassis↔wheel revolute constraint.  Its tangential force is calculated here
//! with the same longitudinal and slip-angle laws as a raycast wheel.  The USD
//! Avian bridge removes Avian's generic tangent impulse for bodies carrying the
//! shared-contact marker, so this system is the sole tire-force owner.

use avian3d::prelude::*;
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use lunco_core::architecture::Port;
use lunco_core::InputPorts;
use lunco_cosim::JointTorqueActuator;

use crate::{
    contact_plane_basis, longitudinal_tire_step, tire_patch_force, TireLateralStiffnessGraph,
};

/// Authored tire parameters and topology for one Avian-backed wheel body.
///
/// This component is a runtime projection of the composed USD wheel/tire and
/// its synthesized revolute motor.  It contains no alternate coefficients: the
/// same values are fed to [`WheelRaycast`] through `WheelParams` and to this
/// contact realization.
#[derive(Component, Debug, Clone, Copy, Reflect)]
#[reflect(Component)]
pub struct JointedWheelTire {
    /// Synthesized revolute motor entity connecting the wheel to its chassis.
    pub drive_joint: Entity,
    /// Wheel radius, m.
    pub radius: f64,
    /// Complete authored tire and drivetrain assembly inertia, kg m².
    pub axle_inertia: f64,
    /// Longitudinal slip stiffness, N/m.
    pub slip_stiffness: f64,
    /// Standard PhysX lateral stiffness graph evaluated at contact load.
    pub lateral_stiffness_graph: TireLateralStiffnessGraph,
    /// Lower edge of the authored measured cornering-speed envelope, m/s.
    pub min_validated_speed: f64,
    /// Authored tire Coulomb coefficient.
    pub friction_mu: f64,
    /// Axle bearing damping, N m s.
    pub bearing_damping: f64,
    /// Axle axis in the chassis-local joint frame.
    pub axle_axis_local: DVec3,
    /// Wheel heading in the chassis-local joint frame before steering.
    pub heading_local: DVec3,
}

#[derive(Clone, Copy)]
struct BodyState {
    position: DVec3,
    rotation: DQuat,
    linear_velocity: DVec3,
    angular_velocity: DVec3,
    center_of_mass: DVec3,
}

impl BodyState {
    fn velocity_at_point(self, point: DVec3) -> DVec3 {
        let center = self.position + self.rotation * self.center_of_mass;
        self.linear_velocity + self.angular_velocity.cross(point - center)
    }
}

#[derive(Clone, Copy)]
struct TireContact {
    point: DVec3,
    normal_force: f64,
    forward: DVec3,
    right: DVec3,
    v_long: f64,
    v_lat: f64,
}

fn body_state(
    q: &Query<(
        &Position,
        &Rotation,
        &LinearVelocity,
        &AngularVelocity,
        &ComputedCenterOfMass,
    )>,
    entity: Entity,
) -> Option<BodyState> {
    let Ok((position, rotation, linear, angular, center_of_mass)) = q.get(entity) else {
        return None;
    };
    Some(BodyState {
        position: position.0,
        rotation: rotation.0,
        linear_velocity: linear.0,
        angular_velocity: angular.0,
        center_of_mass: center_of_mass.0,
    })
}

/// Apply the shared tire law to touching Avian wheel contacts.
///
/// Avian stores the accumulated normal impulse on each contact point;
/// that is the normal-load measurement used here.  This system runs at the
/// fixed-frame boundary, so the accumulated impulse is converted with the
/// fixed-frame duration rather than a solver-internal substep divisor. The axle
/// solve runs once per
/// wheel against the aggregate load, then its force is distributed over the
/// contact points by load share.  This avoids solving the motor torque once per
/// manifold point while retaining each point's normal and slip angle.
///
/// This is one master-tick exchange. Avian's `PhysicsSchedule`, which runs
/// after the co-simulation actuation boundary, distributes the resulting force
/// over its internal solver substeps. This system must not run a second
/// substep loop or advance a Modelica participant inside one.
pub fn apply_jointed_tire_forces(
    mut bodies: ParamSet<(
        Query<Forces>,
        Query<(
            &Position,
            &Rotation,
            &LinearVelocity,
            &AngularVelocity,
            &ComputedCenterOfMass,
        )>,
    )>,
    q_tires: Query<(Entity, &JointedWheelTire)>,
    q_joints: Query<(&JointTorqueActuator, &RevoluteJoint)>,
    q_ports: Query<&Port>,
    q_inputs: Query<&InputPorts>,
    q_child_of: Query<&ChildOf>,
    collisions: Collisions,
    fixed_time: Res<Time<Fixed>>,
) {
    let full_dt = fixed_time.delta_secs_f64();
    if full_dt <= 0.0 {
        return;
    }

    let mut pending: Vec<(Entity, DVec3, DVec3)> = Vec::new();

    {
        let q_state = bodies.p1();
        for (wheel, tire) in &q_tires {
            let Ok((motor, joint)) = q_joints.get(tire.drive_joint) else {
                continue;
            };
            if joint.body2 != wheel {
                // The USD projection's wheel joint contract is chassis=body1,
                // wheel=body2. A malformed or unrelated joint must not receive a
                // guessed force direction.
                continue;
            }
            let Some(wheel_state) = body_state(&q_state, wheel) else {
                continue;
            };
            let Some(chassis_state) = body_state(&q_state, joint.body1) else {
                continue;
            };

            let frame1 = joint.local_basis1().unwrap_or(DQuat::IDENTITY);
            let axle_world = (chassis_state.rotation * frame1 * tire.axle_axis_local)
                .try_normalize()
                .unwrap_or(DVec3::X);
            let heading_world = chassis_state.rotation * frame1 * tire.heading_local;
            let omega = (wheel_state.angular_velocity - chassis_state.angular_velocity)
                .dot(axle_world)
                * motor.drive_sign;
            let Ok(port) = q_ports.get(motor.port_entity) else {
                continue;
            };
            let braking =
                lunco_core::architecture::owning_input_ports(wheel, &q_child_of, &q_inputs)
                    .is_some_and(|inputs| inputs.brake_active);
            let brake_torque = if braking {
                -omega.signum() * motor.brake_torque
            } else {
                0.0
            };
            let axle_torque = port.value + brake_torque;

            let hub = wheel_state.position;
            let other_hub_velocity = |other: Option<Entity>| {
                other
                    .and_then(|entity| body_state(&q_state, entity))
                    .map(|state| state.velocity_at_point(hub))
                    .unwrap_or(DVec3::ZERO)
            };

            let mut contacts = Vec::new();
            let mut total_normal_force = 0.0;
            for pair in collisions.collisions_with(wheel) {
                let wheel_is_body1 = pair.body1 == Some(wheel);
                let wheel_is_body2 = pair.body2 == Some(wheel);
                if !wheel_is_body1 && !wheel_is_body2 {
                    continue;
                }
                let other = if wheel_is_body1 {
                    pair.body2
                } else {
                    pair.body1
                };
                let hub_velocity = wheel_state.velocity_at_point(hub) - other_hub_velocity(other);
                for manifold in &pair.manifolds {
                    // The contact normal points from collider1 to collider2. Flip
                    // it when the wheel is collider/body1 so the tire sees the
                    // ground-to-wheel support normal.
                    let normal = if wheel_is_body1 {
                        -manifold.normal
                    } else {
                        manifold.normal
                    };
                    let (forward, right) = contact_plane_basis(heading_world, axle_world, normal);
                    for point in &manifold.points {
                        let normal_force = lunco_physics::contact_force_from_impulse(
                            point.normal_impulse,
                            full_dt,
                        );
                        if normal_force <= 0.0 {
                            continue;
                        }
                        let v_long = hub_velocity.dot(forward);
                        let v_lat = hub_velocity.dot(right);
                        total_normal_force += normal_force;
                        contacts.push(TireContact {
                            point: point.point,
                            normal_force,
                            forward,
                            right,
                            v_long,
                            v_lat,
                        });
                    }
                }
            }
            if total_normal_force <= 0.0 || contacts.is_empty() {
                continue;
            }

            let weighted_v_long = contacts
                .iter()
                .map(|contact| contact.v_long * contact.normal_force)
                .sum::<f64>()
                / total_normal_force;
            let (_, f_long) = longitudinal_tire_step(
                omega,
                weighted_v_long,
                tire.radius,
                tire.axle_inertia,
                tire.slip_stiffness,
                tire.bearing_damping,
                axle_torque,
                0.0,
                total_normal_force,
                tire.friction_mu,
                full_dt,
            );
            for contact in contacts {
                let load_share = contact.normal_force / total_normal_force;
                let f_long = f_long * load_share;
                let rolling_reference = contact
                    .v_long
                    .abs()
                    .max((omega * tire.radius).abs())
                    .max(tire.min_validated_speed);
                let (_, f_lat) = tire_patch_force(
                    f_long,
                    rolling_reference,
                    contact.v_lat,
                    contact.normal_force,
                    tire.friction_mu,
                    tire.lateral_stiffness_graph,
                );
                let force = contact.forward * f_long + contact.right * f_lat;
                pending.push((wheel, force, contact.point));
            }
        }
    }

    let mut q_forces = bodies.p0();
    for (wheel, force, point) in pending {
        if let Ok(mut forces) = q_forces.get_mut(wheel) {
            forces.apply_force_at_point(force, point);
        }
    }
}
