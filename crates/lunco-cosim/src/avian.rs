//! Avian rigid bodies exposed as co-simulation ports (the **body** half of the
//! avian backend; [`crate::joint`] is the joint half).
//!
//! Avian's components are foreign types we don't own, so they are exposed
//! through a declarative port spec ([`crate::ports::AvianGroup`]) rather than a
//! mirror component per kind. A rigid body publishes its full kinematic state —
//! position, linear velocity, attitude (`quat_*` + `yaw`/`pitch`/`roll`), and
//! body rates (`angvel_*`) — as read-only outputs, and accepts world/body-frame
//! forces and torques as inputs. Authored kinematic bodies additionally accept
//! position inputs; dynamic bodies never do. All address avian's own `Position` /
//! `Rotation` / `LinearVelocity` / `AngularVelocity` / `Forces` directly, with
//! no `HashMap` mirror and no per-tick sync system to keep a copy in step.
//!
//! Force inputs are an **additive sink**: the wire write lands in
//! [`PendingForces`] (the propagation master has already summed all wires into
//! that one value), and the single generic [`apply_pending_forces`] system
//! applies it through avian's query-shaped `Forces` API and clears it each tick.
//! That one system is the only per-tick avian system left.

use avian3d::prelude::{
    AngularInertia, AngularVelocity, CenterOfMass, Collider, ColliderMassProperties,
    ComputedAngularInertia, ComputedCenterOfMass, ComputedMass, Forces, LinearVelocity, Mass,
    NoAutoAngularInertia, NoAutoCenterOfMass, NoAutoMass, Physics, Position, RevoluteJoint,
    RigidBody, Rotation, Sleeping, WriteRigidBodyForces,
};
use bevy::math::DVec3;
use bevy::prelude::*;
use std::hash::{Hash, Hasher};

use crate::ports::{AvianGroup, AvianPort, AvianPortContract};
use lunco_cosim_core::{ForceActuator, TorqueActuator};
use lunco_physics::joint::{JointTorqueActuator, bounded_brake_torque, revolute_hinge_axis_world};
use lunco_port_core::ports::PortDirection;

/// Per-entity force accumulator written by `force_*` input ports and drained
/// into avian each physics tick by [`apply_pending_forces`].
///
/// Replaces the old `AvianSim.inputs` mirror map. A wire to `force_y` sets `f.y`
/// (already summed across wires by the propagation master); next tick the
/// summed value is rewritten. Inserted lazily on the first force write, so a
/// body that is never force-driven never carries it.
#[derive(Component, Debug, Clone, Copy, Default, Reflect)]
#[reflect(Component, Default)]
pub struct PendingForces {
    /// World-space linear force (N) to apply this tick.
    pub f: DVec3,
    /// Body-frame linear force (N): rotated into world by avian's
    /// `apply_local_force` at apply time. Use for thrust that follows the
    /// vehicle's attitude (gimbaled engine, RCS, body-fixed thruster).
    pub f_local: DVec3,
    /// World-space torque (N·m) to apply this tick (e.g. reaction wheel,
    /// thrust-vector moment expressed in world frame).
    pub torque: DVec3,
}

/// The solved linear acceleration seen by a rigid-body-mounted accelerometer.
///
/// Avian exposes velocity as a native solver fact, but not an accelerometer
/// reading.  The sample is captured after physics writeback and is therefore
/// the finite-difference kinematics of the solved body, including gravity,
/// thrust, contacts, and joints.  The Modelica IMU converts that navigation
/// frame quantity into specific force; no controller or scene reads this
/// component directly.
#[derive(Component, Debug, Clone, Copy, Default, Reflect)]
#[reflect(Component, Default)]
pub struct SolvedLinearAcceleration {
    /// Previous solved world-frame velocity, used for the next sample.
    previous_velocity: DVec3,
    /// Current solved world-frame acceleration, m/s².
    pub value: DVec3,
    /// False until a complete velocity interval has been observed.
    pub valid: bool,
}

/// Add the raw accelerometer state to every Avian body once it exists.
pub fn ensure_acceleration_samples(
    mut commands: Commands,
    query: Query<
        (Entity, &LinearVelocity),
        (
            With<RigidBody>,
            With<lunco_core::PhysicsStateReady>,
            Without<SolvedLinearAcceleration>,
        ),
    >,
) {
    let samples = query
        .iter()
        .map(|(entity, velocity)| {
            (
                entity,
                SolvedLinearAcceleration {
                    previous_velocity: velocity.0,
                    value: DVec3::ZERO,
                    valid: false,
                },
            )
        })
        .collect::<Vec<_>>();
    if samples.is_empty() {
        return;
    }
    // Bodies become physics-ready in admission batches. Insert their identical
    // initialization state in one fallible command while retaining query order.
    commands.try_insert_batch(samples);
}

/// Capture solved acceleration after Avian has written back its state.
pub fn sample_solved_acceleration(
    time: Res<Time<Physics>>,
    mut query: Query<
        (&LinearVelocity, &mut SolvedLinearAcceleration),
        (With<RigidBody>, With<lunco_core::PhysicsStateReady>),
    >,
) {
    let dt = time.delta_secs_f64();
    if !dt.is_finite() || dt <= 0.0 {
        return;
    }
    for (velocity, mut sample) in &mut query {
        let current = velocity.0;
        if sample.valid {
            sample.value = (current - sample.previous_velocity) / dt;
        } else {
            sample.value = DVec3::ZERO;
            sample.valid = true;
        }
        sample.previous_velocity = current;
    }
}

/// Apply solved joint torque as equal and opposite body torques.
///
/// The Modelica/network solver determines the scalar.  This system performs
/// only the mechanical boundary projection; it does not derive torque from a
/// command, speed, battery state, motor name, or fixed torque-speed curve.
pub fn apply_joint_torque_actuators(
    physics_time: Res<Time<Physics>>,
    virtual_time: Res<Time<Virtual>>,
    faults: Option<Res<lunco_core::RuntimeFaults>>,
    q_actuators: Query<(&JointTorqueActuator, &RevoluteJoint)>,
    mut q_ports: ParamSet<(
        Query<&lunco_port_core::Port>,
        Query<&mut lunco_port_core::Port>,
    )>,
    q_sleeping: Query<(), With<Sleeping>>,
    q_child_of: Query<&ChildOf>,
    q_inputs: Query<&lunco_port_core::InputPorts>,
    mut bodies: ParamSet<(
        Query<(&Rotation, &AngularVelocity)>,
        Query<Forces, lunco_physics::Integrable>,
    )>,
    mut commands: Commands,
) {
    if !lunco_physics::physics_is_live_state(&physics_time, &virtual_time, faults.as_deref()) {
        return;
    }

    for (actuator, joint) in &q_actuators {
        let torque = {
            let q_read_ports = q_ports.p0();
            let Ok(port) = q_read_ports.get(actuator.port_entity) else {
                continue;
            };
            port.value
        };
        if !torque.is_finite()
            || !actuator.brake_torque.is_finite()
            || !actuator.rotational_inertia.is_finite()
            || !actuator.drive_sign.is_finite()
        {
            continue;
        }
        let (axis, coordinate_speed) = {
            let q_bodies = bodies.p0();
            let Ok([(body1_rotation, body1_omega), (_, body2_omega)]) =
                q_bodies.get_many([joint.body1, joint.body2])
            else {
                continue;
            };
            let Some(axis) = revolute_hinge_axis_world(joint, body1_rotation.0) else {
                continue;
            };
            let coordinate_speed = (body2_omega.0 - body1_omega.0).dot(axis) * actuator.drive_sign;
            (axis, coordinate_speed)
        };
        if let Ok(mut speed_port) = q_ports.p1().get_mut(actuator.speed_port_entity) {
            speed_port.value = coordinate_speed;
        }
        let brake = if lunco_port_core::owning_input_ports(joint.body2, &q_child_of, &q_inputs)
            .is_some_and(|inputs| inputs.brake_active)
        {
            bounded_brake_torque(
                actuator.brake_torque,
                actuator.rotational_inertia,
                coordinate_speed,
                physics_time.delta_secs_f64(),
            )
        } else {
            0.0
        };
        let coordinate_torque = torque + brake;
        if coordinate_torque == 0.0 {
            continue;
        }
        let world_torque = axis * (coordinate_torque * actuator.drive_sign);
        let mut q_forces = bodies.p1();
        if let Ok([mut body1_forces, mut body2_forces]) =
            q_forces.get_many_mut([joint.body1, joint.body2])
        {
            body1_forces.apply_torque(-world_torque);
            body2_forces.apply_torque(world_torque);
        } else {
            if let Ok(mut body1_forces) = q_forces.get_mut(joint.body1) {
                body1_forces.apply_torque(-world_torque);
            }
            if let Ok(mut body2_forces) = q_forces.get_mut(joint.body2) {
                body2_forces.apply_torque(world_torque);
            }
        }
        if q_sleeping.contains(joint.body1) {
            commands.queue(avian3d::dynamics::solver::islands::WakeBody(joint.body1));
        }
        if q_sleeping.contains(joint.body2) {
            commands.queue(avian3d::dynamics::solver::islands::WakeBody(joint.body2));
        }
    }
}

