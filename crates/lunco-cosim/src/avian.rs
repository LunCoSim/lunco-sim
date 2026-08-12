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
    ComputedAngularInertia, ComputedCenterOfMass, ComputedMass, ContactGraph, Forces,
    LinearVelocity, Mass, NoAutoAngularInertia, NoAutoCenterOfMass, NoAutoMass, Physics, Position,
    RigidBody, Rotation, Sensor, WriteRigidBodyForces,
};
use avian3d::schedule::PhysicsTime;
use bevy::math::DVec3;
use bevy::prelude::*;

use crate::connection::PortDirection;
use crate::ports::{AvianGroup, AvianPort};

/// The avian input ports that sink into [`PendingForces`] — i.e. **writing one
/// pushes a rigid body around**. Declared here, beside the port table that
/// implements them, because a port's meaning belongs to the backend that owns
/// it. [`crate::connection::is_physics_force_port`] is the consumer.
///
/// ENUMERATED, never matched by spelling. A name test cannot tell a body torque
/// (N·m about a world axis, applied to a rigid body) from a shaft torque (N·m
/// through a gearbox, applied to nothing) — and it cannot see a body-force port
/// that is not spelled `force*`/`torque*` at all. Add a port that writes
/// `PendingForces`, add it here.
pub const BODY_FORCE_PORTS: &[&str] = &[
    // World-space linear force → `PendingForces::f`.
    "force_x",
    "force_y",
    "force_z",
    // Body-frame linear force → `PendingForces::f_local` (rotated into world at
    // apply time). These are why an exact `force_{x,y,z}` list would be a hole.
    "force_local_x",
    "force_local_y",
    "force_local_z",
    // World-space torque → `PendingForces::torque`.
    "torque_x",
    "torque_y",
    "torque_z",
];

/// Input ports that drive generic physical actuators. These are separate from
/// [`BODY_FORCE_PORTS`]: an actuator is a child prim, while the accumulator and
/// Avian rigid body live on its owning body.
pub const ACTUATOR_FORCE_PORTS: &[&str] = &["force_command", "torque_command"];

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
    for (entity, velocity) in &query {
        commands
            .entity(entity)
            .try_insert(SolvedLinearAcceleration {
                previous_velocity: velocity.0,
                value: DVec3::ZERO,
                valid: false,
            });
    }
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

/// A generic force actuator authored by a USD prim.
///
/// The local position is measured from the owning rigid-body origin, not from
/// its render transform. The direction is the force ON the body, in the body's
/// local frame; exhaust visuals may point in the opposite direction. The
/// component is metadata only. The live command is kept separately in
/// [`PendingActuatorCommand`] so port writes do not mutate the authored
/// description. An RCS nozzle and a translation thruster use this same type.
#[derive(Component, Debug, Clone, Copy, Reflect)]
#[reflect(Component)]
pub struct ForceActuator {
    /// Mount position relative to the owning rigid-body origin (m).
    pub local_position: Vec3,
    /// Unit force direction in the owning body's local frame.
    pub direction_local: Vec3,
    /// Maximum accepted thrust for this nozzle (N).
    pub max_force_n: f64,
}

/// A generic torque actuator. A reaction wheel, control-moment gyro, or any
/// other torque source uses this same type.
#[derive(Component, Debug, Clone, Copy, Reflect)]
#[reflect(Component)]
pub struct TorqueActuator {
    /// Torque axis in the owning body's local frame.
    pub axis_local: Vec3,
    /// Maximum torque magnitude (N·m).
    pub max_torque_nm: f64,
}

/// One tick's command for a [`ForceActuator`] or [`TorqueActuator`].
#[derive(Component, Debug, Clone, Copy, Default, Reflect)]
#[reflect(Component, Default)]
pub struct PendingActuatorCommand {
    /// Commanded magnitude in the actuator's physical unit. The actuator
    /// clamps it to its authored limit before Avian integration.
    pub value: f64,
}

/// Ensure `entity` carries [`PendingForces`], then mutate it. The `force_*`
/// write closures use this so an un-driven body stays clean until first written.
fn with_pending(world: &mut World, entity: Entity, set: impl FnOnce(&mut PendingForces)) -> bool {
    // The port binding may name a body that a concurrent scene reload just
    // despawned (LoadScene tears the old scene down while propagation is still
    // running). `entity_mut` would panic on that stale id, so fetch fallibly and
    // bail cleanly — next tick propagates against the fresh scene.
    let Ok(mut em) = world.get_entity_mut(entity) else {
        return false;
    };
    if !em.contains::<PendingForces>() {
        em.insert(PendingForces::default());
    }
    if let Some(mut pf) = em.get_mut::<PendingForces>() {
        set(&mut pf);
        true
    } else {
        false
    }
}

