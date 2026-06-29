//! Avian rigid bodies exposed as co-simulation ports (the **body** half of the
//! avian backend; [`crate::joint`] is the joint half).
//!
//! Avian's components are foreign types we don't own, so they are exposed
//! through a declarative port spec ([`crate::ports::AvianGroup`]) rather than a
//! mirror component per kind. A rigid body publishes its position/velocity as
//! read-only outputs and accepts forces as inputs — all addressing avian's own
//! `Position`/`LinearVelocity`/`Forces` directly, with no `HashMap` mirror and
//! no per-tick sync system to keep a copy in step.
//!
//! Force inputs are an **additive sink**: the wire write lands in
//! [`PendingForces`] (the propagation master has already summed all wires into
//! that one value), and the single generic [`apply_pending_forces`] system
//! applies it through avian's query-shaped `Forces` API and clears it each tick.
//! That one system is the only per-tick avian system left.

use avian3d::prelude::{Forces, LinearVelocity, Position, RigidBody, WriteRigidBodyForces};
use bevy::math::DVec3;
use bevy::prelude::*;

use crate::connection::{PortDirection, PortType};
use crate::ports::{AvianGroup, AvianPort};

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
    if world.get::<PendingForces>(entity).is_none() {
        world.entity_mut(entity).insert(PendingForces::default());
    }
    if let Some(mut pf) = world.get_mut::<PendingForces>(entity) {
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
            port_type: PortType::Kinematic,
            read: Some(|w, e| w.get::<Position>(e).map(|p| p.0.x)),
            write: None,
        },
        AvianPort {
            name: "position_y",
            dir: PortDirection::Out,
            port_type: PortType::Kinematic,
            read: Some(|w, e| w.get::<Position>(e).map(|p| p.0.y)),
            write: None,
        },
        AvianPort {
            name: "position_z",
            dir: PortDirection::Out,
            port_type: PortType::Kinematic,
            read: Some(|w, e| w.get::<Position>(e).map(|p| p.0.z)),
            write: None,
        },
        // `height` is the conventional alias for `position_y`.
        AvianPort {
            name: "height",
            dir: PortDirection::Out,
            port_type: PortType::Kinematic,
            read: Some(|w, e| w.get::<Position>(e).map(|p| p.0.y)),
            write: None,
        },
        AvianPort {
            name: "velocity_x",
            dir: PortDirection::Out,
            port_type: PortType::Kinematic,
            read: Some(|w, e| w.get::<LinearVelocity>(e).map(|v| v.0.x)),
            write: None,
        },
        AvianPort {
            name: "velocity_y",
            dir: PortDirection::Out,
            port_type: PortType::Kinematic,
            read: Some(|w, e| w.get::<LinearVelocity>(e).map(|v| v.0.y)),
            write: None,
        },
        AvianPort {
            name: "velocity_z",
            dir: PortDirection::Out,
            port_type: PortType::Kinematic,
            read: Some(|w, e| w.get::<LinearVelocity>(e).map(|v| v.0.z)),
            write: None,
        },
        // Force inputs: additive sink into `PendingForces`. Reading returns the
        // value pending this tick (0 once applied/cleared).
        AvianPort {
            name: "force_x",
            dir: PortDirection::In,
            port_type: PortType::Force,
            read: Some(|w, e| Some(w.get::<PendingForces>(e).map_or(0.0, |p| p.f.x))),
            write: Some(|w, e, v| with_pending(w, e, |pf| pf.f.x = v)),
        },
        AvianPort {
            name: "force_y",
            dir: PortDirection::In,
            port_type: PortType::Force,
            read: Some(|w, e| Some(w.get::<PendingForces>(e).map_or(0.0, |p| p.f.y))),
            write: Some(|w, e, v| with_pending(w, e, |pf| pf.f.y = v)),
        },
        AvianPort {
            name: "force_z",
            dir: PortDirection::In,
            port_type: PortType::Force,
            read: Some(|w, e| Some(w.get::<PendingForces>(e).map_or(0.0, |p| p.f.z))),
            write: Some(|w, e, v| with_pending(w, e, |pf| pf.f.z = v)),
        },
        // Body-frame force inputs: rotated into world by the body's attitude at
        // apply time (`apply_local_force`). Thrust along the vehicle's own axes.
        AvianPort {
            name: "force_local_x",
            dir: PortDirection::In,
            port_type: PortType::Force,
            read: Some(|w, e| Some(w.get::<PendingForces>(e).map_or(0.0, |p| p.f_local.x))),
            write: Some(|w, e, v| with_pending(w, e, |pf| pf.f_local.x = v)),
        },
        AvianPort {
            name: "force_local_y",
            dir: PortDirection::In,
            port_type: PortType::Force,
            read: Some(|w, e| Some(w.get::<PendingForces>(e).map_or(0.0, |p| p.f_local.y))),
            write: Some(|w, e, v| with_pending(w, e, |pf| pf.f_local.y = v)),
        },
        AvianPort {
            name: "force_local_z",
            dir: PortDirection::In,
            port_type: PortType::Force,
            read: Some(|w, e| Some(w.get::<PendingForces>(e).map_or(0.0, |p| p.f_local.z))),
            write: Some(|w, e, v| with_pending(w, e, |pf| pf.f_local.z = v)),
        },
        // World-space torque inputs (N·m): reaction wheels, thrust-vector moment.
        AvianPort {
            name: "torque_x",
            dir: PortDirection::In,
            port_type: PortType::Force,
            read: Some(|w, e| Some(w.get::<PendingForces>(e).map_or(0.0, |p| p.torque.x))),
            write: Some(|w, e, v| with_pending(w, e, |pf| pf.torque.x = v)),
        },
        AvianPort {
            name: "torque_y",
            dir: PortDirection::In,
            port_type: PortType::Force,
            read: Some(|w, e| Some(w.get::<PendingForces>(e).map_or(0.0, |p| p.torque.y))),
            write: Some(|w, e, v| with_pending(w, e, |pf| pf.torque.y = v)),
        },
        AvianPort {
            name: "torque_z",
            dir: PortDirection::In,
            port_type: PortType::Force,
            read: Some(|w, e| Some(w.get::<PendingForces>(e).map_or(0.0, |p| p.torque.z))),
            write: Some(|w, e, v| with_pending(w, e, |pf| pf.torque.z = v)),
        },
    ],
};

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