/// One tick's command for a [`ForceActuator`] or [`TorqueActuator`].
#[derive(Component, Debug, Clone, Copy, Default, Reflect)]
#[reflect(Component, Default)]
pub struct PendingActuatorCommand {
    /// Commanded magnitude in the actuator's physical unit. The actuator
    /// clamps it to its authored limit before Avian integration.
    pub value: f64,
}

/// Reused per-system scratch for the deterministic actuator transaction. The
/// physics boundary must sort commands by authored identity, but allocating a
/// fresh vector and cloning `Name` strings on every fixed step would turn the
/// determinism fence into a frame-time tax.
#[derive(Default)]
pub struct ActuatorCommandScratch {
    force: Vec<(Option<u64>, u64, u64, Entity, ForceActuator, f64)>,
    torque: Vec<(Option<u64>, u64, u64, Entity, TorqueActuator, f64)>,
}

/// Stable, allocation-free FNV-1a hash for a local authored name. Networked
/// USD entities sort by `GlobalEntityId`; this is only the deterministic local
/// fallback for an authored entity that has not entered the identity registry.
fn stable_name_hash(name: Option<&Name>) -> u64 {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;
    name.map_or(0, |name| {
        name.as_str().bytes().fold(OFFSET, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(PRIME)
        })
    })
}

/// Ensure `entity` carries [`PendingForces`], then mutate it. The `force_*`
/// write closures use this so an un-driven body stays clean until first written.
fn with_pending(world: &mut World, entity: Entity, set: impl FnOnce(&mut PendingForces)) {
    let mut em = world
        .get_entity_mut(entity)
        .expect("prepared Avian force input still has its owning entity");
    if !em.contains::<PendingForces>() {
        em.insert(PendingForces::default());
    }
    let mut pending = em
        .get_mut::<PendingForces>()
        .expect("PendingForces was inserted before applying the port write");
    set(&mut pending);
}

/// Ensure an actuator command exists, then update it from the port write.
fn with_pending_actuator_command(world: &mut World, entity: Entity, value: f64) {
    let mut em = world
        .get_entity_mut(entity)
        .expect("prepared actuator command still has its owning entity");
    if !em.contains::<PendingActuatorCommand>() {
        em.insert(PendingActuatorCommand::default());
    }
    em.get_mut::<PendingActuatorCommand>()
        .expect("PendingActuatorCommand was inserted before applying the port write")
        .value = value;
}

/// Contact as a PHYSICS fact, on any collider — no instrument required.
///
/// Gated on [`Collider`] for the same reason the rigid-body group is gated on
/// [`RigidBody`]: a collider that is being pushed on has a contact force whether
/// or not anyone authored a sensor to notice, exactly as a body has a velocity
/// whether or not anyone authored a speedometer.
///
/// This is the layer a PHYSICAL PART reads — a structure, a damper, a mount takes
/// the load it is actually carrying from here, because it carries that load
/// whether or not anyone authored an instrument to notice. Gating a part's own
/// behaviour behind an instrument would mean hardware that responds only if
/// someone remembered to install a switch.
///
/// Flight software reads these primitive contact facts through an authored
/// Modelica conversion when it needs a touchdown signal. Both answers come
/// from [`lunco_physics::contact_of`].
///
/// Read on demand from the contact graph — no mirror component, no per-tick sync
/// system, matching every other port in this module.
pub const COLLIDER_CONTACT_GROUP: AvianGroup = AvianGroup {
    source: "Avian contact solver",
    present: |w, e| w.get::<Collider>(e).is_some(),
    entities: |world, out| {
        out.extend(world.query_filtered::<Entity, With<Collider>>().iter(world));
    },
    topology_key: collider_contact_topology_key,
    ports: &[
        AvianPort {
            name: "contact",
            contract: AvianPortContract::DIMENSIONLESS,
            dir: PortDirection::Out,
            read: Some(|w, e| {
                Some(if lunco_physics::contact_from_world(w, e).0 {
                    1.0
                } else {
                    0.0
                })
            }),
            write: None,
        },
        AvianPort {
            name: "contact_force",
            contract: AvianPortContract::FORCE,
            dir: PortDirection::Out,
            read: Some(|w, e| Some(lunco_physics::contact_from_world(w, e).1)),
            write: None,
        },
        // This SHAPE's own mass (kg), as physics computes it from the geometry and
        // its density — `UsdPhysicsMassAPI`'s `physics:mass`, or `physics:density`
        // times the volume of the shape USD authored.
        //
        // Here so a part's model can ASK for its mass instead of restating it. A
        // strut's spring-damper needs the mass it is accelerating, and that number
        // was hand-typed into the Modelica input on all four legs — a physical
        // property duplicated beside the physics that owns it, free to drift the
        // moment the geometry changed. Wire `inputs:m_strut.connect` to this and
        // there is one mass, in the one place UsdPhysics puts it.
        AvianPort {
            name: "mass",
            contract: AvianPortContract::MASS,
            dir: PortDirection::Out,
            read: Some(|w, e| w.get::<ColliderMassProperties>(e).map(|m| m.mass as f64)),
            write: None,
        },
    ],
    install_topology: register_collider_contact_topology,
};

fn register_collider_contact_topology(app: &mut App) {
    app.add_observer(lunco_port_core::ports::bump_port_topology_on_add::<Collider>)
        .add_observer(lunco_port_core::ports::bump_port_topology_on_remove::<Collider>)
        .add_observer(lunco_port_core::ports::bump_port_topology_on_add::<ColliderMassProperties>)
        .add_observer(
            lunco_port_core::ports::bump_port_topology_on_remove::<ColliderMassProperties>,
        );
}

fn collider_contact_topology_key(world: &World, entity: Entity) -> u64 {
    let Some(_) = world.get::<Collider>(entity) else {
        return 0;
    };
    1 | (u64::from(world.get::<ColliderMassProperties>(entity).is_some()) << 1)
}

/// A USD-authored force actuator. Its command is scalar force; position and
/// direction are structural facts read from the USD prim.
pub const FORCE_ACTUATOR_GROUP: AvianGroup = AvianGroup {
    source: "Avian force actuator",
    present: |w, e| w.get::<ForceActuator>(e).is_some(),
    entities: |world, out| {
        out.extend(
            world
                .query_filtered::<Entity, With<ForceActuator>>()
                .iter(world),
        );
    },
    topology_key: |world, entity| {
        world
            .get::<ForceActuator>(entity)
            .map(force_actuator_topology_key)
            .unwrap_or(0)
    },
    ports: &[AvianPort {
        name: "force_command",
        contract: AvianPortContract::FORCE_ACTUATOR,
        dir: PortDirection::In,
        read: Some(|w, e| {
            w.get::<PendingActuatorCommand>(e)
                .map(|command| command.value)
        }),
        write: Some(with_pending_actuator_command),
    }],
    install_topology: register_force_actuator_topology,
};

