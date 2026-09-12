//! The cosim engine's port **backends** and their registration into the shared
//! [`PortRegistry`].
//!
//! The registry itself, its discovery/access operations, and the value types
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
//!   predicate, identity key, and structural invalidation hook.
//! - **SysML/hardware** single-value [`Port`]s — one bidirectional scalar each.
//!
//! Registration order *is* resolution precedence (first match wins): Modelica,
//! avian, then the single-value ports — see [`register_builtin_port_backends`].

use bevy::prelude::*;
use std::hash::{Hash, Hasher};

use lunco_core::architecture::{InputPorts, OutputPorts, Port, PortSurface};
use lunco_core::ports::{
    port_entity_map_key, port_name_set_key, push_map, PortBackend, PortDirection, PortMetadata,
    PortRef, PortRegistry, PortTopologyRevision, PortTopologyState,
};

use crate::{DeclaredOutputPorts, SimComponent};

/// The fixed port name a [`Port`] exposes (its `f64` `value`).
pub const PORT_NAME: &str = "value";

/// One avian port: a named scalar on an avian component, with its causality,
/// physical domain, and read/write realization. Part of an [`AvianGroup`].
///
/// Avian's components are foreign types, so they are exposed by these closures
/// rather than `#[derive]`. Most ports are a one-line field read; the few
/// derived/semantic ones (joint twist, motor target) are named functions.
#[derive(Clone, Copy)]
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
/// below. Adding a kind (a raw physics query, a D6 joint, …) is one entry in [`AVIAN`]
/// plus its group declaration, structural key, and invalidation hook.
pub struct AvianGroup {
    /// Does `entity` belong to this group (carry the gating component)?
    pub present: fn(&World, Entity) -> bool,
    /// Append every entity that can belong to this group to `out`.
    pub entities: fn(&mut World, &mut Vec<Entity>),
    /// Return the identity key for the ports emitted by this group on `entity`.
    ///
    /// The key is zero when the group is absent and must ignore live samples.
    /// It must still include structural backing-component presence for ports
    /// whose `read` callback can return `None`; the backend candidate key is
    /// what lets the UI rebuild a row when that backing component appears or
    /// disappears.
    pub topology_key: fn(&World, Entity) -> u64,
    /// The ports this kind exposes.
    pub ports: &'static [AvianPort],
    /// Install the lifecycle and structural checks that can change this group.
    ///
    /// Keeping this beside the group declaration makes adding a new Avian port
    /// family an atomic change: its candidate predicate, ports, and invalidation
    /// owner cannot drift into a separate application-composition list.
    pub install_topology: fn(&mut App),
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
    crate::avian::KINEMATIC_POSITION_GROUP,
    crate::avian::FORCE_ACTUATOR_GROUP,
    crate::avian::TORQUE_ACTUATOR_GROUP,
    crate::avian::COLLIDER_CONTACT_GROUP,
    crate::joint::REVOLUTE_JOINT_GROUP,
    crate::joint::PRISMATIC_JOINT_GROUP,
    crate::avian_queries::RAYCAST_GROUP,
];

/// Install every Avian group's own topology watcher.
///
/// The group table is the authoritative composition point for Avian ports. A
/// group cannot be added without also providing the lifecycle/structural hook
/// that keeps the durable UI invalidation generation correct.
pub(crate) fn register_avian_port_topology(app: &mut App) {
    for group in AVIAN {
        (group.install_topology)(app);
    }
}

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
                value,
            });
        }
    }
}

fn avian_entities(world: &mut World, out: &mut Vec<Entity>) {
    for group in AVIAN {
        (group.entities)(world, out);
    }
}

fn avian_topology_key(world: &World, entity: Entity) -> u64 {
    AVIAN.iter().enumerate().fold(0u64, |key, (index, group)| {
        key ^ (group.topology_key)(world, entity).rotate_left((index * 8) as u32)
    })
}

fn avian_unit(name: &str) -> Option<&'static str> {
    if name.starts_with("position_")
        || name.starts_with("ray_hit_position_")
        || name == "displacement"
        || name == "ray_distance"
    {
        Some("m")
    } else if name.starts_with("velocity_") || name == "velocity" {
        Some("m/s")
    } else if name.starts_with("angvel_") {
        Some("rad/s")
    } else if name == "angle" {
        Some("rad")
    } else if name.starts_with("force_") || name == "force" {
        Some("N")
    } else if name.starts_with("torque_") || name == "torque" {
        Some("N·m")
    } else if name == "ray_sample_time" {
        Some("s")
    } else {
        None
    }
}

