//! The cosim engine's port **backends** and their registration into the shared
//! [`PortRegistry`].
//!
//! The registry itself, the four query operations, and the value types
//! ([`PortRef`], [`PortBackend`], [`PortDirection`]) live in
//! [`lunco_core::ports`] — the neutral substrate *below* every participant — so
//! that wires, the API, the inspector, and every scripting runtime read/write
//! through one surface without depending "up" into this engine. This module only
//! supplies the cosim-owned backends and registers them via
//! [`register_builtin_port_backends`].
//!
//! Three kinds of backend live here:
//! - **Modelica** [`SimComponent`] — `HashMap<String, f64>` inputs/outputs.
//! - **Avian** rigid bodies + revolute/prismatic joints — foreign components
//!   exposed by an external spec ([`AvianPort`]/[`AvianGroup`]) rather than
//!   `#[derive]`. Adding an avian kind is one entry in [`AVIAN`] plus its group
//!   declaration.
//! - **SysML/hardware** single-value [`Port`]s — one bidirectional scalar each.
//!
//! Registration order *is* resolution precedence (first match wins): Modelica,
//! avian, then the single-value ports — see [`register_builtin_port_backends`].

use bevy::prelude::*;

use lunco_core::architecture::Port;
use lunco_core::ports::{push_map, PortBackend, PortDirection, PortRef, PortRegistry};

use crate::SimComponent;

/// The fixed port name a [`Port`] exposes (its `f64` `value`).
pub const PORT_NAME: &str = "value";

/// One avian port: a named scalar on an avian component, with its causality,
/// physical domain, and read/write realization. Part of an [`AvianGroup`].
///
/// Avian's components are foreign types, so they are exposed by these closures
/// rather than `#[derive]`. Most ports are a one-line field read; the few
/// derived/semantic ones (joint twist, motor target) are named functions.
pub struct AvianPort {
    /// Port name (e.g. `"position_y"`, `"force_y"`, `"angle"`).
    pub name: &'static str,
    /// Causality. `read_output` consults `Out`/`InOut`; `read_input`/`write`
    /// consult `In`/`InOut`.
    pub dir: PortDirection,
    /// Read the current value. `None` for a port with no readable backing.
    pub read: Option<fn(&World, Entity) -> Option<f64>>,
    /// Write the value. `None` for a read-only state output. `true` if applied.
    pub write: Option<fn(&mut World, Entity, f64) -> bool>,
}

/// A group of avian ports gated on a component's presence — one avian kind
/// (rigid body, revolute joint, prismatic joint, …). Declared in
/// [`crate::avian`] / [`crate::joint`] and folded into the avian [`PortBackend`]
/// below. Adding a kind (a sensor, a D6 joint, …) is one entry in [`AVIAN`] plus
/// its group declaration — no new struct, observer, or system.
pub struct AvianGroup {
    /// Does `entity` belong to this group (carry the gating component)?
    pub present: fn(&World, Entity) -> bool,
    /// The ports this kind exposes.
    pub ports: &'static [AvianPort],
}

/// The avian backend table: every avian kind we expose, in one place.
///
/// Ordered by LAYER, because the two layers answer different questions. The first
/// four are PHYSICS — what the solver knows about a body, a collider or a joint,
/// exposed because the thing exists and nobody had to author an instrument to
/// notice. The last three are INSTRUMENTS — authored in USD, mounted at a point,
/// read by onboard control. Instruments CONSUME the physics layer; they do not
/// compete with it, which is why the touchdown switch and the collider contact
/// ports share one computation (`crate::avian::contact_of`).
pub(crate) const AVIAN: &[AvianGroup] = &[
    crate::avian::RIGID_BODY_GROUP,
    crate::avian::COLLIDER_CONTACT_GROUP,
    crate::joint::REVOLUTE_JOINT_GROUP,
    crate::joint::PRISMATIC_JOINT_GROUP,
    crate::sensors::IMU_SENSOR_GROUP,
    crate::sensors::RANGE_SENSOR_GROUP,
    crate::sensors::CONTACT_SENSOR_GROUP,
];

fn avian_list(world: &World, entity: Entity, out: &mut Vec<PortRef>) {
    for group in AVIAN {
        if !(group.present)(world, entity) {
            continue;
        }
        for p in group.ports {
            // A readable port whose backing component is absent (e.g. velocity
            // on a kinematic body) simply doesn't list; a write-only declared
            // port lists with value 0.
            let value = match p.read {
                Some(read) => match read(world, entity) {
                    Some(v) => v,
                    None => continue,
                },
                None => 0.0,
            };
            out.push(PortRef { name: p.name.to_string(), direction: p.dir, value });
        }
    }
}