/// Ensure an actuator command exists, then update it from the port write.
fn with_pending_actuator_command(world: &mut World, entity: Entity, value: f64) -> bool {
    let Ok(mut em) = world.get_entity_mut(entity) else {
        return false;
    };
    if !em.contains::<PendingActuatorCommand>() {
        em.insert(PendingActuatorCommand::default());
    }
    if let Some(mut command) = em.get_mut::<PendingActuatorCommand>() {
        command.value = value;
        true
    } else {
        false
    }
}

/// Whether a collider is touching anything, and the total contact normal force
/// (N) on it — computed from avian's own contact graph.
///
/// THE one contact computation in the engine. The [`COLLIDER_CONTACT_GROUP`]
/// ports call it through [`contact_from_world`]. Modelica touchdown conversion
/// reads the same native contact fact; it does not re-derive it, or the two
/// answers could drift.
///
/// The force is `Σ normal impulse / (solver passes × physics_dt)`. Avian's
/// documented `normal_impulse` is accumulated across the substeps, but Avian
/// 0.7 records the accumulated normal impulse once in each of its two contact
/// passes (bias and relaxation). This boundary converts that solver accounting
/// into the physical impulse delivered during the complete physics interval.
/// The divisor is the full physics interval, not a solver substep interval.
///
/// `contact_pairs_with` yields every pair whose AABBs overlap, INCLUDING pairs
/// that are not yet touching, so `is_touching` is not optional: without it a leg
/// reads "in contact" while its pad is still approaching, which is the exact
/// false-early-contact this replaced. Sensor colliders are also excluded here:
/// a landing marker's overlap volume is a mission event, not a load-bearing
/// surface, and must not feed a physical touchdown signal.
pub fn contact_of(graph: &ContactGraph, physics_dt: f64, entity: Entity) -> (bool, f64) {
    contact_of_filtered(graph, physics_dt, entity, |_| false)
}

/// Read contact while excluding explicitly authored overlap-only colliders.
/// The predicate is supplied by the caller because `ContactGraph` deliberately
/// stores pair topology, not ECS component semantics.
fn contact_of_filtered(
    graph: &ContactGraph,
    physics_dt: f64,
    entity: Entity,
    is_sensor: impl Fn(Entity) -> bool,
) -> (bool, f64) {
    // Avian 0.7 runs the normal contact constraint twice per substep: once
    // with penetration bias and once without it. `normal_impulse` is the
    // solver's documented accumulated field, but its implementation adds the
    // accumulated value in both passes. Keep this conversion here, at the
    // native contact boundary, so every consumer receives a physical load.
    const CONTACT_SOLVER_PASSES: f64 = 2.0;
    let mut normal_impulse = 0.0;
    let mut touching = false;
    for pair in graph.contact_pairs_with(entity) {
        let other = if pair.collider1 == entity {
            pair.collider2
        } else {
            pair.collider1
        };
        if is_sensor(entity) || is_sensor(other) {
            continue;
        }
        if !pair.is_touching() {
            continue;
        }
        touching = true;
        for manifold in &pair.manifolds {
            for point in &manifold.points {
                normal_impulse += point.normal_impulse;
            }
        }
    }
    (
        touching,
        normal_impulse / (CONTACT_SOLVER_PASSES * physics_dt.max(1e-9)),
    )
}