fn avian_metadata(
    world: &World,
    entity: Entity,
    name: &str,
    direction: PortDirection,
) -> PortMetadata {
    const SOURCES: [&str; 8] = [
        "Avian rigid body",
        "Avian kinematic body",
        "Avian force actuator",
        "Avian torque actuator",
        "Avian contact solver",
        "Avian revolute joint",
        "Avian prismatic joint",
        "Avian ray query",
    ];
    for (group_index, group) in AVIAN.iter().enumerate() {
        if !(group.present)(world, entity) {
            continue;
        }
        if let Some(port) = group
            .ports
            .iter()
            .find(|port| port.name == name && port.dir == direction)
        {
            return PortMetadata::scalar(
                direction,
                avian_unit(name),
                None,
                None,
                SOURCES[group_index],
                if port.write.is_some() {
                    "control owner"
                } else {
                    "physics solver"
                },
                port.write.is_some(),
            );
        }
    }
    PortMetadata::unknown(direction)
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
    avian_resolve(world, entity, name, |d| {
        matches!(d, PortDirection::Out | PortDirection::InOut)
    })
}

fn avian_resolve_input(world: &World, entity: Entity, name: &str) -> Option<u64> {
    avian_resolve(world, entity, name, |d| {
        matches!(d, PortDirection::In | PortDirection::InOut)
    })
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
fn sim_component_topology_key(
    component: &SimComponent,
    declared: Option<&DeclaredOutputPorts>,
) -> u64 {
    let inputs = port_name_set_key(component.inputs.keys());
    let outputs = port_name_set_key(component.outputs.keys());
    let declared = declared
        .map(|ports| port_name_set_key(ports.names.iter()))
        .unwrap_or(0);
    inputs ^ outputs.rotate_left(21) ^ declared.rotate_left(42)
}

const SIMCOMPONENT_BACKEND: PortBackend = PortBackend {
    list_entities: |world, out| {
        out.extend(
            world
                .query_filtered::<Entity, With<SimComponent>>()
                .iter(world),
        );
    },
    topology_key: |world, entity| {
        let Some(component) = world.get::<SimComponent>(entity) else {
            return 0;
        };
        sim_component_topology_key(component, world.get::<DeclaredOutputPorts>(entity))
    },
    list: |w, e, out| {
        if let Some(c) = w.get::<SimComponent>(e) {
            push_map(out, &c.outputs, PortDirection::Out);
            if let Some(declared) = w.get::<DeclaredOutputPorts>(e) {
                for name in &declared.names {
                    if !c.outputs.contains_key(name) {
                        out.push(PortRef {
                            name: name.clone(),
                            direction: PortDirection::Out,
                            value: 0.0,
                        });
                    }
                }
            }
            push_map(out, &c.inputs, PortDirection::In);
        }
    },
    metadata: Some(|world, entity, _name, direction| {
        let source = world
            .get::<SimComponent>(entity)
            .map(|component| component.model_name.clone())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "Modelica component".into());
        let authority = if matches!(direction, PortDirection::Out) {
            "solver"
        } else {
            "controller / wire"
        };
        PortMetadata::scalar(direction, None, None, None, source, authority, true)
    }),
    read_output: |w, e, n| {
        w.get::<SimComponent>(e)
            .and_then(|c| c.outputs.get(n).copied())
    },
    read_input: |w, e, n| {
        w.get::<SimComponent>(e)
            .and_then(|c| c.inputs.get(n).copied())
    },
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
    list_entities: avian_entities,
    topology_key: avian_topology_key,
    list: avian_list,
    metadata: Some(avian_metadata),
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
    list_entities: |world, out| {
        out.extend(world.query_filtered::<Entity, With<Port>>().iter(world));
    },
    topology_key: |world, entity| u64::from(world.get::<Port>(entity).is_some()),
    list: |w, e, out| {
        if let Some(p) = w.get::<Port>(e) {
            out.push(PortRef {
                name: PORT_NAME.to_string(),
                direction: PortDirection::InOut,
                value: p.value,
            });
        }
    },
    metadata: Some(|_world, _entity, _name, direction| {
        PortMetadata::scalar(
            direction,
            None,
            None,
            None,
            "hardware port",
            "controller / plant",
            true,
        )
    }),
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

/// Generic runtime output surface backed by child [`Port`] entities.
///
/// Imperative producers use this when their authored `outputs:*` values are
/// not owned by a `SimComponent` (for example an authored drivetrain program).
/// The producer's
/// output names remain visible to the common port registry, but they are
/// read-only here: commands enter through [`InputPorts`], and a producer owns
/// the writes to its outputs.
const OUTPUT_PORTS_BACKEND: PortBackend = PortBackend {
    list_entities: |world, out| {
        out.extend(
            world
                .query_filtered::<Entity, With<OutputPorts>>()
                .iter(world),
        );
    },
    topology_key: |world, entity| {
        let Some(outputs) = world.get::<OutputPorts>(entity) else {
            return 0;
        };
        let names = output_ports_topology_key(outputs);
        let live = outputs
            .ports
            .values()
            .filter(|port_entity| world.get::<Port>(**port_entity).is_some())
            .count() as u64;
        names ^ live.rotate_left(47)
    },
    list: |world, entity, out| {
        let Some(outputs) = world.get::<OutputPorts>(entity) else {
            return;
        };
        for (name, port_entity) in &outputs.ports {
            if let Some(port) = world.get::<Port>(*port_entity) {
                out.push(PortRef {
                    name: name.clone(),
                    direction: PortDirection::Out,
                    value: port.value,
                });
            }
        }
    },
    metadata: Some(|_world, _entity, _name, direction| {
        PortMetadata::scalar(
            direction,
            None,
            None,
            None,
            "runtime producer",
            "producer",
            false,
        )
    }),
    read_output: |world, entity, name| {
        world
            .get::<OutputPorts>(entity)
            .and_then(|outputs| outputs.get(name))
            .and_then(|port_entity| world.get::<Port>(port_entity))
            .map(|port| port.value)
    },
    read_input: |_world, _entity, _name| None,
    write_input: |_world, _entity, _name, _value| false,
    resolve_output: None,
    resolve_input: None,
    read_slot: None,
    write_slot: None,
};

fn output_ports_topology_key(outputs: &OutputPorts) -> u64 {
    port_entity_map_key(outputs.ports.iter())
}

fn port_surface_topology_key(surface: &PortSurface) -> u64 {
    port_entity_map_key(surface.ports.iter())
}

/// Return the identity of a connection's endpoints and port directions.
///
/// The affine transform is intentionally excluded: changing `scale` or
/// `offset` changes propagation values, not the connection topology or the
/// port candidate surface. Endpoint/name/direction edits are structural and
/// must reopen the durable port projection gate even when the component stays
/// on the same entity.
fn connection_topology_key(connection: &crate::SimConnection) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    connection.start_element.hash(&mut hasher);
    connection.start_connector.hash(&mut hasher);
    connection.start_is_input.hash(&mut hasher);
    connection.end_element.hash(&mut hasher);
    connection.end_connector.hash(&mut hasher);
    hasher.finish()
}

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
    list_entities: |world, out| {
        out.extend(
            world
                .query_filtered::<Entity, With<InputPorts>>()
                .iter(world),
        );
    },
    topology_key: |world, entity| u64::from(world.get::<InputPorts>(entity).is_some()),
    list: |w, e, out| {
        // `GlobalEntityId` names every composed USD prim, not just a vehicle.
        // The `InputPorts` surface is the architecture's already-authoritative
        // command and possession boundary (see `lunco_core::InputPorts`).
        // `ControlBinding` is merely an input-device adapter and `OutputPorts`
        // are mechanical output plumbing, so neither defines this port's owner.
        // Never manufacture `piloted` on meshes, joints, sensors, or arbitrary
        // Modelica children merely because they happen to have a stable id.
        if w.get::<InputPorts>(e).is_some() {
            out.push(PortRef {
                name: "piloted".to_string(),
                direction: PortDirection::Out,
                value: piloted_value(w, e),
            });
        }
    },
    metadata: Some(|_world, _entity, _name, direction| {
        PortMetadata::scalar(
            direction,
            None,
            Some(0.0),
            Some(1.0),
            "session registry",
            "possession",
            false,
        )
    }),
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