fn register_force_actuator_topology(app: &mut App) {
    app.add_observer(lunco_port_core::ports::bump_port_topology_on_add::<ForceActuator>)
        .add_observer(lunco_port_core::ports::bump_port_topology_on_remove::<ForceActuator>)
        .add_systems(PostUpdate, check_force_actuator_structure);
}

fn force_actuator_topology_key(actuator: &ForceActuator) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    actuator.max_force_n.to_bits().hash(&mut hasher);
    hasher.finish()
}

fn check_force_actuator_structure(
    changed: Query<(Entity, &ForceActuator), Changed<ForceActuator>>,
    mut state: ResMut<lunco_port_core::ports::PortTopologyState>,
    mut revision: ResMut<lunco_port_core::ports::PortTopologyRevision>,
) {
    for (entity, actuator) in &changed {
        if state.changed::<ForceActuator>(entity, force_actuator_topology_key(actuator)) {
            revision.bump();
        }
    }
}

/// A USD-authored torque actuator. Its command is scalar torque; its axis and
/// limit are structural facts read from the USD prim.
pub const TORQUE_ACTUATOR_GROUP: AvianGroup = AvianGroup {
    source: "Avian torque actuator",
    present: |w, e| w.get::<TorqueActuator>(e).is_some(),
    entities: |world, out| {
        out.extend(
            world
                .query_filtered::<Entity, With<TorqueActuator>>()
                .iter(world),
        );
    },
    topology_key: |world, entity| {
        world
            .get::<TorqueActuator>(entity)
            .map(torque_actuator_topology_key)
            .unwrap_or(0)
    },
    ports: &[AvianPort {
        name: "torque_command",
        contract: AvianPortContract::TORQUE_ACTUATOR,
        dir: PortDirection::In,
        read: Some(|w, e| {
            w.get::<PendingActuatorCommand>(e)
                .map(|command| command.value)
        }),
        write: Some(with_pending_actuator_command),
    }],
    install_topology: register_torque_actuator_topology,
};

fn register_torque_actuator_topology(app: &mut App) {
    app.add_observer(lunco_port_core::ports::bump_port_topology_on_add::<TorqueActuator>)
        .add_observer(lunco_port_core::ports::bump_port_topology_on_remove::<TorqueActuator>)
        .add_systems(PostUpdate, check_torque_actuator_structure);
}

fn torque_actuator_topology_key(actuator: &TorqueActuator) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    actuator.max_torque_nm.to_bits().hash(&mut hasher);
    hasher.finish()
}

fn check_torque_actuator_structure(
    changed: Query<(Entity, &TorqueActuator), Changed<TorqueActuator>>,
    mut state: ResMut<lunco_port_core::ports::PortTopologyState>,
    mut revision: ResMut<lunco_port_core::ports::PortTopologyRevision>,
) {
    for (entity, actuator) in &changed {
        if state.changed::<TorqueActuator>(entity, torque_actuator_topology_key(actuator)) {
            revision.bump();
        }
    }
}