/// [`contact_of`] for a caller holding only a `&World` — the port-read closures.
///
/// Physics timing comes from the same resource the solver uses, so a port read
/// and the solver agree about the duration of the solved step. Missing resources mean physics
/// has not started; "not touching" is the truthful answer then, not a panic.
pub fn contact_from_world(world: &World, entity: Entity) -> (bool, f64) {
    let Some(graph) = world.get_resource::<ContactGraph>() else {
        return (false, 0.0);
    };
    let dt = world
        .get_resource::<Time<Physics>>()
        .map(|t| t.delta_secs_f64())
        .unwrap_or(0.0)
        .max(1e-9);
    contact_of_filtered(graph, dt, entity, |candidate| {
        world.get::<Sensor>(candidate).is_some()
    })
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
/// from [`contact_of`].
///
/// Read on demand from the contact graph — no mirror component, no per-tick sync
/// system, matching every other port in this module.
pub const COLLIDER_CONTACT_GROUP: AvianGroup = AvianGroup {
    present: |w, e| w.get::<Collider>(e).is_some(),
    ports: &[
        AvianPort {
            name: "contact",
            dir: PortDirection::Out,
            read: Some(|w, e| Some(if contact_from_world(w, e).0 { 1.0 } else { 0.0 })),
            write: None,
        },
        AvianPort {
            name: "contact_force",
            dir: PortDirection::Out,
            read: Some(|w, e| Some(contact_from_world(w, e).1)),
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
            dir: PortDirection::Out,
            read: Some(|w, e| w.get::<ColliderMassProperties>(e).map(|m| m.mass as f64)),
            write: None,
        },
    ],
};

/// A USD-authored force actuator. Its command is scalar force; position and
/// direction are structural facts read from the USD prim.
pub const FORCE_ACTUATOR_GROUP: AvianGroup = AvianGroup {
    present: |w, e| w.get::<ForceActuator>(e).is_some(),
    ports: &[AvianPort {
        name: "force_command",
        dir: PortDirection::In,
        read: Some(|w, e| Some(w.get::<PendingActuatorCommand>(e).map_or(0.0, |p| p.value))),
        write: Some(with_pending_actuator_command),
    }],
};

/// A USD-authored torque actuator. Its command is scalar torque; its axis and
/// limit are structural facts read from the USD prim.
pub const TORQUE_ACTUATOR_GROUP: AvianGroup = AvianGroup {
    present: |w, e| w.get::<TorqueActuator>(e).is_some(),
    ports: &[AvianPort {
        name: "torque_command",
        dir: PortDirection::In,
        read: Some(|w, e| Some(w.get::<PendingActuatorCommand>(e).map_or(0.0, |p| p.value))),
        write: Some(with_pending_actuator_command),
    }],
};

/// The rigid-body port group: position/velocity outputs + force inputs.
///
/// Gated on [`RigidBody`] presence. Position ports resolve from [`Position`]
/// (present on every body); velocity ports from [`LinearVelocity`] (dynamic
/// bodies only — absent on a kinematic body, so those ports simply don't list).
pub const RIGID_BODY_GROUP: AvianGroup = AvianGroup {
    present: |w, e| w.get::<RigidBody>(e).is_some(),
    ports: &[
        AvianPort {
            name: "position_x",
            dir: PortDirection::Out,
            read: Some(|w, e| w.get::<Position>(e).map(|p| p.0.x)),
            write: None,
        },
        AvianPort {
            name: "position_y",
            dir: PortDirection::Out,
            read: Some(|w, e| w.get::<Position>(e).map(|p| p.0.y)),
            write: None,
        },
        AvianPort {
            name: "position_z",
            dir: PortDirection::Out,
            read: Some(|w, e| w.get::<Position>(e).map(|p| p.0.z)),
            write: None,
        },
        AvianPort {
            name: "velocity_x",
            dir: PortDirection::Out,
            read: Some(|w, e| w.get::<LinearVelocity>(e).map(|v| v.0.x)),
            write: None,
        },
        AvianPort {
            name: "velocity_y",
            dir: PortDirection::Out,
            read: Some(|w, e| w.get::<LinearVelocity>(e).map(|v| v.0.y)),
            write: None,
        },
        AvianPort {
            name: "velocity_z",
            dir: PortDirection::Out,
            read: Some(|w, e| w.get::<LinearVelocity>(e).map(|v| v.0.z)),
            write: None,
        },
        // Solved navigation-frame acceleration for a rigid-body-mounted
        // accelerometer. The sample is captured after physics writeback, so it
        // includes every native force and constraint that changed the body.
        AvianPort {
            name: "acceleration_x",
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
            dir: PortDirection::Out,
            read: Some(|w, e| w.get::<LinearVelocity>(e).map(|v| v.0.length())),
            write: None,
        },
        // Attitude as a quaternion (canonical, gimbal-safe). Avian's `Rotation`
        // wraps a `DQuat` in the f64 build. Read-only — write attitude via torque.
        AvianPort {
            name: "quat_w",
            dir: PortDirection::Out,
            read: Some(|w, e| w.get::<Rotation>(e).map(|r| r.0.w)),
            write: None,
        },
        AvianPort {
            name: "quat_x",
            dir: PortDirection::Out,
            read: Some(|w, e| w.get::<Rotation>(e).map(|r| r.0.x)),
            write: None,
        },
        AvianPort {
            name: "quat_y",
            dir: PortDirection::Out,
            read: Some(|w, e| w.get::<Rotation>(e).map(|r| r.0.y)),
            write: None,
        },
        AvianPort {
            name: "quat_z",
            dir: PortDirection::Out,
            read: Some(|w, e| w.get::<Rotation>(e).map(|r| r.0.z)),
            write: None,
        },
        // Euler convenience (radians). Order `YXZ` → (yaw, pitch, roll) for a
        // Y-up world: yaw about world Y, then pitch about X, then roll about Z.
        // Derived from `Rotation`; control laws that want body rates read `angvel_*`.
        AvianPort {
            name: "yaw",
            dir: PortDirection::Out,
            read: Some(|w, e| {
                w.get::<Rotation>(e)
                    .map(|r| r.0.to_euler(bevy::math::EulerRot::YXZ).0)
            }),
            write: None,
        },
        AvianPort {
            name: "pitch",
            dir: PortDirection::Out,
            read: Some(|w, e| {
                w.get::<Rotation>(e)
                    .map(|r| r.0.to_euler(bevy::math::EulerRot::YXZ).1)
            }),
            write: None,
        },
        AvianPort {
            name: "roll",
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
            dir: PortDirection::Out,
            read: Some(|w, e| w.get::<AngularVelocity>(e).map(|v| v.0.x)),
            write: None,
        },
        AvianPort {
            name: "angvel_y",
            dir: PortDirection::Out,
            read: Some(|w, e| w.get::<AngularVelocity>(e).map(|v| v.0.y)),
            write: None,
        },
        AvianPort {
            name: "angvel_z",
            dir: PortDirection::Out,
            read: Some(|w, e| w.get::<AngularVelocity>(e).map(|v| v.0.z)),
            write: None,
        },
        // Force inputs: additive sink into `PendingForces`. Reading returns the
        // value pending this tick (0 once applied/cleared).
        AvianPort {
            name: "force_x",
            dir: PortDirection::In,
            read: Some(|w, e| Some(w.get::<PendingForces>(e).map_or(0.0, |p| p.f.x))),
            write: Some(|w, e, v| with_pending(w, e, |pf| pf.f.x = v)),
        },
        AvianPort {
            name: "force_y",
            dir: PortDirection::In,
            read: Some(|w, e| Some(w.get::<PendingForces>(e).map_or(0.0, |p| p.f.y))),
            write: Some(|w, e, v| with_pending(w, e, |pf| pf.f.y = v)),
        },
        AvianPort {
            name: "force_z",
            dir: PortDirection::In,
            read: Some(|w, e| Some(w.get::<PendingForces>(e).map_or(0.0, |p| p.f.z))),
            write: Some(|w, e, v| with_pending(w, e, |pf| pf.f.z = v)),
        },
        // Body-frame force inputs: rotated into world by the body's attitude at
        // apply time (`apply_local_force`). Thrust along the vehicle's own axes.
        AvianPort {
            name: "force_local_x",
            dir: PortDirection::In,
            read: Some(|w, e| Some(w.get::<PendingForces>(e).map_or(0.0, |p| p.f_local.x))),
            write: Some(|w, e, v| with_pending(w, e, |pf| pf.f_local.x = v)),
        },
        AvianPort {
            name: "force_local_y",
            dir: PortDirection::In,
            read: Some(|w, e| Some(w.get::<PendingForces>(e).map_or(0.0, |p| p.f_local.y))),
            write: Some(|w, e, v| with_pending(w, e, |pf| pf.f_local.y = v)),
        },
        AvianPort {
            name: "force_local_z",
            dir: PortDirection::In,
            read: Some(|w, e| Some(w.get::<PendingForces>(e).map_or(0.0, |p| p.f_local.z))),
            write: Some(|w, e, v| with_pending(w, e, |pf| pf.f_local.z = v)),
        },
        // World-space torque inputs (N·m): reaction wheels, thrust-vector moment.
        AvianPort {
            name: "torque_x",
            dir: PortDirection::In,
            read: Some(|w, e| Some(w.get::<PendingForces>(e).map_or(0.0, |p| p.torque.x))),
            write: Some(|w, e, v| with_pending(w, e, |pf| pf.torque.x = v)),
        },
        AvianPort {
            name: "torque_y",
            dir: PortDirection::In,
            read: Some(|w, e| Some(w.get::<PendingForces>(e).map_or(0.0, |p| p.torque.y))),
            write: Some(|w, e, v| with_pending(w, e, |pf| pf.torque.y = v)),
        },
        AvianPort {
            name: "torque_z",
            dir: PortDirection::In,
            read: Some(|w, e| Some(w.get::<PendingForces>(e).map_or(0.0, |p| p.torque.z))),
            write: Some(|w, e, v| with_pending(w, e, |pf| pf.torque.z = v)),
        },
        // Mass properties (read+write). The triple moves together — propellant
        // burn lightens mass, shifts COM, and shrinks inertia — so a Modelica
        // tank model (or a script, or a wire) can keep all three consistent
        // through the one port surface. See [`write_mass`] for the avian write
        // contract (`NoAuto*` markers + `Computed*`).
        AvianPort {
            name: "mass",
            dir: PortDirection::InOut,
            read: Some(read_mass),
            write: Some(write_mass),
        },
        AvianPort {
            name: "inertia_xx",
            dir: PortDirection::InOut,
            read: Some(|w, e| inertia_diagonal(w, e).map(|d| d.x)),
            write: Some(|w, e, v| write_inertia_axis(w, e, 0, v)),
        },
        AvianPort {
            name: "inertia_yy",
            dir: PortDirection::InOut,
            read: Some(|w, e| inertia_diagonal(w, e).map(|d| d.y)),
            write: Some(|w, e, v| write_inertia_axis(w, e, 1, v)),
        },
        AvianPort {
            name: "inertia_zz",
            dir: PortDirection::InOut,
            read: Some(|w, e| inertia_diagonal(w, e).map(|d| d.z)),
            write: Some(|w, e, v| write_inertia_axis(w, e, 2, v)),
        },
        AvianPort {
            name: "com_x",
            dir: PortDirection::InOut,
            read: Some(|w, e| center_of_mass(w, e).map(|c| c.x)),
            write: Some(|w, e, v| write_com_axis(w, e, 0, v)),
        },
        AvianPort {
            name: "com_y",
            dir: PortDirection::InOut,
            read: Some(|w, e| center_of_mass(w, e).map(|c| c.y)),
            write: Some(|w, e, v| write_com_axis(w, e, 1, v)),
        },
        AvianPort {
            name: "com_z",
            dir: PortDirection::InOut,
            read: Some(|w, e| center_of_mass(w, e).map(|c| c.z)),
            write: Some(|w, e, v| write_com_axis(w, e, 2, v)),
        },
    ],
};

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
    present: |w, e| {
        w.get::<lunco_core::Mobility>(e)
            .is_some_and(|mobility| *mobility == lunco_core::Mobility::Kinematic)
    },
    ports: &[
        AvianPort {
            name: "position_x",
            dir: PortDirection::In,
            read: Some(|w, e| w.get::<Position>(e).map(|p| p.0.x)),
            write: Some(|w, e, value| write_kinematic_position_axis(w, e, value, 0)),
        },
        AvianPort {
            name: "position_y",
            dir: PortDirection::In,
            read: Some(|w, e| w.get::<Position>(e).map(|p| p.0.y)),
            write: Some(|w, e, value| write_kinematic_position_axis(w, e, value, 1)),
        },
        AvianPort {
            name: "position_z",
            dir: PortDirection::In,
            read: Some(|w, e| w.get::<Position>(e).map(|p| p.0.z)),
            write: Some(|w, e, value| write_kinematic_position_axis(w, e, value, 2)),
        },
    ],
};