/// Detect in-place changes to the map-backed port owners without sampling every
/// owner. Bevy's change filter identifies the small set of components touched by
/// a producer; the structural key then ignores their live values. This is the
/// durable check for map-backed surfaces, while component add/remove observers
/// cover candidate membership and foreign Avian components.
pub(crate) fn check_port_owner_structure(
    input_ports: Query<(Entity, &InputPorts), Changed<InputPorts>>,
    components: Query<
        (Entity, &SimComponent, Option<&DeclaredOutputPorts>),
        Or<(Changed<SimComponent>, Changed<DeclaredOutputPorts>)>,
    >,
    output_ports: Query<(Entity, &OutputPorts), Changed<OutputPorts>>,
    port_surfaces: Query<(Entity, &PortSurface), Changed<PortSurface>>,
    mut state: ResMut<PortTopologyState>,
    mut revision: ResMut<PortTopologyRevision>,
) {
    for (entity, inputs) in &input_ports {
        if state.changed::<InputPorts>(entity, port_name_set_key(inputs.values.keys())) {
            revision.bump();
        }
    }
    for (entity, component, declared) in &components {
        if state.changed::<SimComponent>(entity, sim_component_topology_key(component, declared)) {
            revision.bump();
        }
    }
    for (entity, outputs) in &output_ports {
        if state.changed::<OutputPorts>(entity, output_ports_topology_key(outputs)) {
            revision.bump();
        }
    }
    for (entity, surface) in &port_surfaces {
        if state.changed::<PortSurface>(entity, port_surface_topology_key(surface)) {
            revision.bump();
        }
    }
}