/// The rigid-body port group: position/velocity outputs + force inputs.
///
/// Gated on [`RigidBody`] presence. Position ports resolve from [`Position`]
/// (present on every body); velocity ports from [`LinearVelocity`] (dynamic
/// bodies only — absent on a kinematic body, so those ports simply don't list).
pub const RIGID_BODY_GROUP: AvianGroup = AvianGroup {
    source: "Avian rigid body",
    present: |w, e| w.get::<RigidBody>(e).is_some(),
    entities: |world, out| {
        out.extend(
            world
                .query_filtered::<Entity, With<RigidBody>>()
                .iter(world),
        );
    },
    topology_key: rigid_body_topology_key,
    ports: &[
        AvianPort {
            name: "position_x",
            contract: AvianPortContract::LENGTH_WORLD,
            dir: PortDirection::Out,
            read: Some(|w, e| w.get::<Position>(e).map(|p| p.0.x)),
            write: None,
        },
        AvianPort {
            name: "position_y",
            contract: AvianPortContract::LENGTH_WORLD,
            dir: PortDirection::Out,
            read: Some(|w, e| w.get::<Position>(e).map(|p| p.0.y)),
            write: None,
        },
        AvianPort {
            name: "position_z",
            contract: AvianPortContract::LENGTH_WORLD,
            dir: PortDirection::Out,
            read: Some(|w, e| w.get::<Position>(e).map(|p| p.0.z)),
            write: None,
        },
        AvianPort {
            name: "velocity_x",
            contract: AvianPortContract::SPEED_WORLD,
            dir: PortDirection::Out,
            read: Some(|w, e| w.get::<LinearVelocity>(e).map(|v| v.0.x)),
            write: None,
        },
        AvianPort {
            name: "velocity_y",
            contract: AvianPortContract::SPEED_WORLD,
            dir: PortDirection::Out,
            read: Some(|w, e| w.get::<LinearVelocity>(e).map(|v| v.0.y)),
            write: None,
        },
        AvianPort {
            name: "velocity_z",
            contract: AvianPortContract::SPEED_WORLD,
            dir: PortDirection::Out,
            read: Some(|w, e| w.get::<LinearVelocity>(e).map(|v| v.0.z)),
            write: None,
        },
        // Solved navigation-frame acceleration for a rigid-body-mounted
        // accelerometer. The sample is captured after physics writeback, so it
        // includes every native force and constraint that changed the body.
        AvianPort {
            name: "acceleration_x",
            contract: AvianPortContract::ACCELERATION_WORLD,
            dir: PortDirection::Out,
            read: Some(|w, e| {
                w.get::<SolvedLinearAcceleration>(e)
                    .map(|sample| if sample.valid { sample.value.x } else { 0.0 })
                    .or(Some(0.0))
            }),
            write: None,
        },
        AvianPort {
            name: "acceleration_y",
            contract: AvianPortContract::ACCELERATION_WORLD,
            dir: PortDirection::Out,
            read: Some(|w, e| {
                w.get::<SolvedLinearAcceleration>(e)
                    .map(|sample| if sample.valid { sample.value.y } else { 0.0 })
                    .or(Some(0.0))
            }),
            write: None,
        },
        AvianPort {
            name: "acceleration_z",
            contract: AvianPortContract::ACCELERATION_WORLD,
            dir: PortDirection::Out,
            read: Some(|w, e| {
                w.get::<SolvedLinearAcceleration>(e)
                    .map(|sample| if sample.valid { sample.value.z } else { 0.0 })
                    .or(Some(0.0))
            }),
            write: None,
        },
        AvianPort {
            name: "acceleration_valid",
            contract: AvianPortContract::DIMENSIONLESS,
            dir: PortDirection::Out,
            read: Some(|w, e| {
                Some(
                    if w.get::<SolvedLinearAcceleration>(e)
                        .is_some_and(|sample| sample.valid)
                    {
                        1.0
                    } else {
                        0.0
                    },
                )
            }),
            write: None,
        },
        // Readiness is a separate causal fact from the existence of the body
        // port. During USD scene admission a dynamic body is intentionally
        // represented as kinematic, so its required velocity component reads
        // zero until its authored release state has been installed.
        AvianPort {
            name: "state_valid",
            contract: AvianPortContract::DIMENSIONLESS,
            dir: PortDirection::Out,
            read: Some(|w, e| {
                Some(if w.get::<lunco_core::PhysicsStateReady>(e).is_some() {
                    1.0
                } else {
                    0.0
                })
            }),
            write: None,
        },
        // Ground speed — the MAGNITUDE of the linear velocity, frame-free.
        //
        // The per-axis ports are world-frame, so "how fast is this rover going"
        // is not any one of them: a vehicle driving north reads its whole speed
        // on `velocity_z` and zero once it turns. Every consumer that wanted a
        // speedometer (telemetry channel, HUD, a model's drag term) was左 to
        // recompute the magnitude from three ports it had to wire separately.
        AvianPort {
            name: "speed",
            contract: AvianPortContract::SPEED,
            dir: PortDirection::Out,
            read: Some(|w, e| w.get::<LinearVelocity>(e).map(|v| v.0.length())),
            write: None,
        },
        // Attitude as a quaternion (canonical, gimbal-safe). Avian's `Rotation`
        // wraps a `DQuat` in the f64 build. Read-only — write attitude via torque.
        AvianPort {
            name: "quat_w",
            contract: AvianPortContract::ROTATION_WORLD,
            dir: PortDirection::Out,
            read: Some(|w, e| w.get::<Rotation>(e).map(|r| r.0.w)),
            write: None,
        },
        AvianPort {
            name: "quat_x",
            contract: AvianPortContract::ROTATION_WORLD,
            dir: PortDirection::Out,
            read: Some(|w, e| w.get::<Rotation>(e).map(|r| r.0.x)),
            write: None,
        },
        AvianPort {
            name: "quat_y",
            contract: AvianPortContract::ROTATION_WORLD,
            dir: PortDirection::Out,
            read: Some(|w, e| w.get::<Rotation>(e).map(|r| r.0.y)),
            write: None,
        },
        AvianPort {
            name: "quat_z",
            contract: AvianPortContract::ROTATION_WORLD,
            dir: PortDirection::Out,
            read: Some(|w, e| w.get::<Rotation>(e).map(|r| r.0.z)),
            write: None,
        },
        // Euler convenience (radians). Order `YXZ` → (yaw, pitch, roll) for a
        // Y-up world: yaw about world Y, then pitch about X, then roll about Z.
        // Derived from `Rotation`; control laws that want body rates read `angvel_*`.
        AvianPort {
            name: "yaw",
            contract: AvianPortContract::ANGLE_WORLD,
            dir: PortDirection::Out,
            read: Some(|w, e| {
                w.get::<Rotation>(e)
                    .map(|r| r.0.to_euler(bevy::math::EulerRot::YXZ).0)
            }),
            write: None,
        },
        AvianPort {
            name: "pitch",
            contract: AvianPortContract::ANGLE_WORLD,
            dir: PortDirection::Out,
            read: Some(|w, e| {
                w.get::<Rotation>(e)
                    .map(|r| r.0.to_euler(bevy::math::EulerRot::YXZ).1)
            }),
            write: None,
        },
        AvianPort {
            name: "roll",
            contract: AvianPortContract::ANGLE_WORLD,
            dir: PortDirection::Out,
            read: Some(|w, e| {
                w.get::<Rotation>(e)
                    .map(|r| r.0.to_euler(bevy::math::EulerRot::YXZ).2)
            }),
            write: None,
        },
        // Body rates (world-frame angular velocity, rad/s). Pairs with the
        // `torque_*` inputs to close an attitude/spin-damping loop.
        AvianPort {
            name: "angvel_x",
            contract: AvianPortContract::ANGULAR_SPEED_WORLD,
            dir: PortDirection::Out,
            read: Some(|w, e| w.get::<AngularVelocity>(e).map(|v| v.0.x)),
            write: None,
        },
        AvianPort {
            name: "angvel_y",
            contract: AvianPortContract::ANGULAR_SPEED_WORLD,
            dir: PortDirection::Out,
            read: Some(|w, e| w.get::<AngularVelocity>(e).map(|v| v.0.y)),
            write: None,
        },
        AvianPort {
            name: "angvel_z",
            contract: AvianPortContract::ANGULAR_SPEED_WORLD,
            dir: PortDirection::Out,
            read: Some(|w, e| w.get::<AngularVelocity>(e).map(|v| v.0.z)),
            write: None,
        },
        // Force inputs: additive sink into `PendingForces`. Reading returns the
        // value pending this tick (0 once applied/cleared).
        AvianPort {
            name: "force_x",
            contract: AvianPortContract::FORCE_WORLD,
            dir: PortDirection::In,
            read: Some(|w, e| w.get::<PendingForces>(e).map(|forces| forces.f.x)),
            write: Some(|w, e, v| with_pending(w, e, |pf| pf.f.x = v)),
        },
        AvianPort {
            name: "force_y",
            contract: AvianPortContract::FORCE_WORLD,
            dir: PortDirection::In,
            read: Some(|w, e| w.get::<PendingForces>(e).map(|forces| forces.f.y)),
            write: Some(|w, e, v| with_pending(w, e, |pf| pf.f.y = v)),
        },
        AvianPort {
            name: "force_z",
            contract: AvianPortContract::FORCE_WORLD,
            dir: PortDirection::In,
            read: Some(|w, e| w.get::<PendingForces>(e).map(|forces| forces.f.z)),
            write: Some(|w, e, v| with_pending(w, e, |pf| pf.f.z = v)),
        },
        // Body-frame force inputs: rotated into world by the body's attitude at
        // apply time (`apply_local_force`). Thrust along the vehicle's own axes.
        AvianPort {
            name: "force_local_x",
            contract: AvianPortContract::FORCE_BODY,
            dir: PortDirection::In,
            read: Some(|w, e| w.get::<PendingForces>(e).map(|forces| forces.f_local.x)),
            write: Some(|w, e, v| with_pending(w, e, |pf| pf.f_local.x = v)),
        },
        AvianPort {
            name: "force_local_y",
            contract: AvianPortContract::FORCE_BODY,
            dir: PortDirection::In,
            read: Some(|w, e| w.get::<PendingForces>(e).map(|forces| forces.f_local.y)),
            write: Some(|w, e, v| with_pending(w, e, |pf| pf.f_local.y = v)),
        },
        AvianPort {
            name: "force_local_z",
            contract: AvianPortContract::FORCE_BODY,
            dir: PortDirection::In,
            read: Some(|w, e| w.get::<PendingForces>(e).map(|forces| forces.f_local.z)),
            write: Some(|w, e, v| with_pending(w, e, |pf| pf.f_local.z = v)),
        },
        // World-space torque inputs (N·m): reaction wheels, thrust-vector moment.
        AvianPort {
            name: "torque_x",
            contract: AvianPortContract::TORQUE_WORLD,
            dir: PortDirection::In,
            read: Some(|w, e| w.get::<PendingForces>(e).map(|forces| forces.torque.x)),
            write: Some(|w, e, v| with_pending(w, e, |pf| pf.torque.x = v)),
        },
        AvianPort {
            name: "torque_y",
            contract: AvianPortContract::TORQUE_WORLD,
            dir: PortDirection::In,
            read: Some(|w, e| w.get::<PendingForces>(e).map(|forces| forces.torque.y)),
            write: Some(|w, e, v| with_pending(w, e, |pf| pf.torque.y = v)),
        },
        AvianPort {
            name: "torque_z",
            contract: AvianPortContract::TORQUE_WORLD,
            dir: PortDirection::In,
            read: Some(|w, e| w.get::<PendingForces>(e).map(|forces| forces.torque.z)),
            write: Some(|w, e, v| with_pending(w, e, |pf| pf.torque.z = v)),
        },
        // Mass properties (read+write). The triple moves together — propellant
        // burn lightens mass, shifts COM, and shrinks inertia — so a Modelica
        // tank model (or a script, or a wire) can keep all three consistent
        // through the one port surface. See [`write_mass`] for the avian write
        // contract (native `Computed*` values + `NoAuto*` markers).
        AvianPort {
            name: "mass",
            contract: AvianPortContract::MASS,
            dir: PortDirection::InOut,
            read: Some(read_mass),
            write: Some(write_mass),
        },
        // A controller that commands forces on one rigid-body member of an
        // articulated vehicle must use the total translational mass of the
        // active dynamic joint island. This is a derived read-only fact and
        // never changes the owning body's own `mass` input/output above.
        AvianPort {
            name: "dynamic_joint_island_mass_kg",
            contract: AvianPortContract::MASS,
            dir: PortDirection::Out,
            read: Some(|world, entity| {
                world
                    .get_resource::<lunco_physics::DynamicJointIslandMasses>()
                    .and_then(|masses| masses.get(entity))
                    .and_then(|sample| sample.mass_kg)
            }),
            write: None,
        },
        // Missing/nonfinite member mass invalidates the whole island. Consumers
        // use this explicit status to inhibit control until the exact complete
        // mass sample is available.
        AvianPort {
            name: "dynamic_joint_island_mass_valid",
            contract: AvianPortContract::DIMENSIONLESS,
            dir: PortDirection::Out,
            read: Some(|world, entity| {
                Some(
                    if world
                        .get_resource::<lunco_physics::DynamicJointIslandMasses>()
                        .and_then(|masses| masses.get(entity))
                        .is_some_and(lunco_physics::DynamicJointIslandMass::is_valid)
                    {
                        1.0
                    } else {
                        0.0
                    },
                )
            }),
            write: None,
        },
        AvianPort {
            name: "inertia_xx",
            contract: AvianPortContract::INERTIA_BODY,
            dir: PortDirection::InOut,
            read: Some(|w, e| inertia_diagonal(w, e).map(|d| d.x)),
            write: Some(|w, e, v| write_inertia_axis(w, e, 0, v)),
        },
        AvianPort {
            name: "inertia_yy",
            contract: AvianPortContract::INERTIA_BODY,
            dir: PortDirection::InOut,
            read: Some(|w, e| inertia_diagonal(w, e).map(|d| d.y)),
            write: Some(|w, e, v| write_inertia_axis(w, e, 1, v)),
        },
        AvianPort {
            name: "inertia_zz",
            contract: AvianPortContract::INERTIA_BODY,
            dir: PortDirection::InOut,
            read: Some(|w, e| inertia_diagonal(w, e).map(|d| d.z)),
            write: Some(|w, e, v| write_inertia_axis(w, e, 2, v)),
        },
        AvianPort {
            name: "com_x",
            contract: AvianPortContract::LENGTH_BODY,
            dir: PortDirection::InOut,
            read: Some(|w, e| center_of_mass(w, e).map(|c| c.x)),
            write: Some(|w, e, v| write_com_axis(w, e, 0, v)),
        },
        AvianPort {
            name: "com_y",
            contract: AvianPortContract::LENGTH_BODY,
            dir: PortDirection::InOut,
            read: Some(|w, e| center_of_mass(w, e).map(|c| c.y)),
            write: Some(|w, e, v| write_com_axis(w, e, 1, v)),
        },
        AvianPort {
            name: "com_z",
            contract: AvianPortContract::LENGTH_BODY,
            dir: PortDirection::InOut,
            read: Some(|w, e| center_of_mass(w, e).map(|c| c.z)),
            write: Some(|w, e, v| write_com_axis(w, e, 2, v)),
        },
    ],
    install_topology: register_rigid_body_topology,
};