fn write_kinematic_position_axis(
    world: &mut World,
    entity: Entity,
    value: f64,
    axis: usize,
) -> bool {
    if !value.is_finite()
        || world.get::<lunco_core::Mobility>(entity) != Some(&lunco_core::Mobility::Kinematic)
    {
        return world.get::<lunco_core::Mobility>(entity) == Some(&lunco_core::Mobility::Kinematic);
    }
    let Some(mut position) = world.get_mut::<Position>(entity) else {
        return false;
    };
    position.0[axis] = value;
    true
}

// ── Mass-property read/write helpers ────────────────────────────────────────
//
// Avian splits user *overrides* (`Mass`/`AngularInertia`/`CenterOfMass`) from the
// `Computed*` components the integrator actually reads. **Reads** return the
// effective `Computed*` value (what the solver uses). **Writes** set the
// *override* component AND its `NoAuto*` marker — writing `Computed*` directly
// would be clobbered by the next recompute.
//
// The marker is NOT optional, which is what this comment used to get wrong: it
// claimed "an override takes precedence over collider-derived mass, so no
// `NoAuto*` marker is needed". Avian says otherwise — `MassPropertyHelper`
// (avian3d `dynamics/rigid_body/mass_properties/system_param.rs:95-120`) only
// consults the override *inside* `if no_auto_inertia { .. }`, and on the `else`
// branch ASSIGNS the collider-derived tensor over the top. Without the marker an
// override survives exactly until the next `update_mass_properties`, which any
// collider or `RigidBody` add re-triggers.
//
// That is precisely the reported symptom: `set inertia_xx 4625` returned `true`
// (the insert does succeed) yet read back UNCHANGED, because the read returns
// `ComputedAngularInertia` and avian had already recomputed it from the collider
// at `ColliderDensity` 1.0. The descent lander measured Ixx=159.3, Iyy=274.3,
// Izz=229.4 against the ~4625/6250/4625 its hull and 2000 kg imply — and
// Ixx != Izz on an axisymmetric hull is the giveaway that those numbers are
// collider geometry rather than anything authored.
//
// Overrides are `f32`; we model the principal (diagonal) inertia only —
// off-diagonal cross-terms are left to static USD authoring. A body with no
// `Computed*` yet simply doesn't list the port.

