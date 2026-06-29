//! Uniform port table — the single surface for **discovering** and
//! **reading/writing** every exposed simulation value, whichever backend owns
//! it.
//!
//! A *port* is a named scalar (`f64`) on a participant entity. Modelica
//! variables ([`SimComponent`]), Avian forces/state ([`crate::AvianSim`]),
//! joint angles ([`crate::JointSim`]), and the SysML/hardware "nervous system"
//! ports ([`PhysicalPort`] / [`DigitalPort`]) all present as ports, so that
//! wires ([`crate::SimConnection`]), the API (`ListPorts` / `GetPort` /
//! `SetPort`), and the UI inspector treat them uniformly — the FMI/SSP contract.
//!
//! This module is **the** place that knows how each backend stores its values.
//! [`crate::systems::propagate::propagate_connections`] addresses every endpoint
//! through here, so a new port-bearing backend is wired into the whole fabric
//! by extending this module alone — `propagate` never changes again.
//!
//! ## Value model
//!
//! The wire currency is `f64` (continuous Real — what FMI-CS exchanges almost
//! everywhere). Typed backends convert at their own boundary: a [`DigitalPort`]
//! `i16` register reads as `f64` and saturates on write, exactly like a real
//! DAC/ADC. The connection's affine transform (`scale`/`offset`) carries the
//! gain. We deliberately do **not** model `Bool`/`Enum`/`String` ports until a
//! concrete need appears — physics co-simulation is all-Real.
//!
//! ## Canonical port names for single-value backends
//!
//! [`PhysicalPort`] and [`DigitalPort`] each hold one scalar, exposed under a
//! fixed name so wires can reference it: [`PHYSICAL_PORT_NAME`] (`"value"`) and
//! [`DIGITAL_PORT_NAME`] (`"raw"`).
//!
//! ## One backend table, four thin operations
//!
//! Every port-bearing backend is one [`PortBackend`] entry in [`BACKENDS`]
//! (list / read-output / read-input / write-input). The four public functions
//! ([`entity_ports`], [`read_output_port`], [`read_port`], [`write_port`]) just
//! fold over that table in order, so a new backend is added in **one place** —
//! no more editing four functions in lockstep (which silently drifted: each
//! risked a different backend order, a different name-shadowing outcome). The
//! table order *is* the resolution precedence (first match wins).
//!
//! ## TODO(ports): tag-driven exposure
//!
//! The table is still authored by hand. The next step is a
//! `#[derive(SimExposed)]` + `#[port(in|out)]` reflect-harvest that *generates*
//! [`BACKENDS`] entries from tagged component fields (the ontology's "Attribute
//! exposed via Bevy Reflection"). The function signatures and the table shape
//! stay stable so that lands as a drop-in: callers are unchanged.

use bevy::prelude::*;

use lunco_core::architecture::{DigitalPort, PhysicalPort};

use crate::connection::{PortDirection, PortType};
use crate::SimComponent;

/// The fixed port name a [`PhysicalPort`] exposes (its `f32` `value`).
pub const PHYSICAL_PORT_NAME: &str = "value";
/// The fixed port name a [`DigitalPort`] exposes (its `i16` `raw_value`).
pub const DIGITAL_PORT_NAME: &str = "raw";

/// A discovered port: identity, causality, physical domain, current value.
///
/// Returned by [`entity_ports`] for listing/introspection. The `value` is a
/// snapshot read at call time; live consumers read through the maps directly.
#[derive(Debug, Clone)]
pub struct PortRef {
    /// Port name — the key in the owning component's `inputs`/`outputs` map,
    /// or the canonical name for a single-value backend.
    pub name: String,
    /// Causality (key in `outputs` → `Out`, key in `inputs` → `In`, a
    /// single-value port → `InOut`).
    pub direction: PortDirection,
    /// Physical domain (best-effort classification — see [`classify`]).
    pub port_type: PortType,
    /// Snapshot of the current value.
    pub value: f64,
}