fn register_rigid_body_topology(app: &mut App) {
    app.add_observer(lunco_port_core::ports::bump_port_topology_on_add::<RigidBody>)
        .add_observer(lunco_port_core::ports::bump_port_topology_on_remove::<RigidBody>)
        .add_observer(lunco_port_core::ports::bump_port_topology_on_add::<Position>)
        .add_observer(lunco_port_core::ports::bump_port_topology_on_remove::<Position>)
        .add_observer(lunco_port_core::ports::bump_port_topology_on_add::<LinearVelocity>)
        .add_observer(lunco_port_core::ports::bump_port_topology_on_remove::<LinearVelocity>)
        .add_observer(lunco_port_core::ports::bump_port_topology_on_add::<Rotation>)
        .add_observer(lunco_port_core::ports::bump_port_topology_on_remove::<Rotation>)
        .add_observer(lunco_port_core::ports::bump_port_topology_on_add::<AngularVelocity>)
        .add_observer(lunco_port_core::ports::bump_port_topology_on_remove::<AngularVelocity>)
        .add_observer(lunco_port_core::ports::bump_port_topology_on_add::<SolvedLinearAcceleration>)
        .add_observer(
            lunco_port_core::ports::bump_port_topology_on_remove::<SolvedLinearAcceleration>,
        )
        .add_observer(lunco_port_core::ports::bump_port_topology_on_add::<ComputedMass>)
        .add_observer(lunco_port_core::ports::bump_port_topology_on_remove::<ComputedMass>)
        .add_observer(lunco_port_core::ports::bump_port_topology_on_add::<ComputedAngularInertia>)
        .add_observer(
            lunco_port_core::ports::bump_port_topology_on_remove::<ComputedAngularInertia>,
        )
        .add_observer(lunco_port_core::ports::bump_port_topology_on_add::<ComputedCenterOfMass>)
        .add_observer(lunco_port_core::ports::bump_port_topology_on_remove::<ComputedCenterOfMass>)
        .add_observer(
            lunco_port_core::ports::bump_port_topology_on_add::<lunco_core::PhysicsStateReady>,
        )
        .add_observer(
            lunco_port_core::ports::bump_port_topology_on_remove::<lunco_core::PhysicsStateReady>,
        )
        .add_observer(
            lunco_port_core::ports::bump_port_topology_on_add::<lunco_core::GlobalEntityId>,
        )
        .add_observer(
            lunco_port_core::ports::bump_port_topology_on_remove::<lunco_core::GlobalEntityId>,
        )
        .add_systems(PostUpdate, check_rigid_body_frame_identity);
}

fn rigid_body_topology_key(world: &World, entity: Entity) -> u64 {
    let Some(_) = world.get::<RigidBody>(entity) else {
        return 0;
    };
    let mut key = 1;
    if world.get::<Position>(entity).is_some() {
        key |= 1 << 1;
    }
    if world.get::<LinearVelocity>(entity).is_some() {
        key |= 1 << 2;
    }
    if world.get::<Rotation>(entity).is_some() {
        key |= 1 << 3;
    }
    if world.get::<AngularVelocity>(entity).is_some() {
        key |= 1 << 4;
    }
    if world.get::<ComputedMass>(entity).is_some() {
        key |= 1 << 5;
    }
    if world.get::<ComputedAngularInertia>(entity).is_some() {
        key |= 1 << 6;
    }
    if world.get::<ComputedCenterOfMass>(entity).is_some() {
        key |= 1 << 7;
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hasher);
    world
        .get::<lunco_core::GlobalEntityId>(entity)
        .map(lunco_core::GlobalEntityId::get)
        .hash(&mut hasher);
    hasher.finish()
}

fn check_rigid_body_frame_identity(
    changed_ids: Query<(Entity, &lunco_core::GlobalEntityId), Changed<lunco_core::GlobalEntityId>>,
    rigid_bodies: Query<(), With<RigidBody>>,
    mut state: ResMut<lunco_port_core::ports::PortTopologyState>,
    mut revision: ResMut<lunco_port_core::ports::PortTopologyRevision>,
) {
    for (entity, global_id) in &changed_ids {
        if rigid_bodies.get(entity).is_ok()
            && state.changed::<lunco_core::GlobalEntityId>(entity, global_id.get())
        {
            revision.bump();
        }
    }
}

/// Position inputs for an authored kinematic body.
///
/// The gate uses [`lunco_core::Mobility::Kinematic`], the stable projection of
/// `physics:kinematicEnabled = true`, rather than Avian's transient
/// [`RigidBody::Kinematic`] component. Dynamic USD bodies deliberately wear that
/// Avian variant while their joints are admitted, and must never become
/// position-commandable during that setup phase.
///
/// This gives signal networks a generic way to pose a non-integrated marker or
/// mechanism through ordinary USD connections. It is not a teleport escape hatch
/// for simulated vehicles: a dynamic body's authored mobility does not satisfy
/// this group, so resolving the input fails closed.
pub const KINEMATIC_POSITION_GROUP: AvianGroup = AvianGroup {
    source: "Avian kinematic body",
    present: |w, e| {
        w.get::<lunco_core::Mobility>(e)
            .is_some_and(|mobility| *mobility == lunco_core::Mobility::Kinematic)
    },
    entities: |world, out| {
        out.extend(
            world
                .query::<(Entity, &lunco_core::Mobility)>()
                .iter(world)
                .filter_map(|(entity, mobility)| {
                    (*mobility == lunco_core::Mobility::Kinematic).then_some(entity)
                }),
        );
    },
    topology_key: kinematic_position_topology_key,
    ports: &[
        AvianPort {
            name: "position_x",
            contract: AvianPortContract::LENGTH_WORLD,
            dir: PortDirection::In,
            read: Some(|w, e| w.get::<Position>(e).map(|p| p.0.x)),
            write: Some(|w, e, value| write_kinematic_position_axis(w, e, value, 0)),
        },
        AvianPort {
            name: "position_y",
            contract: AvianPortContract::LENGTH_WORLD,
            dir: PortDirection::In,
            read: Some(|w, e| w.get::<Position>(e).map(|p| p.0.y)),
            write: Some(|w, e, value| write_kinematic_position_axis(w, e, value, 1)),
        },
        AvianPort {
            name: "position_z",
            contract: AvianPortContract::LENGTH_WORLD,
            dir: PortDirection::In,
            read: Some(|w, e| w.get::<Position>(e).map(|p| p.0.z)),
            write: Some(|w, e, value| write_kinematic_position_axis(w, e, value, 2)),
        },
    ],
    install_topology: register_kinematic_position_topology,
};