fn read_mass(w: &World, e: Entity) -> Option<f64> {
    w.get::<ComputedMass>(e).map(|m| m.value())
}

fn write_mass(w: &mut World, e: Entity, v: f64) -> bool {
    if w.get::<RigidBody>(e).is_none() {
        return false;
    }
    let mass = Mass(v as f32);
    if w.get::<Mass>(e) == Some(&mass) && w.get::<NoAutoMass>(e).is_some() {
        return true;
    }
    w.entity_mut(e).insert((mass, NoAutoMass));
    true
}

fn inertia_diagonal(w: &World, e: Entity) -> Option<DVec3> {
    w.get::<ComputedAngularInertia>(e)
        .map(|i| i.value().diagonal())
}

fn write_inertia_axis(w: &mut World, e: Entity, axis: usize, v: f64) -> bool {
    if w.get::<RigidBody>(e).is_none() {
        return false;
    }
    // Start from the current override if present, else the effective computed
    // diagonal — so writing one axis preserves the others (and the local frame).
    let (mut principal, local_frame) = match w.get::<AngularInertia>(e) {
        Some(ai) => (ai.principal, ai.local_frame),
        None => (
            inertia_diagonal(w, e).unwrap_or(DVec3::ZERO).as_vec3(),
            Quat::IDENTITY,
        ),
    };
    match axis {
        0 => principal.x = v as f32,
        1 => principal.y = v as f32,
        _ => principal.z = v as f32,
    }
    let inertia = AngularInertia {
        principal,
        local_frame,
    };
    if w.get::<AngularInertia>(e) == Some(&inertia) && w.get::<NoAutoAngularInertia>(e).is_some() {
        return true;
    }
    w.entity_mut(e).insert((inertia, NoAutoAngularInertia));
    true
}