/// Best-effort physical-domain classification from a port name.
///
/// TODO(ports): drop this heuristic once backends publish `SimPort` metadata
/// (name → `PortType`) alongside their values — read the declared type instead
/// of guessing from the name. Type is used only for UI grouping and connection
/// validation, so a wrong guess is cosmetic, never load-bearing.
fn classify(name: &str) -> PortType {
    if name.starts_with("force") || name.starts_with("torque") {
        PortType::Force
    } else if name.starts_with("position")
        || name.starts_with("velocity")
        || name == "height"
        || name == "angle"
    {
        PortType::Kinematic
    } else if name.contains("temp") || name.contains("heat") {
        PortType::Thermal
    } else {
        PortType::Signal
    }
}

#[inline]
fn push_map(
    out: &mut Vec<PortRef>,
    map: &std::collections::HashMap<String, f64>,
    dir: PortDirection,
) {
    for (name, value) in map {
        out.push(PortRef {
            name: name.clone(),
            direction: dir,
            port_type: classify(name),
            value: *value,
        });
    }
}

/// One port-bearing backend, expressed as four operations over `(World,
/// Entity)`. The whole resolver is a fold over [`BACKENDS`] — entries are
/// non-capturing closures coerced to `fn` pointers, so the table is a `const`.
///
/// Each op is causality-correct: `read_output`/`read_input` see only the
/// matching direction, `write_input` only accepts an existing input slot (the
/// strictness that lets `propagate` report dangling wires). A single-value
/// backend ([`PhysicalPort`]/[`DigitalPort`]) is bidirectional — its one scalar
/// *is* both its output and its input, so it answers all three.
struct PortBackend {
    /// Append this backend's ports on `entity` (outputs then inputs) to `out`.
    list: fn(&World, Entity, &mut Vec<PortRef>),
    /// Read the **output** named `name`, or `None` if this backend has no such
    /// output on `entity`.
    read_output: fn(&World, Entity, &str) -> Option<f64>,
    /// Read the **input** named `name`, or `None`.
    read_input: fn(&World, Entity, &str) -> Option<f64>,
    /// Write `value` to **input** `name`; `true` iff the port existed here.
    write_input: fn(&mut World, Entity, &str, f64) -> bool,
}

/// One avian port: a named scalar on an avian component, with its causality,
/// physical domain, and read/write realization. Part of an [`AvianGroup`].
///
/// Avian's components are foreign types, so they are exposed by an external spec
/// (these closures) rather than `#[derive]` — the backend lives in this crate
/// (`crates/lunco-cosim`; original design in git history). Most ports are a
/// one-line field read; the few
/// derived/semantic ones (joint twist, motor target) are named functions.
pub struct AvianPort {
    /// Port name (e.g. `"position_y"`, `"force_y"`, `"angle"`).
    pub name: &'static str,
    /// Causality. `read_output` consults `Out`/`InOut`; `read_input`/`write`
    /// consult `In`/`InOut`.
    pub dir: PortDirection,
    /// Physical domain (UI grouping / future connection validation).
    pub port_type: PortType,
    /// Read the current value. `None` for a port with no readable backing.
    pub read: Option<fn(&World, Entity) -> Option<f64>>,
    /// Write the value. `None` for a read-only state output. `true` if applied.
    pub write: Option<fn(&mut World, Entity, f64) -> bool>,
}

/// A group of avian ports gated on a component's presence — one avian kind
/// (rigid body, revolute joint, …). Declared in [`crate::avian`] /
/// [`crate::joint`] and folded into the single avian [`PortBackend`] entry below.
/// Adding a kind (prismatic joint, sensor, …) is one entry in [`AVIAN`] plus its
/// group declaration — no new struct, observer, or system.
pub struct AvianGroup {
    /// Does `entity` belong to this group (carry the gating component)?
    pub present: fn(&World, Entity) -> bool,
    /// The ports this kind exposes.
    pub ports: &'static [AvianPort],
}

/// The avian backend table: every avian kind we expose, in one place.
const AVIAN: &[AvianGroup] = &[
    crate::avian::RIGID_BODY_GROUP,
    crate::joint::REVOLUTE_JOINT_GROUP,
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
            out.push(PortRef {
                name: p.name.to_string(),
                direction: p.dir,
                port_type: p.port_type,
                value,
            });
        }
    }
}