fn register_kinematic_position_topology(app: &mut App) {
    app.add_observer(lunco_port_core::ports::bump_port_topology_on_add::<lunco_core::Mobility>)
        .add_observer(lunco_port_core::ports::bump_port_topology_on_remove::<lunco_core::Mobility>)
        .add_systems(PostUpdate, check_kinematic_position_structure);
}

fn kinematic_position_topology_key(world: &World, entity: Entity) -> u64 {
    if !world
        .get::<lunco_core::Mobility>(entity)
        .is_some_and(|mobility| *mobility == lunco_core::Mobility::Kinematic)
    {
        return 0;
    }
    1 | (u64::from(world.get::<Position>(entity).is_some()) << 1)
}

/// A Mobility value is structural for this group: only the `Kinematic` value
/// admits the position-input ports. Dynamic and static values are both absent
/// from the group, so transitions between those two values do not invalidate.
fn check_kinematic_position_structure(
    changed: Query<(Entity, &lunco_core::Mobility), Changed<lunco_core::Mobility>>,
    mut state: ResMut<lunco_port_core::ports::PortTopologyState>,
    mut revision: ResMut<lunco_port_core::ports::PortTopologyRevision>,
) {
    for (entity, mobility) in &changed {
        let key = u64::from(*mobility == lunco_core::Mobility::Kinematic);
        if state.changed::<lunco_core::Mobility>(entity, key) {
            revision.bump();
        }
    }
}

fn write_kinematic_position_axis(world: &mut World, entity: Entity, value: f64, axis: usize) {
    let mobility = world
        .get::<lunco_core::Mobility>(entity)
        .expect("prepared kinematic position input retains its mobility");
    assert_eq!(*mobility, lunco_core::Mobility::Kinematic);
    assert!(axis < 3, "kinematic position axis is declared by its owner");
    let mut position = world
        .get_mut::<Position>(entity)
        .expect("prepared kinematic position input retains Position");
    position.0[axis] = value;
}

// ── Mass-property read/write helpers ────────────────────────────────────────
//
// The f64 solver owns `Computed*`; Avian's local authoring overrides are f32.
// Live writes remove the corresponding local override and install `NoAuto*`.
// Avian's MassPropertyHelper retains the native computed value when automatic
// computation is disabled and no local override exists. Thus collider changes
// cannot overwrite the value or narrow it during a later recomputation.
//
// Scalar inertia inputs require an axis-aligned body tensor. The owner rejects
// them before commit when cross terms are present; authored full tensors retain
// their matrix and remain readable. A scalar cannot express a coupled tensor
// update or validate its positive definiteness.

fn read_mass(w: &World, e: Entity) -> Option<f64> {
    w.get::<ComputedMass>(e).map(|m| m.value())
}

fn write_mass(w: &mut World, e: Entity, v: f64) {
    assert!(
        w.get::<RigidBody>(e).is_some(),
        "prepared mass input retains RigidBody"
    );
    let mass = ComputedMass::new(v);
    if w.get::<ComputedMass>(e) != Some(&mass)
        || w.get::<NoAutoMass>(e).is_none()
        || w.get::<Mass>(e).is_some()
    {
        w.entity_mut(e).remove::<Mass>().insert((mass, NoAutoMass));
    }
}

fn inertia_diagonal(w: &World, e: Entity) -> Option<DVec3> {
    w.get::<ComputedAngularInertia>(e).map(|inertia| {
        let inverse = inertia.inverse_tensor();
        if inverse.m01 == 0.0 && inverse.m02 == 0.0 && inverse.m12 == 0.0 {
            // Component-wise reciprocals avoid determinant overflow/underflow
            // for diagonal tensors spanning the native f64 scalar range.
            DVec3::from_array(
                inverse.diagonal().to_array().map(
                    |value| {
                        if value == 0.0 { 0.0 } else { value.recip() }
                    },
                ),
            )
        } else {
            inertia.tensor().diagonal()
        }
    })
}

/// Whether scalar inputs can update the current native body-frame tensor.
///
/// Positive diagonal inputs describe a complete positive-definite tensor only
/// when its cross terms are zero. Coupled tensors need an atomic matrix input;
/// scalar writes must not flatten or reinterpret their local inertial frame.
pub(crate) fn scalar_inertia_writable(w: &World, e: Entity) -> bool {
    w.get::<ComputedAngularInertia>(e).is_some_and(|inertia| {
        let tensor = inertia.inverse_tensor();
        tensor.is_finite() && tensor.m01 == 0.0 && tensor.m02 == 0.0 && tensor.m12 == 0.0
    })
}

fn write_inertia_axis(w: &mut World, e: Entity, axis: usize, v: f64) {
    assert!(
        w.get::<RigidBody>(e).is_some(),
        "prepared inertia input retains RigidBody"
    );
    assert!(axis < 3, "inertia axis is declared by its owner");
    assert!(
        scalar_inertia_writable(w, e),
        "prepared scalar inertia input retains an axis-aligned tensor"
    );
    let mut diagonal = inertia_diagonal(w, e)
        .expect("writable inertia ports require an effective native baseline");
    diagonal[axis] = v;
    let inertia = ComputedAngularInertia::new(diagonal);
    if w.get::<ComputedAngularInertia>(e) != Some(&inertia)
        || w.get::<NoAutoAngularInertia>(e).is_none()
        || w.get::<AngularInertia>(e).is_some()
    {
        w.entity_mut(e)
            .remove::<AngularInertia>()
            .insert((inertia, NoAutoAngularInertia));
    }
}

fn center_of_mass(w: &World, e: Entity) -> Option<DVec3> {
    w.get::<ComputedCenterOfMass>(e).map(|c| c.0)
}

fn write_com_axis(w: &mut World, e: Entity, axis: usize, v: f64) {
    assert!(
        w.get::<RigidBody>(e).is_some(),
        "prepared centre-of-mass input retains RigidBody"
    );
    assert!(axis < 3, "centre-of-mass axis is declared by its owner");
    let mut c = center_of_mass(w, e)
        .expect("writable centre-of-mass ports require an effective native baseline");
    c[axis] = v;
    let center = ComputedCenterOfMass(c);
    if w.get::<ComputedCenterOfMass>(e) != Some(&center)
        || w.get::<NoAutoCenterOfMass>(e).is_none()
        || w.get::<CenterOfMass>(e).is_some()
    {
        w.entity_mut(e)
            .remove::<CenterOfMass>()
            .insert((center, NoAutoCenterOfMass));
    }
}

