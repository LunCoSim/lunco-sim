//! Avian rigid bodies exposed as co-simulation ports (the **body** half of the
//! avian backend; [`crate::joint`] is the joint half).
//!
//! Avian's components are foreign types we don't own, so they are exposed
//! through a declarative port spec ([`crate::ports::AvianGroup`]) rather than a
//! mirror component per kind. A rigid body publishes its full kinematic state —
//! position, linear velocity, attitude (`quat_*` + `yaw`/`pitch`/`roll`), and
//! body rates (`angvel_*`) — as read-only outputs, and accepts world/body-frame
//! forces and torques as inputs. All address avian's own `Position` /
//! `Rotation` / `LinearVelocity` / `AngularVelocity` / `Forces` directly, with
//! no `HashMap` mirror and no per-tick sync system to keep a copy in step.
//!
//! Force inputs are an **additive sink**: the wire write lands in
//! [`PendingForces`] (the propagation master has already summed all wires into
//! that one value), and the single generic [`apply_pending_forces`] system
//! applies it through avian's query-shaped `Forces` API and clears it each tick.
//! That one system is the only per-tick avian system left.

use avian3d::prelude::{
    AngularInertia, AngularVelocity, CenterOfMass, ComputedAngularInertia, ComputedCenterOfMass,
    ComputedMass, Forces, LinearVelocity, Mass, NoAutoAngularInertia, NoAutoCenterOfMass,
    NoAutoMass, Position, RigidBody, Rotation, WriteRigidBodyForces,
};
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
        // `height` is the conventional alias for `position_y`.
        AvianPort {
            name: "height",
            dir: PortDirection::Out,
            read: Some(|w, e| w.get::<Position>(e).map(|p| p.0.y)),
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
                w.get::<Rotation>(e).map(|r| r.0.to_euler(bevy::math::EulerRot::YXZ).0)
            }),
            write: None,
        },
        AvianPort {
            name: "pitch",
            dir: PortDirection::Out,
            read: Some(|w, e| {
                w.get::<Rotation>(e).map(|r| r.0.to_euler(bevy::math::EulerRot::YXZ).1)
            }),
            write: None,
        },
        AvianPort {
            name: "roll",
            dir: PortDirection::Out,
            read: Some(|w, e| {
                w.get::<Rotation>(e).map(|r| r.0.to_euler(bevy::math::EulerRot::YXZ).2)
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
    w.entity_mut(e).insert((Mass(v as f32), NoAutoMass));
    true
}

fn inertia_diagonal(w: &World, e: Entity) -> Option<DVec3> {
    w.get::<ComputedAngularInertia>(e).map(|i| i.value().diagonal())
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
    w.entity_mut(e)
        .insert((AngularInertia { principal, local_frame }, NoAutoAngularInertia));
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
    w.entity_mut(e).insert((CenterOfMass(c), NoAutoCenterOfMass));
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
    mut q_pending: Query<(Entity, &mut PendingForces)>,
    mut forces: Query<Forces>,
) {
    for (e, mut pf) in &mut q_pending {
        if pf.f != DVec3::ZERO || pf.f_local != DVec3::ZERO || pf.torque != DVec3::ZERO {
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
}
