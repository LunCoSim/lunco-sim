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
use lunco_core::RebuildOnChange;

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
struct CompiledWire {
    src_entity: Entity,
    src_port: String,
    /// Index into [`CompiledWiring::targets`] — the accumulator slot.
    dst_index: usize,
    scale: f64,
    offset: f64,
}

/// The flattened wiring fabric — the "SignalBus" — cached inside
/// [`propagate_connections`] and rebuilt only when the [`crate::SimConnection`]
/// set actually changes.
///
/// Replaces the old per-tick snapshot (string-cloning every connector every
/// tick + a string-keyed `HashMap` accumulator). Targets are interned to dense
/// indices so propagation accumulates into a plain `Vec<f64>` with no hashing.
#[derive(Default)]
pub struct CompiledWiring {
    wires: Vec<CompiledWire>,
    /// Distinct targets `(entity, input port)`, one accumulator slot each.
    targets: Vec<(Entity, String)>,
}

impl CompiledWiring {
    /// Recompile the fabric from the live [`SimConnection`] set. Runs only when
    /// the wiring changed (driven by [`RebuildOnChange`]).
    fn rebuild(&mut self, world: &mut World) {
        self.wires.clear();
        self.targets.clear();
        let mut target_index: HashMap<(Entity, String), usize> = HashMap::new();

        let mut q = world.query::<&SimConnection>();
        for c in q.iter(world) {
            if c.start_element == Entity::PLACEHOLDER || c.end_element == Entity::PLACEHOLDER {
                continue;
            }
            let key = (c.end_element, c.end_connector.clone());
            let dst_index = *target_index.entry(key).or_insert_with(|| {
                let i = self.targets.len();
                self.targets.push((c.end_element, c.end_connector.clone()));
                i
            });
            self.wires.push(CompiledWire {
                src_entity: c.start_element,
                src_port: c.start_connector.clone(),
                dst_index,
                scale: c.scale,
                offset: c.offset,
            });
        }
    }
}

/// Propagates values through the wiring fabric.
///
/// Exclusive system: it addresses arbitrary backends through the resolver,
/// which needs whole-world access. Self-contained — it caches the compiled
/// fabric in a `Local` and rebuilds it only when the [`crate::SimConnection`]
/// set changes, so calling this system alone (e.g. in tests, without the full
/// schedule) both compiles and propagates. No per-tick query snapshot, string
/// clone, or hash on the steady path:
///
/// 1. **Recompile-if-changed** — [`RebuildOnChange`] rebuilds the fabric only
///    when the `SimConnection` set changes (`Changed`/`Added`/`Removed`, plus a
///    forced first run), so this system stays self-contained yet allocation-free
///    on the steady path.
/// 2. **Seed** — every target's accumulator slot to `0.0`, so a target whose
///    source vanished cleanly returns to zero.
/// 3. **Accumulate** — read each source via [`PortRegistry::read_output_port`],
///    sum `src*scale+offset` into `acc[dst_index]`.
/// 4. **Write** — push each accumulated value to its input via
///    [`PortRegistry::write_port`], once per target, in stable (insertion)
///    order. A target with no such input port is a dangling wire — reported,
///    not silently dropped.
///
/// Undriven input ports are never touched, so a manual `SetPort` hold survives.
pub fn propagate_connections(
    world: &mut World,
    mut wiring: Local<RebuildOnChange<SimConnection, CompiledWiring>>,
    mut acc: Local<Vec<f64>>,
) {
    // Registry is a `Vec` of `Copy` backend fn-pointers; clone it out so the
    // write phase can take `&mut World` without holding a resource borrow.
    let registry = world.resource::<PortRegistry>().clone();

    // Phase 1: recompile the fabric iff the connection set changed. The compiled
    // fabric is owned by the `Local` (no world borrow), so the phases below keep
    // `&mut World` for the resolver.
    let compiled = wiring.get_or_rebuild(world, |compiled, world| compiled.rebuild(world));

    if compiled.targets.is_empty() {
        return;
    }

    // Phase 2: seed accumulator slots.
    acc.clear();
    acc.resize(compiled.targets.len(), 0.0);

    // Phase 3: accumulate.
    for w in &compiled.wires {
        let Some(src) = registry.read_output_port(world, w.src_entity, &w.src_port) else {
            continue; // source output absent — contributes nothing this tick
        };
        acc[w.dst_index] += src * w.scale + w.offset;
    }

    // Phase 4: write each target once through the resolver.
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
}