/// Apply each entity's nonzero accumulated [`PendingForces`] into avian, then
/// clear those values. Already-zero accumulators stay unchanged across ticks.
///
/// The single per-tick avian system: it bridges the `force_*` ports (which land
/// in [`PendingForces`]) to avian's query-shaped `Forces` writer. Avian clears
/// non-constant forces each step, so re-applying the freshly summed value every
/// tick is correct. Runs in
/// [`lunco_cosim_core::schedule::CosimApplySet::ApplyForces`]
/// (after propagation).
pub fn apply_pending_forces(
    physics_time: Res<Time<Physics>>,
    virtual_time: Res<Time<Virtual>>,
    mut q_pending: Query<(Entity, &mut PendingForces)>,
    mut holds: Option<ResMut<lunco_physics::PhysicsHolds>>,
    mut faults: Option<ResMut<lunco_core::RuntimeFaults>>,
    mut scratch: Local<ActuatorCommandScratch>,
    // Force must land only on a body the solver will integrate. A disabled body
    // (frozen while its program compiles, say) never has its accumulators
    // cleared, so force applied to it is stored, not spent, and discharges in
    // full on the step that eventually runs — see `lunco_physics::Integrable`.
    mut forces: Query<Forces, lunco_physics::Integrable>,
    mut actuator_commands: ParamSet<(
        Query<(
            Entity,
            &ForceActuator,
            &mut PendingActuatorCommand,
            Option<&lunco_core::GlobalEntityId>,
            Option<&Name>,
        )>,
        Query<(
            Entity,
            &TorqueActuator,
            &mut PendingActuatorCommand,
            Option<&lunco_core::GlobalEntityId>,
            Option<&Name>,
        )>,
    )>,
    q_parents: Query<&ChildOf>,
    q_poses: Query<(Entity, &Position, &Rotation), lunco_physics::Integrable>,
) {
    let physics_live =
        lunco_physics::physics_is_live_state(&physics_time, &virtual_time, faults.as_deref());
    for (e, mut pf) in &mut q_pending {
        if !pf.f.is_finite() || !pf.f_local.is_finite() || !pf.torque.is_finite() {
            let detail = format!(
                "force={:?}, force_local={:?}, torque={:?}",
                pf.f, pf.f_local, pf.torque
            );
            pf.f = DVec3::ZERO;
            pf.f_local = DVec3::ZERO;
            pf.torque = DVec3::ZERO;
            if let Some(holds) = holds.as_deref_mut() {
                holds.set(lunco_physics::PhysicsHolds::SAFETY_FAILURE, true);
            }
            if let Some(faults) = faults.as_deref_mut() {
                if faults.raise("cosim-nonfinite-force", Some(e), "PendingForces", detail) {
                    error!(
                        "[cosim] terminal runtime failure: non-finite force accumulator on {e:?}"
                    );
                }
            }
            continue;
        }
        let has_pending_force =
            pf.f != DVec3::ZERO || pf.f_local != DVec3::ZERO || pf.torque != DVec3::ZERO;
        if physics_live && has_pending_force {
            if let Ok(mut f) = forces.get_mut(e) {
                if pf.f != DVec3::ZERO {
                    f.apply_force(pf.f);
                }
                if pf.f_local != DVec3::ZERO {
                    // Avian rotates this into world by the body's attitude.
                    f.apply_local_force(pf.f_local);
                }
                if pf.torque != DVec3::ZERO {
                    f.apply_torque(pf.torque);
                }
            }
        }
        if has_pending_force {
            pf.f = DVec3::ZERO;
            pf.f_local = DVec3::ZERO;
            pf.torque = DVec3::ZERO;
        }
    }

    // Copy and clear actuator commands before applying them. The copy is
    // sorted by authored identity rather than ECS/archetype iteration order:
    // USD prims can finish asynchronous projection in different update
    // batches, which must never change the floating-point order in which
    // several thrusters add force/torque to the same rigid body. GID is the
    // network-stable identity; Name is the deterministic local fallback for
    // non-networked authored entities, and entity bits are only a final tie
    // breaker for duplicate names.
    scratch.force.clear();
    for (entity, actuator, mut command, gid, name) in actuator_commands.p0().iter_mut() {
        scratch.force.push((
            gid.map(lunco_core::GlobalEntityId::get),
            stable_name_hash(name),
            entity.to_bits(),
            entity,
            *actuator,
            command.value,
        ));
        command.value = 0.0;
    }
    scratch
        .force
        .sort_unstable_by_key(|entry| (entry.0, entry.1, entry.2));

    scratch.torque.clear();
    for (entity, actuator, mut command, gid, name) in actuator_commands.p1().iter_mut() {
        scratch.torque.push((
            gid.map(lunco_core::GlobalEntityId::get),
            stable_name_hash(name),
            entity.to_bits(),
            entity,
            *actuator,
            command.value,
        ));
        command.value = 0.0;
    }

    scratch
        .torque
        .sort_unstable_by_key(|entry| (entry.0, entry.1, entry.2));

    // A held physics clock does not consume Avian's force accumulator. The
    // commands were cleared above so a command sampled during loading or a
    // readiness hold cannot become a launch impulse when the hold ends.
    if !physics_live {
        return;
    }

    // Generic force-actuator commands are drained only after ordinary body
    // force ports. Each actuator is resolved through the live ECS hierarchy to
    // the nearest rigid body, then Avian receives the actual world-space force
    // and point. Avian owns the resulting r×F torque calculation and the live
    // center of mass.
    for (_, _, _, actuator_entity, actuator, force_n) in scratch.force.drain(..) {
        if !force_n.is_finite()
            || !actuator.local_position.is_finite()
            || !actuator.direction_local.is_finite()
            || !actuator.max_force_n.is_finite()
        {
            if let Some(holds) = holds.as_deref_mut() {
                holds.set(lunco_physics::PhysicsHolds::SAFETY_FAILURE, true);
            }
            if let Some(faults) = faults.as_deref_mut() {
                if faults.raise(
                    "cosim-nonfinite-force-actuator",
                    Some(actuator_entity),
                    "ForceActuator",
                    format!(
                        "command={force_n:?}, position={:?}, direction={:?}, max_force={:?}",
                        actuator.local_position, actuator.direction_local, actuator.max_force_n
                    ),
                ) {
                    error!(
                        "[cosim] terminal runtime failure: non-finite force actuator command on {actuator_entity:?}"
                    );
                }
            }
            continue;
        }
        let Some(body) = nearest_rigid_body(actuator_entity, &q_parents, &q_poses) else {
            // The actuator may arrive one frame before its body. Propagation will
            // issue the command again on the next tick; dropping this value is
            // preferable to applying it to a guessed body.
            continue;
        };
        let Ok((_, position, rotation)) = q_poses.get(body) else {
            continue;
        };
        let Ok(mut body_forces) = forces.get_mut(body) else {
            continue;
        };
        let direction = actuator.direction_local.normalize_or_zero().as_dvec3();
        if direction == DVec3::ZERO || actuator.max_force_n <= 0.0 {
            continue;
        }
        let thrust = force_n.clamp(0.0, actuator.max_force_n);
        if thrust == 0.0 {
            continue;
        }
        let local_position = actuator.local_position.as_dvec3();
        let world_point = position.0 + rotation.0 * local_position;
        let world_force = rotation.0 * (direction * thrust);
        body_forces.apply_force_at_point(world_force, world_point);
    }

    // Torque actuators (reaction wheels, CMGs, and future devices) use the
    // same description-driven command path. Avian owns the torque integration;
    // no actuator-specific Rust or Modelica r×F calculation is involved.
    for (_, _, _, actuator_entity, actuator, torque_nm) in scratch.torque.drain(..) {
        if !torque_nm.is_finite()
            || !actuator.axis_local.is_finite()
            || !actuator.max_torque_nm.is_finite()
        {
            if let Some(holds) = holds.as_deref_mut() {
                holds.set(lunco_physics::PhysicsHolds::SAFETY_FAILURE, true);
            }
            if let Some(faults) = faults.as_deref_mut() {
                if faults.raise(
                    "cosim-nonfinite-torque-actuator",
                    Some(actuator_entity),
                    "TorqueActuator",
                    format!(
                        "command={torque_nm:?}, axis={:?}, max_torque={:?}",
                        actuator.axis_local, actuator.max_torque_nm
                    ),
                ) {
                    error!(
                        "[cosim] terminal runtime failure: non-finite torque actuator command on {actuator_entity:?}"
                    );
                }
            }
            continue;
        }
        let Some(body) = nearest_rigid_body(actuator_entity, &q_parents, &q_poses) else {
            continue;
        };
        let Ok(mut body_forces) = forces.get_mut(body) else {
            continue;
        };
        let axis = actuator.axis_local.normalize_or_zero().as_dvec3();
        if axis == DVec3::ZERO || actuator.max_torque_nm <= 0.0 {
            continue;
        }
        let torque = torque_nm.clamp(-actuator.max_torque_nm, actuator.max_torque_nm);
        if torque != 0.0 {
            let (_, _, rotation) = q_poses.get(body).expect("body found in pose query");
            body_forces.apply_torque(rotation.0 * (axis * torque));
        }
    }
}