/// Resolve the first avian port named `name` whose causality satisfies `dir_ok`,
/// scanning [`AVIAN`] groups in precedence order and skipping groups whose gating
/// component is absent on `entity`. The returned reference is `'static` (groups
/// are `const`), so callers may drop the immutable `world` borrow and re-borrow
/// it mutably before invoking the port's `write`.
///
/// CQ-112: folds the identical group/present/name/direction scan that
/// [`avian_read_output`], [`avian_read_input`], and [`avian_write_input`] each
/// open-coded. Port `(name, dir)` pairs are unique within the avian table (the
/// joint's two `angle` ports differ by direction), so first-match resolution is
/// behaviour-identical to the prior "continue until a readable/writable match"
/// loops.
fn find_avian_port(
    world: &World,
    entity: Entity,
    name: &str,
    dir_ok: fn(PortDirection) -> bool,
) -> Option<&'static AvianPort> {
    for group in AVIAN {
        if !(group.present)(world, entity) {
            continue;
        }
        for p in group.ports {
            if p.name == name && dir_ok(p.dir) {
                return Some(p);
            }
        }
    }
    None
}

fn avian_read_output(world: &World, entity: Entity, name: &str) -> Option<f64> {
    let port = find_avian_port(world, entity, name, |d| {
        matches!(d, PortDirection::Out | PortDirection::InOut)
    })?;
    port.read?(world, entity)
}

fn avian_read_input(world: &World, entity: Entity, name: &str) -> Option<f64> {
    let port = find_avian_port(world, entity, name, |d| {
        matches!(d, PortDirection::In | PortDirection::InOut)
    })?;
    port.read?(world, entity)
}

fn avian_write_input(world: &mut World, entity: Entity, name: &str, value: f64) -> bool {
    let Some(port) = find_avian_port(world, entity, name, |d| {
        matches!(d, PortDirection::In | PortDirection::InOut)
    }) else {
        return false;
    };
    match port.write {
        Some(write) => write(world, entity, value),
        None => false,
    }
}

/// The backend table — **the** list of port-bearing component types, in
/// resolution-precedence order. Adding a backend is one entry here; the four
/// public functions below pick it up automatically.
const BACKENDS: &[PortBackend] = &[
    // --- Map-based backends: HashMap<String, f64> inputs/outputs ---
    PortBackend {
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
    },
    // Avian (rigid bodies + revolute joints), folded from the `AVIAN` spec
    // table. One entry replaces the former per-kind `AvianSim`/`JointSim`
    // backends; new avian kinds are added in `AVIAN`, not here.
    PortBackend {
        list: avian_list,
        read_output: avian_read_output,
        read_input: avian_read_input,
        write_input: avian_write_input,
    },
    // --- Single-value SysML/hardware ports: one bidirectional scalar each ---
    PortBackend {
        list: |w, e, out| {
            if let Some(p) = w.get::<PhysicalPort>(e) {
                out.push(PortRef {
                    name: PHYSICAL_PORT_NAME.to_string(),
                    direction: PortDirection::InOut,
                    port_type: PortType::Signal,
                    value: p.value as f64,
                });
            }
        },
        read_output: |w, e, n| {
            if n != PHYSICAL_PORT_NAME {
                return None;
            }
            w.get::<PhysicalPort>(e).map(|p| p.value as f64)
        },
        // Bidirectional: the one value is both output and input.
        read_input: |w, e, n| {
            if n != PHYSICAL_PORT_NAME {
                return None;
            }
            w.get::<PhysicalPort>(e).map(|p| p.value as f64)
        },
        write_input: |w, e, n, v| {
            if n != PHYSICAL_PORT_NAME {
                return false;
            }
            if let Some(mut p) = w.get_mut::<PhysicalPort>(e) {
                p.value = v as f32;
                return true;
            }
            false
        },
    },
    PortBackend {
        list: |w, e, out| {
            if let Some(d) = w.get::<DigitalPort>(e) {
                out.push(PortRef {
                    name: DIGITAL_PORT_NAME.to_string(),
                    direction: PortDirection::InOut,
                    port_type: PortType::Signal,
                    value: d.raw_value as f64,
                });
            }
        },
        read_output: |w, e, n| {
            if n != DIGITAL_PORT_NAME {
                return None;
            }
            w.get::<DigitalPort>(e).map(|d| d.raw_value as f64)
        },
        read_input: |w, e, n| {
            if n != DIGITAL_PORT_NAME {
                return None;
            }
            w.get::<DigitalPort>(e).map(|d| d.raw_value as f64)
        },
        write_input: |w, e, n, v| {
            if n != DIGITAL_PORT_NAME {
                return false;
            }
            if let Some(mut d) = w.get_mut::<DigitalPort>(e) {
                // DAC quantization: saturate the continuous value into the i16
                // register, exactly as real hardware clamps out-of-range commands.
                d.raw_value = v.round().clamp(i16::MIN as f64, i16::MAX as f64) as i16;
                return true;
            }
            false
        },
    },
];

