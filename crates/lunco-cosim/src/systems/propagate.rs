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

/// A flattened connection, decoupled from the live `SimConnection` component so
/// the source query borrow is released before the mutable write pass.
type FlatConnection = (Entity, String, Entity, String, f64, f64);

/// Propagates values through all [`crate::SimConnection`]s.
///
/// Exclusive system: it addresses arbitrary backends through the resolver,
/// which needs whole-world access. Three phases, no per-type special-casing:
///
/// 1. **Collect** — snapshot every valid connection (drops the query borrow).
/// 2. **Accumulate** — read each source via [`read_port`], sum `src*scale+offset`
///    per target. Every driven target is seeded to `0.0`, so a target whose
///    source vanished cleanly returns to zero.
/// 3. **Write** — push each accumulated value to its input via [`write_port`].
///    A target with no such input port is a dangling wire — reported, not
///    silently dropped.
///
/// Undriven input ports are never touched, so a manual `SetPort` hold survives.
pub fn propagate_connections(
    world: &mut World,
    mut conns: Local<Vec<FlatConnection>>,
    mut acc: Local<HashMap<(Entity, String), f64>>,
) {
    // Phase 1: collect. `world.query` borrows the world immutably for the
    // duration of `iter`; we clone out so the borrow ends before phase 3's
    // mutable writes. (TODO(perf): cache as a flat index table — the SignalBus
    // step — once propagation shows up in a profile; string-keyed is fine at
    // the current connection counts.)
    conns.clear();
    let mut q = world.query::<&SimConnection>();
    for c in q.iter(world) {
        if c.start_element == Entity::PLACEHOLDER || c.end_element == Entity::PLACEHOLDER {
            continue;
        }
        conns.push((
            c.start_element,
            c.start_connector.clone(),
            c.end_element,
            c.end_connector.clone(),
            c.scale,
            c.offset,
        ));
    }
    if conns.is_empty() {
        return;
    }

    // Resolve every endpoint through the shared port registry. Cloned out of the
    // world (a `Vec` of `Copy` backend fn-pointers) so phase 3 can take `&mut
    // World` without holding a resource borrow.
    let registry = world.resource::<PortRegistry>().clone();

    // Phase 2: accumulate. Seed every driven target to 0 so summing is clean
    // and a target with a missing source resets rather than holding stale data.
    acc.clear();
    for (_, _, end_element, end_connector, _, _) in conns.iter() {
        acc.entry((*end_element, end_connector.clone())).or_insert(0.0);
    }
    for (start_element, start_connector, end_element, end_connector, scale, offset) in conns.iter() {
        let Some(src) = registry.read_output_port(world, *start_element, start_connector) else {
            continue; // source output absent — contributes nothing this tick
        };
        if let Some(slot) = acc.get_mut(&(*end_element, end_connector.clone())) {
            *slot += src * *scale + *offset;
        }
    }

    // Phase 3: write each target once through the resolver.
    for ((entity, name), value) in acc.iter() {
        if !registry.write_port(world, *entity, name, *value) {
            warn_once!(
                "[cosim] connection targets unknown input port '{}' on {:?} — value dropped \
                 (declare the port or fix the wire)",
                name,
                entity
            );
        }
    }
}
