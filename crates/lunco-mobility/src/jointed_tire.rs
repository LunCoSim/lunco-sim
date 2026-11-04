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
use lunco_physics::joint::{JointTorqueActuator, revolute_hinge_axis_world};
use lunco_port_core::InputPorts;
use lunco_port_core::Port;

use crate::wheel_kinematics::body_point_velocity;
use crate::{TireLateralStiffnessGraph, contact_plane_basis, solve_axle_step, tire_patch_force};

/// Authored tire parameters and topology for one Avian-backed wheel body.
///
/// This component is a runtime projection of the composed USD wheel/tire and
/// its authored revolute joint. It contains no alternate coefficients: the
/// same values are fed to [`WheelRaycast`] through `WheelParams` and to this
/// contact realization.
#[derive(Component, Debug, Clone, Copy, Reflect)]
#[reflect(Component)]
pub struct JointedWheelTire {
    /// Authored revolute joint entity connecting the wheel to its chassis.
    pub drive_joint: Entity,
    /// Wheel radius, m.
    pub radius: f64,
    /// Complete authored tire and drivetrain assembly inertia, kg m².
    pub axle_inertia: f64,
    /// Longitudinal force per dimensionless slip, N.
    pub slip_stiffness: f64,
    /// Standard PhysX lateral stiffness graph evaluated at contact load.
    pub lateral_stiffness_graph: TireLateralStiffnessGraph,
    /// Lower edge of the authored measured cornering-speed envelope, m/s.
    pub min_validated_speed: f64,
    /// Standard vehicle-level minimum longitudinal slip denominators, m/s.
    pub longitudinal_slip_denominators: crate::LongitudinalSlipDenominators,
    /// Authored tire Coulomb coefficient.
    pub friction_mu: f64,
    /// Axle bearing damping, N m s.
    pub bearing_damping: f64,
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
        body_point_velocity(
            self.linear_velocity,
            self.angular_velocity,
            lunco_spatial::coords::GridPos(point),
            lunco_spatial::coords::GridPos(self.position),
            lunco_spatial::coords::GridRot(self.rotation),
            self.center_of_mass,
        )
    }
}

#[derive(Clone, Copy)]
struct TireContact {
    /// Native rigid-body support; absent for a fixed world collider.
    support: Option<Entity>,
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
        rotation: rotation.0.as_dquat(),
        linear_velocity: linear.0.as_dvec3(),
        angular_velocity: angular.0.as_dvec3(),
        center_of_mass: center_of_mass.0.as_dvec3(),
    })
}

