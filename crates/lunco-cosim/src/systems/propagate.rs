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
//! port-bearing backend (Modelica, Avian, joint, hardware `Port`, …) joins the
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

/// Does **this peer simulate** `target`, i.e. may propagation write into it?
///
/// The criterion is per-ENTITY, because that is what the rule actually is: a
/// pure client must not run cosim on a body whose motion the host owns — it
/// renders that body's snapshots, and locally driving its ports fights the
/// snapshot stream (drift/jitter whenever the host is briefly static). It must
/// absolutely still run cosim on everything it simulates itself, or a predicted
/// rover's command never reaches its actuators and the rover stops driving on
/// clients while working fine on the host.
///
/// Host and standalone simulate every body authoritatively, so the question is
/// only asked on a client. There, a target is simulated locally when:
///
/// * it carries [`lunco_core::OwnedLocally`] — the body this session possesses
///   and predicts (input-replay reconciled), or a wheel of it
///   (`propagate_owned_to_wheels` mirrors the marker onto `ArticulatedLink`s), or
/// * it carries [`lunco_core::PredictedDynamic`] — a free body promoted to local
///   dynamic prediction (state reconciled), or
/// * it is **not replicated at all** ([`lunco_core::NetReplicate`] absent). This
///   clause is the load-bearing one for vessel control: the endpoints of a
///   rover's actuation graph are bare [`lunco_core::architecture::Port`] entities
///   (`lunco-usd-sim`'s `try_wire_wheel` targets `p_drive`/`p_steer`), which have
///   no `RigidBody` and therefore never enter replication membership
///   (`apply_net_replication` requires one). They are local scaffolding whose
///   values only ever reach avian through the actuator systems, and those carry
///   their own per-chassis kinematic guards.
///
/// Skipped, therefore, is exactly the intended set: a replicated body this peer
/// neither owns nor predicts — a pure snapshot proxy.
///
/// Note this is the same locally-simulated classifier the netcode's proxy seams
/// use (`interpolate_proxies` / `drive_kinematic_proxies` both take
/// `Or<(With<OwnedLocally>, With<PredictedDynamic>)>`), plus the
/// never-replicated case those seams get for free by only ever iterating
/// replicated gids.
fn peer_simulates(world: &World, target: Entity, is_client: bool) -> bool {
    if !is_client {
        return true; // host / standalone: authoritative over everything
    }
    world.get::<lunco_core::OwnedLocally>(target).is_some()
        || world.get::<lunco_core::PredictedDynamic>(target).is_some()
        || world.get::<lunco_core::NetReplicate>(target).is_none()
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
    /// Read the source's input side — see [`SimConnection::start_is_input`].
    src_is_input: bool,
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
            // An input-side source has no resolved OUTPUT handle; the name read is
            // the only correct path for it.
            let src_resolved = if c.start_is_input {
                None
            } else {
                registry.resolve_output(world, c.start_element, &c.start_connector)
            };
            self.wires.push(CompiledWire {
                src_entity: c.start_element,
                src_port: c.start_connector.clone(),
                src_is_input: c.start_is_input,
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
///
/// ## Per-target network gating
///
/// Propagation is gated **per target**, not by process role. See
/// [`peer_simulates`]: a pure client keeps propagating into everything it
/// actually simulates (its owned/predicted bodies, and every purely-local
/// entity such as the bare `Port` nodes of a rover's actuation graph) and skips
/// only targets that are replicated from the host and merely rendered. Host and
/// standalone propagate into everything.
pub fn propagate_connections(
    world: &mut World,
    mut wiring: Local<RebuildOnChange<SimConnection, CompiledWiring>>,
    mut acc: Local<Vec<f64>>,
    // Dangling-wire names already reported. Dedup is per PORT NAME, not per
    // call site: a `warn_once!` here reported only the FIRST dangling wire in
    // the whole process and silently dropped every other one, which is exactly
    // how a model that lost ALL of its inputs could look like it had lost one.
    mut reported: Local<std::collections::HashSet<String>>,
    // The target NAME SET the `reported` dedup was last cleared for. See the reset
    // below: `rewired` fires far more often than "a new scene".
    mut last_targets: Local<std::collections::HashSet<String>>,
) {
    // Registry is a `Vec` of `Copy` backend fn-pointers; clone it out so the
    // write phase can take `&mut World` without holding a resource borrow.
    let registry = world.resource::<PortRegistry>().clone();

    // Phase 1: recompile the fabric iff the connection set changed. The compiled
    // fabric is owned by the `Local` (no world borrow), so the phases below keep
    // `&mut World` for the resolver.
    let mut rewired = false;
    let compiled = wiring.get_or_rebuild(world, |compiled, world| {
        rewired = true;
        compiled.rebuild(world)
    });
    // A rewire (new scene, edited connection) makes every prior dangling-wire
    // report stale: the names below belong to THIS fabric. Without the reset the
    // dedup set — keyed by port name for the process lifetime — would silence a
    // genuine dangling `angle` in the scene you just loaded because some earlier
    // scene already had one.
    //
    // But clear on a CHANGED fabric, not on every rebuild. `rewire_usd_connections`
    // despawns and respawns EVERY `SimConnection` on any structural change — any
    // prim spawn or despawn anywhere, or any live USD edit — so `rewired` is true
    // on a large fraction of frames while a scene is streaming in. Resetting the
    // dedup set there turned a once-per-name report into hundreds of identical
    // lines per second. Key the reset on the target names actually changing.
    if rewired {
        let names: std::collections::HashSet<String> =
            compiled.targets.iter().map(|t| t.name.clone()).collect();
        if *last_targets != names {
            reported.clear();
            *last_targets = names;
        }
    }

    if compiled.targets.is_empty() {
        // No wires ⇒ nothing broken. Clear any report left from a prior fabric.
        let mut diag = world.resource_mut::<crate::diagnostics::CosimDiagnostics>();
        if !diag.broken.is_empty() {
            diag.broken.clear();
        }
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
        let read_src = |w: &CompiledWire| {
            if w.src_is_input {
                registry.read_input_port(world, w.src_entity, &w.src_port)
            } else {
                registry.read_output_port(world, w.src_entity, &w.src_port)
            }
        };
        let src = match w.src_resolved {
            // Fast path; on a stale handle (source component removed/swapped since
            // the last rebuild) fall back to the name read so behaviour matches the
            // pre-resolve master exactly.
            Some(r) => registry
                .read_resolved(world, w.src_entity, r)
                .or_else(|| read_src(w)),
            None => read_src(w),
        };
        let Some(src) = src else {
            continue; // source output absent — contributes nothing this tick
        };
        acc[w.dst_index] += src * w.scale + w.offset;
    }

    // Phase 4: write each target once, by resolved handle where available.
    // Gated per target (see `peer_simulates`), never by process role.
    let is_client = matches!(
        world.get_resource::<lunco_core::NetworkRole>().copied(),
        Some(lunco_core::NetworkRole::Client)
    );
    // Machine-readable mirror of the dangling-wire log lines below, rebuilt every
    // tick so `GET /api/diagnostics` polls the current fabric (see `CosimDiagnostics`).
    let mut broken: Vec<crate::diagnostics::BrokenConnection> = Vec::new();
    // Targets that DID take their write this tick — the proof a wire is real, and
    // the only thing that can retract a fault (see below).
    let mut landed: Vec<(Entity, String)> = Vec::new();
    for (i, t) in compiled.targets.iter().enumerate() {
        if !peer_simulates(world, t.entity, is_client) {
            continue;
        }
        let written = match t.resolved {
            // Fast path; on a stale handle fall back to the name write (short-
            // circuits when the slot write succeeds, so never double-writes).
            Some(r) => {
                registry.write_resolved(world, t.entity, r, acc[i])
                    || registry.write_port(world, t.entity, &t.name, acc[i])
            }
            None => registry.write_port(world, t.entity, &t.name, acc[i]),
        };
        // A target on an entity that exposes NO PORT SURFACE AT ALL is not a
        // dangling wire, and reporting it as one buried the real diagnostic:
        //
        // * `demand` on a `Motor_*` and `torque` on a `Gearbox_*` are STRUCTURAL
        //   endpoints. Those prims' data is folded into `WheelParams` at parse
        //   time and the runtime path is InputPorts → DriveMix → wheel port;
        //   nothing registers a backend for those names, by design. The USD wires
        //   document the mechanical chain, and they should stay.
        // * `throttle`, `drive_left`, `drive_right` DO belong to real backends —
        //   the OBC's command inputs, a `.mo` model's declared inputs — but those
        //   backends only claim the entity once the model asset finishes loading
        //   or the control binding lands. Warning during that window described a
        //   load order, not a fault.
        //
        // An entity that exposes ports but not THIS name is the genuine case — a
        // typo'd or stale wire — and still warns.
        //
        // DOWNGRADED, not silenced. `sun_tracker` fails today with exactly this
        // shape — a model that never publishes `sun_azimuth` — and dropping the
        // line entirely would have taken the only evidence with it.
        if written {
            landed.push((t.entity, t.name.clone()));
            continue;
        }
        let has_port_surface = !registry.entity_ports(world, t.entity).is_empty();
        {
            // Diagnostics carry EVERY unresolved target this tick (a poller wants
            // the live set), whereas the log is deduped per name to stay readable.
            broken.push(crate::diagnostics::BrokenConnection {
                entity: t.entity,
                global_id: world.get::<lunco_core::GlobalEntityId>(t.entity).copied(),
                port: t.name.clone(),
                has_port_surface,
                dropped_value: acc[i],
            });
            if reported.insert(t.name.clone()) {
                if has_port_surface {
                    warn!(
                        "[cosim] connection targets unknown input port '{}' on {:?} — value dropped \
                         (declare the port or fix the wire)",
                        t.name, t.entity
                    );
                } else {
                    debug!(
                        "[cosim] connection targets '{}' on {:?}, which exposes no ports yet — \
                         value dropped (structural endpoint, or a model still loading)",
                        t.name, t.entity
                    );
                }
            }
        }
    }

    // Publish the tick's report. Overwrites last tick's (the set is "what is broken
    // now"), so a wire that resolves once its model loads clears itself.
    //
    // `faults` is the opposite: it REMEMBERS which wires have never carried a
    // value. Propagation is change-driven, so a wire that dropped its write at
    // load is not re-attempted on a quiet tick and the live set reads empty a
    // second later — a gate sampling it at verdict time passes a run whose vehicle
    // was never actuated.
    //
    // A wire that DOES land retracts its fault and is never reported again: the
    // first write may well arrive late (a joint's `angle` port exists only once
    // avian admits both bodies), and that window is load order, not an authoring
    // error. Only a wire that never lands at all survives here.
    let mut diag = world.resource_mut::<crate::diagnostics::CosimDiagnostics>();
    for key in landed {
        diag.faults.remove(&key);
        diag.landed.insert(key);
    }
    for b in &broken {
        let key = (b.entity, b.port.clone());
        if b.has_port_surface && !diag.landed.contains(&key) {
            diag.faults.entry(key).or_insert_with(|| b.clone());
        }
    }
    diag.broken = broken;
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
                    start_is_input: false,
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
        assert_eq!(
            a, b,
            "wire order must not depend on SimConnection spawn order"
        );
        assert_eq!(
            a, c,
            "wire order must not depend on SimConnection spawn order"
        );
        // …and it is the network-stable order, not an accident.
        assert_eq!(
            a.iter().map(|(g, _)| *g).collect::<Vec<_>>(),
            vec![Some(10), Some(20), Some(30)]
        );
    }

    /// A wire whose target accepts no write must land in [`CosimDiagnostics`] —
    /// the machine-readable form behind `GET /api/diagnostics`. Empty registry ⇒
    /// the sink exposes no port surface, so it reports as a structural/loading
    /// endpoint (`has_port_surface == false`), not a genuine fault, but it is
    /// still reported (a poller wants the full unresolved set).
    #[test]
    fn broken_wire_populates_diagnostics() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.init_resource::<PortRegistry>();
        world.init_resource::<crate::diagnostics::CosimDiagnostics>();

        let src = world.spawn(GlobalEntityId::from_raw(10)).id();
        // Sink has an id but no port backend of any kind.
        let sink = world.spawn(GlobalEntityId::from_raw(20)).id();
        world.spawn(SimConnection {
            start_element: src,
            start_connector: "out".into(),
            end_element: sink,
            end_connector: "nonexistent_port".into(),
            scale: 1.0,
            offset: 0.0,
            start_is_input: false,
        });

        world.run_system_once(propagate_connections).unwrap();

        let diag = world.resource::<crate::diagnostics::CosimDiagnostics>();
        assert_eq!(diag.broken.len(), 1, "the one unresolved target is reported");
        let b = &diag.broken[0];
        assert_eq!(b.port, "nonexistent_port");
        assert_eq!(b.global_id, Some(GlobalEntityId::from_raw(20)));
        assert!(
            !b.has_port_surface,
            "empty registry ⇒ sink exposes no ports ⇒ not a genuine fault"
        );

        // And it self-clears: give the sink nothing new, but drop the wire, and the
        // report empties on the next tick (the set is "what is broken NOW").
        let conns: Vec<Entity> = {
            let mut q = world.query_filtered::<Entity, With<SimConnection>>();
            q.iter(&world).collect()
        };
        for e in conns {
            world.entity_mut(e).despawn();
        }
        world.run_system_once(propagate_connections).unwrap();
        assert!(
            world
                .resource::<crate::diagnostics::CosimDiagnostics>()
                .broken
                .is_empty(),
            "no wires ⇒ report clears"
        );
    }

    /// A structural endpoint is load order, not an authoring error, so it must
    /// never reach `faults` — the field a gate fails a run on.
    #[test]
    fn an_endpoint_with_no_port_surface_is_not_recorded_as_a_fault() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.init_resource::<PortRegistry>();
        world.init_resource::<crate::diagnostics::CosimDiagnostics>();

        let src = world.spawn(GlobalEntityId::from_raw(10)).id();
        let sink = world.spawn(GlobalEntityId::from_raw(20)).id();
        world.spawn(SimConnection {
            start_element: src,
            start_connector: "out".into(),
            end_element: sink,
            end_connector: "not_yet_loaded".into(),
            scale: 1.0,
            offset: 0.0,
            start_is_input: false,
        });

        world.run_system_once(propagate_connections).unwrap();

        let diag = world.resource::<crate::diagnostics::CosimDiagnostics>();
        assert_eq!(diag.broken.len(), 1, "still visible to a poller");
        assert!(
            diag.faults.is_empty(),
            "a target exposing no ports at all is still loading, not misauthored"
        );
    }

    /// A wire proven to have carried a value can never be re-reported as a fault.
    ///
    /// This is the rule that a permanent-fault-log version of this got wrong. A
    /// joint's `angle` port exists only once avian has admitted both its bodies
    /// into the island graph — a documented multi-frame window every jointed
    /// mechanism passes through — so recording that first dropped write forever
    /// failed `rocker_bogie` on the very antenna its own scenario measured
    /// working. Landing is monotone: once wired, always wired.
    #[test]
    fn a_wire_that_has_landed_is_never_re_reported_as_a_fault() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.init_resource::<PortRegistry>();
        world.init_resource::<crate::diagnostics::CosimDiagnostics>();

        let src = world.spawn(GlobalEntityId::from_raw(10)).id();
        let sink = world.spawn(GlobalEntityId::from_raw(20)).id();
        world.spawn(SimConnection {
            start_element: src,
            start_connector: "out".into(),
            end_element: sink,
            end_connector: "angle".into(),
            scale: 1.0,
            offset: 0.0,
            start_is_input: false,
        });

        // Stand in for "this wire wrote successfully on an earlier tick", which
        // is all `landed` records. Reaching a real port backend would need a
        // registered provider and would test that provider, not this rule.
        world
            .resource_mut::<crate::diagnostics::CosimDiagnostics>()
            .landed
            .insert((sink, "angle".to_string()));

        world.run_system_once(propagate_connections).unwrap();

        assert!(
            world
                .resource::<crate::diagnostics::CosimDiagnostics>()
                .faults
                .is_empty(),
            "a wire already proven to have landed must not be re-reported"
        );
    }
}
