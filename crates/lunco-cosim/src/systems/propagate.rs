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

use lunco_core::ports::{PortRegistry, ResolvedPort};
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
///
/// `src_resolved` caches the FMI-style [`ResolvedPort`] handle when a fast-path
/// backend (avian) owns the source, so the accumulate phase reads by slot — one
/// component access, no cross-backend fold or group scan. `None` when no
/// fast-path backend owns it (map-backed source): the tick falls back to the
/// name read, which is already cheap (the backend is registered first).
struct CompiledWire {
    src_entity: Entity,
    src_port: String,
    src_resolved: Option<ResolvedPort>,
    /// The source's network-stable identity ([`lunco_core::GlobalEntityId`]), or
    /// `None` for a purely local entity. The **sort key** that makes summation
    /// order peer-independent — see [`CompiledWiring::rebuild`] (P10).
    src_gid: Option<u64>,
    /// Index into [`CompiledWiring::targets`] — the accumulator slot.
    dst_index: usize,
    scale: f64,
    offset: f64,
}

/// One compiled target: the input endpoint every wire into it accumulates onto,
/// with its resolved write handle (see [`CompiledWire::src_resolved`]).
struct CompiledTarget {
    entity: Entity,
    name: String,
    resolved: Option<ResolvedPort>,
}

/// The flattened wiring fabric — the "SignalBus" — cached inside
/// [`propagate_connections`] and rebuilt only when the [`crate::SimConnection`]
/// set actually changes.
///
/// Replaces the old per-tick snapshot (string-cloning every connector every
/// tick + a string-keyed `HashMap` accumulator). Targets are interned to dense
/// indices so propagation accumulates into a plain `Vec<f64>` with no hashing.
/// Each endpoint is resolved to a [`ResolvedPort`] handle at compile time so the
/// hot loop exchanges by slot, not by name-scan across backends.
#[derive(Default)]
pub struct CompiledWiring {
    wires: Vec<CompiledWire>,
    /// Distinct targets, one accumulator slot each.
    targets: Vec<CompiledTarget>,
}

impl CompiledWiring {
    /// Recompile the fabric from the live [`SimConnection`] set. Runs only when
    /// the wiring changed (driven by [`RebuildOnChange`]). Resolves every
    /// endpoint to its [`ResolvedPort`] handle here — the ONE scan — so the
    /// per-tick loop reads/writes by slot.
    fn rebuild(&mut self, world: &mut World) {
        self.wires.clear();
        self.targets.clear();
        let mut target_index: HashMap<(Entity, String), usize> = HashMap::new();

        // Registry is `Copy` fn-pointers; clone it out so resolution below borrows
        // `world` immutably alongside the collected connections.
        let registry = world.resource::<PortRegistry>().clone();
        let mut q = world.query::<&SimConnection>();
        let conns: Vec<SimConnection> = q.iter(world).cloned().collect();

        for c in &conns {
            if c.start_element == Entity::PLACEHOLDER || c.end_element == Entity::PLACEHOLDER {
                continue;
            }
            let key = (c.end_element, c.end_connector.clone());
            let dst_index = *target_index.entry(key).or_insert_with(|| {
                let i = self.targets.len();
                // Resolve the target's input handle once (fast-path backends only).
                let resolved = registry.resolve_input(world, c.end_element, &c.end_connector);
                self.targets.push(CompiledTarget {
                    entity: c.end_element,
                    name: c.end_connector.clone(),
                    resolved,
                });
                i
            });
            // Resolve the source's output handle once.
            let src_resolved = registry.resolve_output(world, c.start_element, &c.start_connector);
            self.wires.push(CompiledWire {
                src_entity: c.start_element,
                src_port: c.start_connector.clone(),
                src_resolved,
                src_gid: world
                    .get::<lunco_core::GlobalEntityId>(c.start_element)
                    .map(|g| g.get()),
                dst_index,
                scale: c.scale,
                offset: c.offset,
            });
        }

        // P10 — **the summation order must not be archetype order.**
        //
        // Wires fanning into one input SUM (`acc[dst_index] += …`), and f64
        // addition is not associative: reorder the terms and you get a different
        // last bit. The wires above were collected in ECS iteration order, which
        // depends on the order `SimConnection` entities were spawned — and host
        // and client reach the same wiring through DIFFERENT paths (local USD
        // load vs replicated spawn). Same wires, different order, different
        // rounding: a bit-level divergence at the root of the force path, on the
        // very bodies the client predicts.
        //
        // Sorting on a network-stable key removes the dependency entirely. One
        // sort per wiring change; ZERO per-tick cost.
        //
        // Key: (dst slot, source's GlobalEntityId, source port). `src_gid` is the
        // identity both peers agree on; local-only sources (`None`) sort first
        // among themselves and are, by construction, not replicated — so they
        // cannot be the thing that differs across peers. The `src_entity` tail is
        // a total-order tiebreak (two wires from the same source port into the
        // same input are numerically interchangeable anyway).
        self.wires.sort_by(|a, b| {
            a.dst_index
                .cmp(&b.dst_index)
                .then_with(|| a.src_gid.cmp(&b.src_gid))
                .then_with(|| a.src_port.cmp(&b.src_port))
                .then_with(|| a.src_entity.to_bits().cmp(&b.src_entity.to_bits()))
        });
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

    // Phase 3: accumulate. Read the source by its resolved handle (avian fast
    // path); fall back to the name read when no fast-path backend owns it, or when
    // a stale handle no longer backs a live value (component removed → re-resolve
    // by name this tick, contributing nothing if truly absent).
    for w in &compiled.wires {
        let src = match w.src_resolved {
            // Fast path; on a stale handle (source component removed/swapped since
            // the last rebuild) fall back to the name read so behaviour matches the
            // pre-resolve master exactly.
            Some(r) => registry
                .read_resolved(world, w.src_entity, r)
                .or_else(|| registry.read_output_port(world, w.src_entity, &w.src_port)),
            None => registry.read_output_port(world, w.src_entity, &w.src_port),
        };
        let Some(src) = src else {
            continue; // source output absent — contributes nothing this tick
        };
        acc[w.dst_index] += src * w.scale + w.offset;
    }

    // Phase 4: write each target once, by resolved handle where available.
    for (i, t) in compiled.targets.iter().enumerate() {
        let written = match t.resolved {
            // Fast path; on a stale handle fall back to the name write (short-
            // circuits when the slot write succeeds, so never double-writes).
            Some(r) => {
                registry.write_resolved(world, t.entity, r, acc[i])
                    || registry.write_port(world, t.entity, &t.name, acc[i])
            }
            None => registry.write_port(world, t.entity, &t.name, acc[i]),
        };
        if !written {
            warn_once!(
                "[cosim] connection targets unknown input port '{}' on {:?} — value dropped \
                 (declare the port or fix the wire)",
                t.name,
                t.entity
            );
        }
    }
}

#[cfg(test)]
mod wire_order_tests {
    use super::*;
    use lunco_core::GlobalEntityId;