/// Apply the shared tire law to touching Avian wheel contacts.
///
/// The normal load and delivered contact geometry come from native Avian contacts.
/// This system runs at the fixed-frame boundary, so impulses are converted with
/// the fixed-frame duration rather than a solver-internal substep divisor. The axle
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
        Query<lunco_physics::AvianForceState>,
    )>,
    q_tires: Query<
        (
            Entity,
            Option<&lunco_physics::PhysicsOrderKey>,
            &JointedWheelTire,
            Option<&crate::WheelHeatPorts>,
        ),
        Without<lunco_core::SimulationObjectPaused>,
    >,
    q_joints: Query<(&JointTorqueActuator, &RevoluteJoint)>,
    q_body_modes: Query<(&RigidBody, Has<RigidBodyDisabled>)>,
    mut q_ports: ParamSet<(Query<&Port>, Query<&mut Port>)>,
    q_inputs: Query<&InputPorts>,
    collisions: Collisions,
    mut pending: Local<Vec<lunco_physics::BodyForce>>,
    mut force_scratch: Local<lunco_physics::ForceBatchScratch>,
    mut commands: Commands,
    fixed_time: Res<Time<Fixed>>,
    mut holds: Option<ResMut<lunco_physics::PhysicsHolds>>,
    mut faults: Option<ResMut<lunco_core::RuntimeFaults>>,
    mut physics_time: Option<ResMut<Time<Physics>>>,
) {
    let full_dt = fixed_time.delta_secs_f64();
    if full_dt <= 0.0 {
        return;
    }
    let wheel_order = match lunco_physics::ordered_physics_entities(
        q_tires.iter().map(|(entity, key, _, _)| (entity, key)),
        "jointed wheel tire force",
    ) {
        Ok(order) => order,
        Err(invalid) => {
            lunco_physics::report_invalid_physics_order(
                holds.as_deref_mut(),
                faults.as_deref_mut(),
                physics_time.as_deref_mut(),
                "physics-wheel-order-invalid",
                &invalid,
            );
            return;
        }
    };
    pending.clear();

    {
        let q_state = bodies.p1();
        for wheel in wheel_order {
            if !q_body_modes
                .get(wheel)
                .is_ok_and(|(body, disabled)| *body == RigidBody::Dynamic && !disabled)
            {
                continue;
            }
            let Ok((_, _, tire, heat_ports)) = q_tires.get(wheel) else {
                continue;
            };
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
            let Some(frame1) = joint.local_basis1() else {
                continue;
            };
            let Some(axle_world) = revolute_hinge_axis_world(joint, chassis_state.rotation) else {
                continue;
            };
            let heading_world = chassis_state.rotation * frame1.as_dquat() * tire.heading_local;
            let omega = (wheel_state.angular_velocity - chassis_state.angular_velocity)
                .dot(axle_world)
                * motor.coordinate_sign();
            let drive_torque = match q_ports.p0().get(motor.torque_endpoint()) {
                Ok(port) => port.value,
                Err(_) => continue,
            };
            let braking = q_inputs
                .get(motor.control_entity)
                .ok()
                .is_some_and(|inputs| inputs.brake_active);
            let other_velocity_at = |other: Option<Entity>, point: DVec3| {
                other
                    .and_then(|entity| body_state(&q_state, entity))
                    .map(|state| state.velocity_at_point(point))
                    .unwrap_or(DVec3::ZERO)
            };

            let mut contacts = Vec::new();
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
                // A retained contact sample cannot keep a retired rigid-body
                // lease alive until native collision cleanup runs. Bodyless
                // static colliders remain valid fixed-world supports.
                if other.is_some_and(|body| !q_body_modes.contains(body)) {
                    continue;
                }
                for manifold in &pair.manifolds {
                    // The contact normal points from collider1 to collider2. Flip
                    // it when the wheel is collider/body1 so the tire sees the
                    // ground-to-wheel support normal.
                    let Some(normal) = crate::contact_normal_for_body(pair, manifold, wheel) else {
                        continue;
                    };
                    let (forward, right) = contact_plane_basis(heading_world, axle_world, normal);
                    for point in &manifold.points {
                        // Native impulse already counts delivered momentum once;
                        // the completed master interval converts it to load.
                        let normal_force = lunco_physics::contact_force_from_impulse(
                            f64::from(point.normal_impulse),
                            full_dt,
                        );
                        if !crate::contact_has_positive_load(normal_force) {
                            continue;
                        }
                        // A jointed tire's translational slip is measured at the
                        // wheel hub. The wheel's axial spin is represented exactly
                        // once by `omega` below; measuring at the contact point
                        // would feed suspension/steering angular motion back into
                        // lateral slip. The resulting force still acts at Avian's
                        // saved contact point, so the lever arm remains physical.
                        let contact_velocity = wheel_state.velocity_at_point(wheel_state.position)
                            - other_velocity_at(other, wheel_state.position);
                        let v_long = contact_velocity.dot(forward);
                        let v_lat = contact_velocity.dot(right);
                        contacts.push(TireContact {
                            support: other,
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
            // Collision graph insertion order depends on collider admission.
            // Sort the complete physical contact tuple before summing loads or
            // applying forces to the shared wheel body.
            contacts.sort_by(|a, b| {
                let values = |c: &TireContact| {
                    [
                        c.point.x,
                        c.point.y,
                        c.point.z,
                        c.forward.x,
                        c.forward.y,
                        c.forward.z,
                        c.right.x,
                        c.right.y,
                        c.right.z,
                        c.normal_force,
                        c.v_long,
                        c.v_lat,
                    ]
                };
                values(a)
                    .into_iter()
                    .zip(values(b))
                    .map(|(a, b)| a.total_cmp(&b))
                    .find(|order| !order.is_eq())
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let total_normal_force = contacts.iter().map(|c| c.normal_force).sum::<f64>();

            if let Some(endpoint) = heat_ports.and_then(|ports| ports.contact) {
                if let Ok(mut port) = q_ports.p1().get_mut(endpoint) {
                    port.value = if total_normal_force > 0.0 && !contacts.is_empty() {
                        1.0
                    } else {
                        0.0
                    };
                }
            }
            if total_normal_force <= 0.0 || contacts.is_empty() {
                if let Some(endpoint) = heat_ports.and_then(|ports| ports.tire) {
                    if let Ok(mut port) = q_ports.p1().get_mut(endpoint) {
                        port.value = 0.0;
                    }
                }
                continue;
            }

            let patch = |contact: &TireContact, w: f64| {
                tire_patch_force(
                    tire.slip_stiffness * contact.normal_force / total_normal_force,
                    w * tire.radius - contact.v_long,
                    contact.v_long.abs().max((w * tire.radius).abs()).max(
                        tire.longitudinal_slip_denominators
                            .minimum(drive_torque != 0.0 || (braking && motor.brake_torque > 0.0)),
                    ),
                    contact
                        .v_long
                        .abs()
                        .max((w * tire.radius).abs())
                        .max(tire.min_validated_speed),
                    contact.v_lat,
                    contact.normal_force,
                    tire.friction_mu,
                    tire.lateral_stiffness_graph,
                )
            };
            let step = match solve_axle_step(
                omega,
                tire.axle_inertia,
                tire.bearing_damping,
                drive_torque,
                if braking { motor.brake_torque } else { 0.0 },
                tire.radius * tire.friction_mu * total_normal_force,
                full_dt,
                |w| {
                    contacts
                        .iter()
                        .map(|contact| patch(contact, w).0 * tire.radius)
                        .sum()
                },
            ) {
                Ok(step) => step,
                Err(detail) => {
                    if let Some(holds) = holds.as_deref_mut() {
                        holds.set(lunco_physics::PhysicsHolds::SAFETY_FAILURE, true);
                    }
                    if let Some(faults) = faults.as_deref_mut() {
                        faults.raise(
                            "physics-wheel-step-invalid",
                            Some(wheel),
                            "wheel mechanical step",
                            detail,
                        );
                    }
                    return;
                }
            };
            let tire_heat = contacts
                .iter()
                .map(|contact| {
                    step.intervals
                        .iter()
                        .filter(|s| s.duration > 0.0)
                        .map(|interval| {
                            let (long, lat) = patch(contact, interval.speed);
                            interval.duration / full_dt
                                * (long * (interval.speed * tire.radius - contact.v_long)
                                    - lat * contact.v_lat)
                        })
                        .sum::<f64>()
                })
                .sum::<f64>();
            if let Some(heat) = heat_ports {
                if let Some(endpoint) = heat.tire {
                    if let Ok(mut port) = q_ports.p1().get_mut(endpoint) {
                        port.value = tire_heat;
                    }
                }
            }
            for contact in &contacts {
                let (f_long, f_lat) = step.intervals.iter().filter(|s| s.duration > 0.0).fold(
                    (0.0, 0.0),
                    |(long, lat), interval| {
                        let (f_long, f_lat) = patch(contact, interval.speed);
                        (
                            long + interval.duration / full_dt * f_long,
                            lat + interval.duration / full_dt * f_lat,
                        )
                    },
                );
                let force = contact.forward * f_long + contact.right * f_lat;
                // A zero-torque tire patch is a persistent contact reaction,
                // not a wake-up event. Preserve the command bit with the
                // pending force so a real drive or brake command can wake a
                // sleeping wheel island through Avian's regular accumulator.
                pending.push(lunco_physics::BodyForce {
                    local_force: DVec3::ZERO,
                    body: wheel,
                    force,
                    point: Some(lunco_spatial::coords::GridPos(contact.point)),
                    torque: DVec3::ZERO,
                    wake: drive_torque != 0.0 || braking,
                });
                // A dynamic support receives the opposite tangential force at
                // the identical world point. Static, kinematic and disabled
                // supports prescribe their motion and exchange momentum with
                // that external constraint rather than a native accumulator.
                if let Some(support) = contact.support
                    && q_body_modes
                        .get(support)
                        .is_ok_and(|(body, disabled)| *body == RigidBody::Dynamic && !disabled)
                {
                    pending.push(lunco_physics::BodyForce {
                        local_force: DVec3::ZERO,
                        body: support,
                        force: -force,
                        point: Some(lunco_spatial::coords::GridPos(contact.point)),
                        torque: DVec3::ZERO,
                        wake: drive_torque != 0.0 || braking,
                    });
                }
            }
        }
    }

    if let Err(error) =
        lunco_physics::prepare_force_batch(&bodies.p2(), &pending, &mut force_scratch)
    {
        crate::report_wheel_force_error(
            &mut commands,
            error,
            holds.as_deref_mut(),
            faults.as_deref_mut(),
            physics_time.as_deref_mut(),
        );
        return;
    }
    if let Err(error) = lunco_physics::apply_force_batch(&mut bodies.p0(), &pending) {
        crate::report_wheel_force_error(
            &mut commands,
            error,
            holds.as_deref_mut(),
            faults.as_deref_mut(),
            physics_time.as_deref_mut(),
        );
    }
}