/// Encode an avian slot: `(group index << 16) | port index` into [`AVIAN`]. The
/// slot is a process-local [`lunco_core::ports::ResolvedPort`] locator — never
/// serialized (see the value model note in `lunco_core::ports`).
fn avian_slot(group_index: usize, port_index: usize) -> u64 {
    ((group_index as u64) << 16) | (port_index as u64)
}

/// Decode an avian slot back to its `'static` [`AvianPort`] (groups are `const`).
/// `None` if the slot is out of range (a stale slot from a bumped table).
fn avian_decode(slot: u64) -> Option<&'static AvianPort> {
    let gi = (slot >> 16) as usize;
    let pi = (slot & 0xffff) as usize;
    AVIAN.get(gi)?.ports.get(pi)
}

/// Resolve the first avian port named `name` whose causality satisfies `dir_ok`
/// to its [`avian_slot`], scanning [`AVIAN`] groups in precedence order and
/// skipping groups whose gating component is absent on `entity`. This is the
/// ONE scan (group-presence + name compare); once resolved, the hot loop reads
/// by slot with a single component access and no re-scan.
fn avian_resolve(
    world: &World,
    entity: Entity,
    name: &str,
    dir_ok: fn(PortDirection) -> bool,
) -> Option<u64> {
    for (gi, group) in AVIAN.iter().enumerate() {
        if !(group.present)(world, entity) {
            continue;
        }
        for (pi, p) in group.ports.iter().enumerate() {
            if p.name == name && dir_ok(p.dir) {
                return Some(avian_slot(gi, pi));
            }
        }
    }
    None
}

fn avian_resolve_output(world: &World, entity: Entity, name: &str) -> Option<u64> {
    avian_resolve(world, entity, name, |d| matches!(d, PortDirection::Out | PortDirection::InOut))
}

fn avian_resolve_input(world: &World, entity: Entity, name: &str) -> Option<u64> {
    avian_resolve(world, entity, name, |d| matches!(d, PortDirection::In | PortDirection::InOut))
}

/// Read the value at a resolved avian slot. The port's `read` does the single
/// component access, returning `None` if that component was removed since
/// resolution — so a stale slot degrades to "no value" (skipped), never a wrong
/// read.
fn avian_read_slot(world: &World, entity: Entity, slot: u64) -> Option<f64> {
    avian_decode(slot)?.read?(world, entity)
}

fn avian_write_slot(world: &mut World, entity: Entity, slot: u64, value: f64) -> bool {
    let Some(port) = avian_decode(slot) else {
        return false;
    };
    match port.write {
        Some(write) => write(world, entity, value),
        None => false,
    }
}

// The name-based ops are DERIVED from the resolve→slot model (no duplicated scan):
// resolve once, then read/write by slot.
fn avian_read_output(world: &World, entity: Entity, name: &str) -> Option<f64> {
    avian_read_slot(world, entity, avian_resolve_output(world, entity, name)?)
}

fn avian_read_input(world: &World, entity: Entity, name: &str) -> Option<f64> {
    avian_read_slot(world, entity, avian_resolve_input(world, entity, name)?)
}

fn avian_write_input(world: &mut World, entity: Entity, name: &str, value: f64) -> bool {
    match avian_resolve_input(world, entity, name) {
        Some(slot) => avian_write_slot(world, entity, slot, value),
        None => false,
    }
}

/// Modelica `SimComponent` — map-based `inputs`/`outputs`.
const SIMCOMPONENT_BACKEND: PortBackend = PortBackend {
    list: |w, e, out| {
        if let Some(c) = w.get::<SimComponent>(e) {
            push_map(out, &c.outputs, PortDirection::Out);
            push_map(out, &c.inputs, PortDirection::In);
        }
    },
    read_output: |w, e, n| w.get::<SimComponent>(e).and_then(|c| c.outputs.get(n).copied()),
    read_input: |w, e, n| w.get::<SimComponent>(e).and_then(|c| c.inputs.get(n).copied()),
    write_input: |w, e, n, v| {
        if let Some(mut c) = w.get_mut::<SimComponent>(e) {
            if c.inputs.contains_key(n) {
                c.inputs.insert(n.to_string(), v);
                return true;
            }
        }
        false
    },
    // No fast path: registered first, so a name read already hits on one
    // `get::<SimComponent>` — resolution would not remove the map lookup.
    resolve_output: None,
    resolve_input: None,
    read_slot: None,
    write_slot: None,
};