    /// P10: the fabric is compiled from ECS iteration order, but the SUMMATION
    /// order must be a function of the wires' *identities*, not of the order the
    /// `SimConnection` entities happened to be spawned in. Host and client reach
    /// the same wiring by different spawn paths, so archetype order can differ —
    /// and f64 `+` is not associative, so a different order is a different last
    /// bit on the force feeding a predicted body.
    ///
    /// Build the same three wires into one input in two different spawn orders;
    /// the compiled wire sequence must come out identical.
    #[test]
    fn wire_summation_order_is_spawn_order_independent() {
        fn compile(spawn_order: &[u64]) -> Vec<(Option<u64>, String)> {
            let mut world = World::new();
            world.init_resource::<PortRegistry>();

            // Three distinct sources with stable network ids 10/20/30.
            let mut src = std::collections::HashMap::new();
            for gid in [10_u64, 20, 30] {
                src.insert(gid, world.spawn(GlobalEntityId::from_raw(gid)).id());
            }
            let sink = world.spawn_empty().id();

            for gid in spawn_order {
                world.spawn(SimConnection {
                    start_element: src[gid],
                    start_connector: format!("out_{gid}"),
                    end_element: sink,
                    end_connector: "force_y".into(),
                    scale: 1.0,
                    offset: 0.0,
                });
            }

            let mut compiled = CompiledWiring::default();
            compiled.rebuild(&mut world);
            compiled
                .wires
                .iter()
                .map(|w| (w.src_gid, w.src_port.clone()))
                .collect()
        }

        let a = compile(&[10, 20, 30]);
        let b = compile(&[30, 10, 20]);
        let c = compile(&[20, 30, 10]);
        assert_eq!(a.len(), 3);
        assert_eq!(a, b, "wire order must not depend on SimConnection spawn order");
        assert_eq!(a, c, "wire order must not depend on SimConnection spawn order");
        // …and it is the network-stable order, not an accident.
        assert_eq!(
            a.iter().map(|(g, _)| *g).collect::<Vec<_>>(),
            vec![Some(10), Some(20), Some(30)]
        );
    }
}