/// Detect in-place edits to authored connection endpoints. Add/remove
/// observers cover connection membership; this check covers rewiring a
/// `SimConnection` component without replacing it. Affine value changes are
/// deliberately excluded because they do not change topology.
pub(crate) fn check_connection_structure(
    changed: Query<(Entity, &crate::SimConnection), Changed<crate::SimConnection>>,
    mut state: ResMut<PortTopologyState>,
    mut revision: ResMut<PortTopologyRevision>,
) {
    for (entity, connection) in &changed {
        if state.changed::<crate::SimConnection>(entity, connection_topology_key(connection)) {
            revision.bump();
        }
    }
}

/// Install the lifecycle observers owned by the built-in cosimulation port
/// providers.
///
/// Candidate membership is a component-lifecycle fact. In-place declarations
/// are handled by the two structural checks registered by [`CoSimPlugin`]; the
/// observers here only cover add/remove transitions. The Avian provider is
/// composed from [`AvianGroup`] declarations, each of which installs its own
/// component watchers and any value-to-membership check it requires.
pub(crate) fn register_builtin_port_topology(app: &mut App) {
    app.add_observer(lunco_core::ports::bump_port_topology_on_add::<InputPorts>)
        .add_observer(lunco_core::ports::bump_port_topology_on_remove::<InputPorts>)
        .add_observer(lunco_core::ports::bump_port_topology_on_add::<OutputPorts>)
        .add_observer(lunco_core::ports::bump_port_topology_on_remove::<OutputPorts>)
        .add_observer(lunco_core::ports::bump_port_topology_on_add::<PortSurface>)
        .add_observer(lunco_core::ports::bump_port_topology_on_remove::<PortSurface>)
        .add_observer(lunco_core::ports::bump_port_topology_on_add::<Port>)
        .add_observer(lunco_core::ports::bump_port_topology_on_remove::<Port>)
        .add_observer(lunco_core::ports::bump_port_topology_on_add::<SimComponent>)
        .add_observer(lunco_core::ports::bump_port_topology_on_remove::<SimComponent>)
        .add_observer(lunco_core::ports::bump_port_topology_on_add::<DeclaredOutputPorts>)
        .add_observer(lunco_core::ports::bump_port_topology_on_remove::<DeclaredOutputPorts>)
        .add_observer(lunco_core::ports::bump_port_topology_on_add::<crate::SimConnection>)
        .add_observer(lunco_core::ports::bump_port_topology_on_remove::<crate::SimConnection>);
    register_avian_port_topology(app);
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
    registry.register(OUTPUT_PORTS_BACKEND);
    registry.register(PILOTED_BACKEND);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn piloted_is_exposed_only_at_a_control_boundary() {
        let mut world = World::new();
        let mesh = world.spawn(lunco_core::GlobalEntityId::from_raw(1)).id();
        let vessel = world
            .spawn((
                lunco_core::GlobalEntityId::from_raw(2),
                InputPorts::new(&["throttle"]),
            ))
            .id();
        let mut ports = PortRegistry::default();
        register_builtin_port_backends(&mut ports);

        assert!(ports
            .entity_ports(&world, mesh)
            .iter()
            .all(|port| port.name != "piloted"));
        assert!(ports
            .entity_ports(&world, vessel)
            .iter()
            .any(|port| port.name == "piloted"));
    }

    #[test]
    fn runtime_outputs_are_read_only_and_visible_on_their_producer() {
        let mut world = World::new();
        let port = world.spawn(Port { value: 0.75 }).id();
        let producer = world
            .spawn(OutputPorts::new(std::collections::HashMap::from([(
                "drive_left".into(),
                port,
            )])))
            .id();
        let mut registry = PortRegistry::default();
        register_builtin_port_backends(&mut registry);

        assert_eq!(
            registry.read_output_port(&world, producer, "drive_left"),
            Some(0.75)
        );
        assert_eq!(
            registry
                .entity_ports(&world, producer)
                .into_iter()
                .find(|port| port.name == "drive_left")
                .map(|port| (port.direction, port.value)),
            Some((PortDirection::Out, 0.75))
        );
        assert!(!registry.write_port(&mut world, producer, "drive_left", 0.1));
        assert_eq!(world.get::<Port>(port).unwrap().value, 0.75);
    }
}