fn center_of_mass(w: &World, e: Entity) -> Option<DVec3> {
    w.get::<ComputedCenterOfMass>(e).map(|c| c.0)
}

fn write_com_axis(w: &mut World, e: Entity, axis: usize, v: f64) -> bool {
    if w.get::<RigidBody>(e).is_none() {
        return false;
    }
    let mut c = match w.get::<CenterOfMass>(e) {
        Some(com) => com.0,
        None => center_of_mass(w, e).unwrap_or(DVec3::ZERO).as_vec3(),
    };
    match axis {
        0 => c.x = v as f32,
        1 => c.y = v as f32,
        _ => c.z = v as f32,
    }
    let center = CenterOfMass(c);
    if w.get::<CenterOfMass>(e) == Some(&center) && w.get::<NoAutoCenterOfMass>(e).is_some() {
        return true;
    }
    w.entity_mut(e).insert((center, NoAutoCenterOfMass));
    true
}

/// Apply each entity's accumulated [`PendingForces`] into avian, then clear it.
///
/// The single per-tick avian system: it bridges the `force_*` ports (which land
/// in [`PendingForces`]) to avian's query-shaped `Forces` writer. Avian clears
/// non-constant forces each step, so re-applying the freshly summed value every
/// tick is correct. Runs in [`crate::systems::apply_forces::CosimSet::ApplyForces`]
/// (after propagation).
pub fn apply_pending_forces(
    physics_time: Res<Time<Physics>>,
    virtual_time: Res<Time<Virtual>>,
    mut q_pending: Query<(Entity, &mut PendingForces)>,
    mut holds: Option<ResMut<lunco_physics::PhysicsHolds>>,
    mut faults: Option<ResMut<lunco_core::RuntimeFaults>>,
    // Force must land only on a body the solver will integrate. A disabled body
    // (frozen while its program compiles, say) never has its accumulators
    // cleared, so force applied to it is stored, not spent, and discharges in
    // full on the step that eventually runs — see `lunco_physics::Integrable`.
    mut forces: Query<Forces, lunco_physics::Integrable>,
    mut actuator_commands: ParamSet<(
        Query<(Entity, &ForceActuator, &mut PendingActuatorCommand)>,
        Query<(Entity, &TorqueActuator, &mut PendingActuatorCommand)>,
    )>,
    q_parents: Query<&ChildOf>,
    q_poses: Query<(Entity, &Position, &Rotation), lunco_physics::Integrable>,
) {
    let physics_live = !physics_time.is_paused()
        && !virtual_time.is_paused()
        && virtual_time.relative_speed_f64() > 0.0
        && !faults.as_deref().is_some_and(|state| state.active());
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
        if physics_live
            && (pf.f != DVec3::ZERO || pf.f_local != DVec3::ZERO || pf.torque != DVec3::ZERO)
        {
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
        pf.f = DVec3::ZERO;
        pf.f_local = DVec3::ZERO;
        pf.torque = DVec3::ZERO;
    }

    // A held physics clock does not consume Avian's force accumulator. Drain
    // actuator commands without applying them so a command sampled during a
    // loading/readiness hold cannot become a launch impulse when the hold ends.
    if !physics_live {
        for (_, _, mut command) in actuator_commands.p0().iter_mut() {
            command.value = 0.0;
        }
        for (_, _, mut command) in actuator_commands.p1().iter_mut() {
            command.value = 0.0;
        }
        return;
    }

    // Generic force-actuator commands are drained only after ordinary body
    // force ports. Each actuator is resolved through the live ECS hierarchy to
    // the nearest rigid body, then Avian receives the actual world-space force
    // and point. Avian owns the resulting r×F torque calculation and the live
    // center of mass.
    for (actuator_entity, actuator, mut command) in actuator_commands.p0().iter_mut() {
        let force_n = command.value;
        command.value = 0.0;
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
    for (actuator_entity, actuator, mut command) in actuator_commands.p1().iter_mut() {
        let torque_nm = command.value;
        command.value = 0.0;
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
    fn unchanged_mass_property_writes_do_not_dirty_avian_state() {
        let mut world = World::new();
        let body = world.spawn(RigidBody::Dynamic).id();

        assert!(write_mass(&mut world, body, 4000.0));
        assert!(write_inertia_axis(&mut world, body, 0, 4625.0));
        assert!(write_inertia_axis(&mut world, body, 1, 6250.0));
        assert!(write_inertia_axis(&mut world, body, 2, 4625.0));
        assert!(write_com_axis(&mut world, body, 0, 0.0));
        assert!(write_com_axis(&mut world, body, 1, 0.4));
        assert!(write_com_axis(&mut world, body, 2, 0.0));
        world.clear_trackers();

        assert!(write_mass(&mut world, body, 4000.0));
        assert!(write_inertia_axis(&mut world, body, 0, 4625.0));
        assert!(write_inertia_axis(&mut world, body, 1, 6250.0));
        assert!(write_inertia_axis(&mut world, body, 2, 4625.0));
        assert!(write_com_axis(&mut world, body, 0, 0.0));
        assert!(write_com_axis(&mut world, body, 1, 0.4));
        assert!(write_com_axis(&mut world, body, 2, 0.0));

        let body_ref = world.entity(body);
        assert!(!body_ref.get_ref::<Mass>().unwrap().is_changed());
        assert!(!body_ref.get_ref::<AngularInertia>().unwrap().is_changed());
        assert!(!body_ref.get_ref::<CenterOfMass>().unwrap().is_changed());
    }
}