/// Enumerate every exposed port on `entity`, across all port-bearing backends.
///
/// The backbone of `ListPorts` — folds [`BACKENDS`] in precedence order.
pub fn entity_ports(world: &World, entity: Entity) -> Vec<PortRef> {
    let mut out = Vec::new();
    for backend in BACKENDS {
        (backend.list)(world, entity, &mut out);
    }
    out
}

/// Read the **output** named `name` on `entity` — the value a connection reads
/// from its *source*.
///
/// A connection source must be an output, so this searches `outputs` only (plus
/// the bidirectional single-value ports, whose one value *is* their output).
/// This is critical when a name exists as both an input and an output on one
/// entity — e.g. a balloon's `height` is a `SimComponent` *input* and an
/// `AvianSim` *output*; the wire must read the AvianSim output, not the stale
/// SimComponent input. The read side of [`crate::systems::propagate`].
pub fn read_output_port(world: &World, entity: Entity, name: &str) -> Option<f64> {
    BACKENDS
        .iter()
        .find_map(|backend| (backend.read_output)(world, entity, name))
}

/// Read the current value of port `name` on `entity`, preferring an **output**
/// across all backends, then falling back to an **input**. `None` if no such
/// port. The backbone of `GetPort` (introspection wants the meaningful value
/// regardless of causality).
pub fn read_port(world: &World, entity: Entity, name: &str) -> Option<f64> {
    if let Some(v) = read_output_port(world, entity, name) {
        return Some(v);
    }
    BACKENDS
        .iter()
        .find_map(|backend| (backend.read_input)(world, entity, name))
}

/// Read the **input** value of port `name` on `entity` — the commanded side,
/// skipping outputs. `None` if no input port of that name exists.
///
/// Distinct from [`read_port`] (which prefers outputs): use this where the input
/// specifically is wanted — e.g. the inspector reading a joint's commanded motor
/// setpoint as opposed to its measured angle (both named `angle`).
pub fn read_input_port(world: &World, entity: Entity, name: &str) -> Option<f64> {
    BACKENDS
        .iter()
        .find_map(|backend| (backend.read_input)(world, entity, name))
}

/// Write `value` to **input** port `name` on `entity`. Returns `true` if such an
/// input port existed and was written.
///
/// Strict: only writes a port that already exists (an undeclared name is
/// rejected with `false`, never silently created) — this is what lets the API
/// and the propagation master report dangling wires. `Out`-only ports are not
/// writable; the single-value ports ([`PhysicalPort`]/[`DigitalPort`]) are
/// bidirectional and always accept a write (with saturation for the `i16`
/// register). First backend that owns the port wins.
///
/// Shared by `SetPort` and [`crate::systems::propagate::propagate_connections`].
///
/// TODO(ports): route a `SetPort` through a **ControlStream hold** so a manual
/// write survives the next propagate tick (latest-wins, overrides a live wire
/// until released — locked design decision 2). Today a wired input reverts next
/// tick; an unwired input holds.
pub fn write_port(world: &mut World, entity: Entity, name: &str, value: f64) -> bool {
    for backend in BACKENDS {
        if (backend.write_input)(world, entity, name, value) {
            return true;
        }
    }
    false
}
