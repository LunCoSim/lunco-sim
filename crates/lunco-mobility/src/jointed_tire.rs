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
    normal: DVec3,
    normal_force: f64,
    forward: DVec3,
    right: DVec3,
    v_long: f64,
    v_lat: f64,
    relative_velocity: DVec3,
}

fn add_external_support_load(
    loads: &mut std::collections::HashMap<Entity, Vec<f64>>,
    island: Entity,
    other_island: Option<Entity>,
    load: f64,
) {
    if other_island != Some(island) && crate::contact_has_positive_load(load) {
        loads.entry(island).or_default().push(load);
    }
}

fn admitted_wheel_mass(
    mass: Option<&ComputedMass>,
    wheel: Entity,
    holds: Option<&mut lunco_physics::PhysicsHolds>,
    faults: Option<&mut lunco_core::RuntimeFaults>,
) -> Option<f64> {
    let value = mass.map(|mass| f64::from(mass.value()));
    if let Some(value) = value.filter(|value| value.is_finite() && *value > 0.0) {
        return Some(value);
    }
    if let Some(holds) = holds {
        holds.set(lunco_physics::PhysicsHolds::SAFETY_FAILURE, true);
    }
    if let Some(faults) = faults {
        faults.raise(
            "physics-wheel-mass-invalid",
            Some(wheel),
            "wheel contact mass",
            "dynamic wheel mass must be finite and positive",
        );
    }
    None
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
            Option<&ComputedMass>,
        ),
        Without<lunco_core::SimulationObjectPaused>,
    >,
    q_joints: Query<(&JointTorqueActuator, &RevoluteJoint)>,
    q_body_modes: Query<(Entity, &RigidBody, Has<RigidBodyDisabled>)>,
    mut q_ports: ParamSet<(Query<&Port>, Query<&mut Port>)>,
    q_inputs: Query<&InputPorts>,
    islands: Res<lunco_physics::DynamicJointIslandMasses>,
    q_gravity: Query<&lunco_physics::kinematics::LocalGravity>,
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
        q_tires.iter().map(|(entity, key, _, _, _)| (entity, key)),
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
    if wheel_order.is_empty() {
        return;
    }
    pending.clear();

    // Apportion each articulated vehicle's mass across all its loaded external
    // contacts, including native wheels and non-wheel support. Internal
    // contacts cannot support their own joint island.
    let mut tire_islands = std::collections::HashSet::new();
    let mut loads: std::collections::HashMap<Entity, Vec<f64>> = std::collections::HashMap::new();
    let is_braking = |motor: &JointTorqueActuator| {
        q_inputs
            .get(motor.control_entity)
            .ok()
            .is_some_and(|inputs| inputs.brake_active)
    };
    for (wheel, _, tire, _, _) in &q_tires {
        if !q_body_modes
            .get(wheel)
            .is_ok_and(|(_, body, disabled)| *body == RigidBody::Dynamic && !disabled)
        {
            continue;
        }
        let Ok((motor, _)) = q_joints.get(tire.drive_joint) else {
            continue;
        };
        if motor.brake_torque <= 0.0 || !is_braking(motor) {
            continue;
        }
        if let Some(island) = islands.get(wheel) {
            tire_islands.insert(island.island);
        }
    }
    if !tire_islands.is_empty() {
        for (body, mode, disabled) in &q_body_modes {
            if *mode != RigidBody::Dynamic || disabled {
                continue;
            }
            let Some(island) = islands.get(body) else {
                continue;
            };
            if !tire_islands.contains(&island.island) {
                continue;
            }
            for pair in collisions.collisions_with(body) {
                let other = if pair.body1 == Some(body) {
                    pair.body2
                } else if pair.body2 == Some(body) {
                    pair.body1
                } else {
                    continue;
                };
                if other.is_some_and(|body| !q_body_modes.contains(body)) {
                    continue;
                }
                let other_island = other
                    .and_then(|body| islands.get(body))
                    .map(|other| other.island);
                for manifold in &pair.manifolds {
                    if crate::contact_normal_for_body(pair, manifold, body).is_none() {
                        continue;
                    }
                    for point in &manifold.points {
                        let load = lunco_physics::contact_force_from_impulse(
                            f64::from(point.normal_impulse),
                            full_dt,
                        );
                        add_external_support_load(&mut loads, island.island, other_island, load);
                    }
                }
            }
        }
    }
    let total_loads: std::collections::HashMap<_, f64> = loads
        .into_iter()
        .map(|(island, mut loads)| {
            loads.sort_by(f64::total_cmp);
            (island, loads.into_iter().sum())
        })
        .collect();

    {
        let q_state = bodies.p1();
        for wheel in wheel_order {
            if !q_body_modes
                .get(wheel)
                .is_ok_and(|(_, body, disabled)| *body == RigidBody::Dynamic && !disabled)
            {
                continue;
            }
            let Ok((_, _, tire, heat_ports, wheel_mass)) = q_tires.get(wheel) else {
                continue;
            };
            let Some(wheel_mass) = admitted_wheel_mass(
                wheel_mass,
                wheel,
                holds.as_deref_mut(),
                faults.as_deref_mut(),
            ) else {
                return;
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
            let braking = is_braking(motor);
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
                        let relative_velocity = wheel_state.velocity_at_point(point.point)
                            - other_velocity_at(other, point.point);
                        contacts.push(TireContact {
                            support: other,
                            point: point.point,
                            normal,
                            normal_force,
                            forward,
                            right,
                            v_long,
                            v_lat,
                            relative_velocity,
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
            let island = islands.get(wheel);
            let mut tire_heat = 0.0;
            for contact in &contacts {
                let static_hold = if braking
                    && motor.brake_torque > drive_torque.abs()
                    && let (Some(island), Ok(gravity)) = (island, q_gravity.get(wheel))
                    && let (Some(mass), Some(total_load)) =
                        (island.mass_kg, total_loads.get(&island.island))
                    && *total_load > 0.0
                    && !contact.support.is_some_and(|body| {
                        islands
                            .get(body)
                            .is_some_and(|other| other.island == island.island)
                    }) {
                    let supported_mass = mass * contact.normal_force / total_load;
                    // The normal-load share carries island gravity. The
                    // correction impulse lands on this wheel, so use its
                    // measured mass instead of the full articulation mass.
                    Some(
                        crate::parking_brake_force_with_velocity(
                            gravity.acceleration(),
                            contact.normal,
                            contact.normal_force,
                            supported_mass,
                            wheel_mass,
                            tire.friction_mu,
                            contact.relative_velocity,
                            full_dt,
                        )
                        .clamp_length_max(
                            (motor.brake_torque - drive_torque.abs()) / tire.radius
                                * contact.normal_force
                                / total_normal_force,
                        ),
                    )
                } else {
                    None
                };
                let mut force = DVec3::ZERO;
                for interval in step.intervals.iter().filter(|s| s.duration > 0.0) {
                    let fraction = interval.duration / full_dt;
                    if let Some(hold) = static_hold
                        && interval.start_speed == 0.0
                        && interval.speed == 0.0
                    {
                        force += hold * fraction;
                        tire_heat -= hold.dot(contact.relative_velocity) * fraction;
                    } else {
                        let (long, lat) = patch(contact, interval.speed);
                        force += (contact.forward * long + contact.right * lat) * fraction;
                        tire_heat += fraction
                            * (long * (interval.speed * tire.radius - contact.v_long)
                                - lat * contact.v_lat);
                    }
                }
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
                        .is_ok_and(|(_, body, disabled)| *body == RigidBody::Dynamic && !disabled)
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
            if let Some(endpoint) = heat_ports.and_then(|heat| heat.tire)
                && let Ok(mut port) = q_ports.p1().get_mut(endpoint)
            {
                port.value = tire_heat;
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

#[cfg(test)]
mod tests {
    use super::{add_external_support_load, admitted_wheel_mass};
    use avian3d::prelude::ComputedMass;
    use bevy::prelude::Entity;
    use std::collections::HashMap;

    #[test]
    fn parking_support_load_includes_native_and_body_contacts_but_excludes_internal_contacts() {
        let rover = Entity::from_raw_u32(1).unwrap();
        let other = Entity::from_raw_u32(2).unwrap();
        let mut loads = HashMap::new();
        // Shared tire, native-material wheel, and chassis support respectively.
        for load in [100.0, 200.0, 100.0] {
            add_external_support_load(&mut loads, rover, None, load);
        }
        // A contact between bodies in one joint island cannot support it.
        add_external_support_load(&mut loads, rover, Some(rover), 500.0);
        add_external_support_load(&mut loads, other, None, 50.0);
        add_external_support_load(&mut loads, rover, None, 0.0);
        assert_eq!(loads[&rover].iter().sum::<f64>(), 400.0);
        assert_eq!(loads[&other].iter().sum::<f64>(), 50.0);
        assert_eq!(100.0 / loads[&rover].iter().sum::<f64>(), 0.25);
    }

    #[test]
    fn parking_invalid_wheel_mass_holds_and_reports_without_panicking() {
        let wheel = Entity::from_raw_u32(3).unwrap();
        let mut holds = lunco_physics::PhysicsHolds::default();
        let mut faults = lunco_core::RuntimeFaults::default();
        assert!(admitted_wheel_mass(None, wheel, Some(&mut holds), Some(&mut faults)).is_none());
        assert!(holds.holds(lunco_physics::PhysicsHolds::SAFETY_FAILURE));
        let fault = faults.first.as_ref().unwrap();
        assert_eq!(fault.kind, "physics-wheel-mass-invalid");
        assert_eq!(fault.entity, Some(wheel));
        assert!(fault.detail.contains("finite and positive"));

        let mass = ComputedMass::new(25.0);
        assert_eq!(
            admitted_wheel_mass(Some(&mass), wheel, None, None),
            Some(25.0)
        );
    }
}