/// Avian rigid bodies + revolute/prismatic joints, folded from the [`AVIAN`]
/// spec table. Exposes the resolve→slot fast path: registered behind
/// `SimComponent`, its name reads otherwise pay a `get::<SimComponent>` miss plus
/// up to six group-presence checks + a name scan — resolution collapses that to a
/// single component access per tick.
const AVIAN_BACKEND: PortBackend = PortBackend {
    list: avian_list,
    read_output: avian_read_output,
    read_input: avian_read_input,
    write_input: avian_write_input,
    resolve_output: Some(avian_resolve_output),
    resolve_input: Some(avian_resolve_input),
    read_slot: Some(avian_read_slot),
    write_slot: Some(avian_write_slot),
};

/// SysML/hardware [`Port`] — one bidirectional `f64` scalar named `value`.
///
/// The value crosses this backend unchanged in both directions: a Modelica model
/// on the far side of a [`crate::SimConnection`] exchanges `f64`, and so does the
/// port it is wired to.
const PORT_BACKEND: PortBackend = PortBackend {
    list: |w, e, out| {
        if let Some(p) = w.get::<Port>(e) {
            out.push(PortRef {
                name: PORT_NAME.to_string(),
                direction: PortDirection::InOut,
                value: p.value,
            });
        }
    },
    read_output: |w, e, n| {
        if n != PORT_NAME {
            return None;
        }
        w.get::<Port>(e).map(|p| p.value)
    },
    read_input: |w, e, n| {
        if n != PORT_NAME {
            return None;
        }
        w.get::<Port>(e).map(|p| p.value)
    },
    write_input: |w, e, n, v| {
        if n != PORT_NAME {
            return false;
        }
        if let Some(mut p) = w.get_mut::<Port>(e) {
            p.value = v;
            return true;
        }
        false
    },
    // Single fixed port on one component — name-based is already a single `get`.
    resolve_output: None,
    resolve_input: None,
    read_slot: None,
    write_slot: None,
};

/// Control-authority sensor: a read-only `piloted` port, 1.0 while the vessel is
/// possessed by ANY external session — a human user OR an autopilot (both are
/// external session-controllers) — else 0.0. It reports only POSSESSION STATUS from
/// the single source of truth ([`SessionRegistry`]); it treats every session
/// uniformly, with no autopilot-specific or role logic.
///
/// This is "the INTERNAL controller yields to whoever possesses it": the vessel's
/// intrinsic GNC (an in-model controller) wires `piloted` and gates
/// `cmd = piloted ? session : gnc`. Because it's WIRED it's a live input (reaches
/// the solver, unlike a folded flag). Session-vs-session (user vs autopilot) is
/// arbitrated by possession + RBAC upstream; the GNC is simply the floor beneath
/// the whole session layer.
const PILOTED_BACKEND: PortBackend = PortBackend {
    list: |w, e, out| {
        if w.get::<lunco_core::GlobalEntityId>(e).is_some() {
            out.push(PortRef {
                name: "piloted".to_string(),
                direction: PortDirection::Out,
                value: piloted_value(w, e),
            });
        }
    },
    read_output: |w, e, n| (n == "piloted").then(|| piloted_value(w, e)),
    read_input: |_, _, _| None,
    write_input: |_, _, _, _| false,
    resolve_output: None,
    resolve_input: None,
    read_slot: None,
    write_slot: None,
};

/// 1.0 iff this entity's vessel is owned by some session (possessed), else 0.0.
fn piloted_value(w: &World, e: Entity) -> f64 {
    let Some(gid) = w.get::<lunco_core::GlobalEntityId>(e).map(|g| g.get()) else {
        return 0.0;
    };
    let owned = w
        .get_resource::<lunco_core::SessionRegistry>()
        .is_some_and(|r| r.owner_of(gid).is_some());
    if owned {
        1.0
    } else {
        0.0
    }
}

/// Register the cosim engine's builtin port backends into `registry`, in
/// resolution-precedence order: Modelica `SimComponent`, avian state, then the
/// single-value hardware [`Port`]. Called from [`crate::CoSimPlugin`]. Other
/// crates (a future FMU import, a script-defined component) register their own
/// backends after these.
pub fn register_builtin_port_backends(registry: &mut PortRegistry) {
    registry.register(SIMCOMPONENT_BACKEND);
    registry.register(AVIAN_BACKEND);
    registry.register(PORT_BACKEND);
    registry.register(PILOTED_BACKEND);
}
