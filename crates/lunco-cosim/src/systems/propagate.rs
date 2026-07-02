//! Connection propagation — the co-simulation master's exchange step.
//!
//! Implements the FMI-CS "read outputs → write inputs" exchange over every
//! [`crate::SimConnection`]. The propagated value is the SSP affine transform
//! `source * scale + offset`; multiple wires into one input **sum** (a
//! signal-flow junction — convenient for force accumulation, a deliberate
//! extension beyond FMI's 1:1 connections).
//!
//! ## Backend-agnostic by construction
//!
//! Every endpoint is addressed through the [`crate::ports`] resolver
//! ([`read_port`] / [`write_port`]), never through per-type queries. A new
//! port-bearing backend (Modelica, Avian, joint, `PhysicalPort`, …) joins the
//! whole wiring fabric by extending the resolver alone — this system never
//! changes. That also makes it front-end agnostic: an endpoint is an `Entity`
//! plus a port name, so USD, the API, and runtime spawns all wire the same way.

use std::collections::HashMap;

use bevy::prelude::*;

use lunco_core::ports::PortRegistry;

use crate::SimConnection;

/// System sets for co-simulation propagation.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CosimSet {
    /// Propagate connections: read outputs → write inputs.
    Propagate,
}

/// One compiled wire: source endpoint + affine gain + the *index* of its target
/// in [`CompiledWiring::targets`]. Connector names are owned here (cloned once at
/// compile time) so the per-tick hot loop touches no strings.
pub struct CompiledWire {
    pub src_entity: Entity,
    pub src_port: String,
    /// Index into [`CompiledWiring::targets`] — the accumulator slot.
    pub dst_index: usize,
    pub scale: f64,
    pub offset: f64,
}

/// The flattened wiring fabric — the "SignalBus" — rebuilt only when the
/// [`crate::SimConnection`] set actually changes (see [`rebuild_compiled_wiring`]).
///
/// Replaces the old per-tick snapshot (string-cloning every connector every
/// tick + a string-keyed `HashMap` accumulator). Targets are interned to dense
/// indices so propagation accumulates into a plain `Vec<f64>` with no hashing.
#[derive(Resource, Default)]
pub struct CompiledWiring {
    pub wires: Vec<CompiledWire>,
    /// Distinct targets `(entity, input port)`, one accumulator slot each.
    pub targets: Vec<(Entity, String)>,
}

/// Rebuilds [`CompiledWiring`] when the wire set changes.
///
/// Gated on `Changed<SimConnection>` (which subsumes `Added`, so an added or
/// edited wire triggers a rebuild) plus `RemovedComponents<SimConnection>` (a
/// removed component or despawned wire entity). On the overwhelming majority of
/// frames — values flowing, wiring static — this early-returns after two empty
/// checks, so the per-tick cost of the old snapshot is gone. Correctness is
/// unchanged: any structural or field edit rebuilds before the next propagate.
pub fn rebuild_compiled_wiring(
    q_changed: Query<(), Changed<SimConnection>>,
    mut removed: RemovedComponents<SimConnection>,
    q_all: Query<&SimConnection>,
    mut compiled: ResMut<CompiledWiring>,
) {
    // Always drain removed events (so the queue can't linger across early
    // returns), then decide whether anything changed.
    let removed_any = removed.read().count() > 0;
    if q_changed.is_empty() && !removed_any {
        return;
    }

    let compiled = &mut *compiled;
    compiled.wires.clear();
    compiled.targets.clear();
    let mut target_index: HashMap<(Entity, String), usize> = HashMap::new();

    for c in &q_all {
        if c.start_element == Entity::PLACEHOLDER || c.end_element == Entity::PLACEHOLDER {
            continue;
        }
        let key = (c.end_element, c.end_connector.clone());
        let dst_index = *target_index.entry(key).or_insert_with(|| {
            let i = compiled.targets.len();
            compiled.targets.push((c.end_element, c.end_connector.clone()));
            i
        });
        compiled.wires.push(CompiledWire {
            src_entity: c.start_element,
            src_port: c.start_connector.clone(),
            dst_index,
            scale: c.scale,
            offset: c.offset,
        });
    }
}

/// Propagates values through the compiled wiring fabric.
///
/// Exclusive system: it addresses arbitrary backends through the resolver,
/// which needs whole-world access. Reads [`CompiledWiring`] (built by
/// [`rebuild_compiled_wiring`]) — no per-tick query, string clone, or hash:
///
/// 1. **Seed** — every target's accumulator slot to `0.0`, so a target whose
///    source vanished cleanly returns to zero.
/// 2. **Accumulate** — read each source via [`PortRegistry::read_output_port`],
///    sum `src*scale+offset` into `acc[dst_index]`.
/// 3. **Write** — push each accumulated value to its input via
///    [`PortRegistry::write_port`], once per target, in stable (insertion)
///    order. A target with no such input port is a dangling wire — reported,
///    not silently dropped.
///
/// Undriven input ports are never touched, so a manual `SetPort` hold survives.
pub fn propagate_connections(world: &mut World, mut acc: Local<Vec<f64>>) {
    // Registry is a `Vec` of `Copy` backend fn-pointers; clone it out so the
    // write phase can take `&mut World` without holding a resource borrow.
    let registry = world.resource::<PortRegistry>().clone();

    // `resource_scope` lifts `CompiledWiring` out of the world for the duration
    // of the closure, giving `&mut World` *and* a borrow of the compiled fabric
    // at once — so we read the owned connector strings by reference (no clone)
    // while still calling the resolver's `&mut World` writes.
    world.resource_scope(|world, compiled: Mut<CompiledWiring>| {
        if compiled.targets.is_empty() {
            return;
        }

        // Phase 1: seed accumulator slots.
        acc.clear();
        acc.resize(compiled.targets.len(), 0.0);

        // Phase 2: accumulate.
        for w in &compiled.wires {
            let Some(src) = registry.read_output_port(world, w.src_entity, &w.src_port) else {
                continue; // source output absent — contributes nothing this tick
            };
            acc[w.dst_index] += src * w.scale + w.offset;
        }

        // Phase 3: write each target once through the resolver.
        for (i, (entity, name)) in compiled.targets.iter().enumerate() {
            if !registry.write_port(world, *entity, name, acc[i]) {
                warn_once!(
                    "[cosim] connection targets unknown input port '{}' on {:?} — value dropped \
                     (declare the port or fix the wire)",
                    name,
                    entity
                );
            }
        }
    });
}