/// Find the nearest rigid-body ancestor of a physical mount.
fn nearest_rigid_body(
    start: Entity,
    parents: &Query<&ChildOf>,
    bodies: &Query<(Entity, &Position, &Rotation), lunco_physics::Integrable>,
) -> Option<Entity> {
    let mut current = start;
    for _ in 0..64 {
        if bodies.get(current).is_ok() {
            return Some(current);
        }
        current = parents.get(current).ok()?.parent();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conventionally_named_force_ports_are_all_declared() {
        for group in crate::ports::AVIAN {
            for p in group.ports {
                let looks_like_force =
                    p.name.starts_with("force_") || p.name.starts_with("torque_");
                if looks_like_force {
                    assert!(
                        lunco_physics::force_ports::is_physics_force_port(p.name),
                        "avian port `{}` looks like a body-force port but is not in \
                         lunco_physics::force_ports::BODY_FORCE_PORTS — it would bypass the RealtimeSafe gate",
                        p.name
                    );
                }
            }
        }
    }

    #[test]
    fn joint_island_mass_ports_expose_only_complete_solver_measurements() {
        let mut app = App::new();
        app.init_resource::<lunco_physics::DynamicJointIslandMasses>()
            .add_systems(Update, lunco_physics::refresh_dynamic_joint_island_masses);
        let root = app
            .world_mut()
            .spawn((RigidBody::Dynamic, ComputedMass::new(2_000.0)))
            .id();
        let leg = app
            .world_mut()
            .spawn((RigidBody::Dynamic, ComputedMass::new(180.0)))
            .id();
        let missing_member = app.world_mut().spawn(RigidBody::Dynamic).id();
        let independent = app
            .world_mut()
            .spawn((RigidBody::Dynamic, ComputedMass::new(75.0)))
            .id();
        app.world_mut().spawn(lunco_physics::PhysicsJointLink {
            body0: root,
            body1: leg,
        });
        app.world_mut().spawn(lunco_physics::PhysicsJointLink {
            body0: root,
            body1: missing_member,
        });

        app.update();
        let world = app.world();
        let read = |name: &str, entity| {
            RIGID_BODY_GROUP
                .ports
                .iter()
                .find(|port| port.name == name)
                .and_then(|port| port.read)
                .expect("joint island output is part of the rigid-body port contract")(
                world, entity,
            )
        };

        assert_eq!(read("dynamic_joint_island_mass_kg", root), None);
        assert_eq!(read("dynamic_joint_island_mass_valid", root), Some(0.0));
        assert_eq!(read("dynamic_joint_island_mass_kg", leg), None);
        assert_eq!(
            read("dynamic_joint_island_mass_valid", missing_member),
            Some(0.0)
        );
        assert_eq!(
            read("dynamic_joint_island_mass_kg", independent),
            Some(75.0)
        );
        assert_eq!(
            read("dynamic_joint_island_mass_valid", independent),
            Some(1.0)
        );
    }

    #[test]
    fn native_live_mass_properties_preserve_precision_recomputation_and_change_detection() {
        let mut world = World::new();
        world.init_resource::<lunco_port_core::ports::PortTopologyRevision>();
        let body = world
            .spawn((
                RigidBody::Dynamic,
                Mass(4000.0),
                AngularInertia::new(Vec3::new(4625.0, 6250.0, 4625.0)),
                CenterOfMass(Vec3::ZERO),
                ComputedMass::new(4000.0),
                ComputedAngularInertia::new(DVec3::new(4625.0, 6250.0, 4625.0)),
                ComputedCenterOfMass(DVec3::ZERO),
            ))
            .id();
        let mut registry = lunco_port_core::ports::PortRegistry::default();
        crate::ports::register_builtin_port_backends(&mut registry);
        let values = [
            ("mass", 4000.00000001),
            ("inertia_xx", 4625.00000001),
            ("inertia_yy", 6250.00000001),
            ("inertia_zz", 4625.00000001),
            ("com_x", 0.00000001),
            ("com_y", 0.40000000001),
            ("com_z", -0.00000001),
        ];
        for (name, value) in values {
            registry.write_port(&mut world, body, name, value).unwrap();
        }
        assert!(world.get::<Mass>(body).is_none());
        assert!(world.get::<AngularInertia>(body).is_none());
        assert!(world.get::<CenterOfMass>(body).is_none());
        assert!(world.get::<NoAutoMass>(body).is_some());
        assert!(world.get::<NoAutoAngularInertia>(body).is_some());
        assert!(world.get::<NoAutoCenterOfMass>(body).is_some());

        // Exercise the dependency's real recompute seam: an unrelated collider
        // update must retain the live native values when local overrides are absent.
        let mut state =
            bevy::ecs::system::SystemState::<avian3d::prelude::MassPropertyHelper>::new(&mut world);
        state
            .get_mut(&mut world)
            .expect("mass-property helper query is valid")
            .update_mass_properties(body);
        state.apply(&mut world);
        for (name, expected) in values {
            let actual = registry.read_port(&world, body, name).unwrap();
            assert!(
                (actual - expected).abs() <= expected.abs() * f64::EPSILON * 2.0,
                "{name}: {actual} != {expected}"
            );
            assert_ne!(actual, (expected as f32) as f64, "{name} narrowed to f32");
        }
        world.clear_trackers();
        for (name, value) in values {
            registry.write_port(&mut world, body, name, value).unwrap();
        }

        let body_ref = world.entity(body);
        assert!(!body_ref.get_ref::<ComputedMass>().unwrap().is_changed());
        assert!(
            !body_ref
                .get_ref::<ComputedAngularInertia>()
                .unwrap()
                .is_changed()
        );
        assert!(
            !body_ref
                .get_ref::<ComputedCenterOfMass>()
                .unwrap()
                .is_changed()
        );

        for name in ["mass", "inertia_xx", "com_y"] {
            registry.write_port(&mut world, body, name, 1.0e50).unwrap();
            let actual = registry.read_port(&world, body, name).unwrap();
            assert!((actual / 1.0e50 - 1.0).abs() <= f64::EPSILON * 2.0);
        }
    }

    #[test]
    fn native_live_mass_properties_reject_coupled_scalar_inertia_without_mutation() {
        let mut world = World::new();
        world.init_resource::<lunco_port_core::ports::PortTopologyRevision>();
        let inertia = ComputedAngularInertia::new_with_local_frame(
            DVec3::new(4.0, 6.0, 8.0),
            bevy::math::DQuat::from_rotation_z(0.4),
        );
        let body = world.spawn((RigidBody::Dynamic, inertia)).id();
        let mut registry = lunco_port_core::ports::PortRegistry::default();
        crate::ports::register_builtin_port_backends(&mut registry);
        for name in ["inertia_xx", "inertia_yy", "inertia_zz"] {
            let error = registry
                .write_port(&mut world, body, name, 5.0)
                .unwrap_err();
            assert!(matches!(
                error.kind,
                lunco_port_core::ports::PortWriteErrorKind::NotWritable
            ));
            assert_eq!(world.get::<ComputedAngularInertia>(body), Some(&inertia));
            assert!(registry.read_port(&world, body, name).unwrap() > 0.0);
        }
        assert!(world.get::<NoAutoAngularInertia>(body).is_none());
    }
}
