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
//!
//! `SimConnection` is the explicit causal co-simulation boundary. The exchange
//! is a single Jacobi/ZOH read-then-write transaction, so feedback between
//! participants is a valid dynamic feedback loop: state advances between
//! transactions and no algebraic convergence is claimed. A true acausal
//! connection belongs to a typed backend island and must be partitioned before
//! stepping; it must not be guessed from an SCC in this causal fabric.

use std::collections::HashMap;

use bevy::prelude::*;

use lunco_core::ports::{PortRegistry, ResolvedPort};
use lunco_core::RebuildOnChange;

use crate::{is_physics_force_port, BoundConnection, RealtimeSafe, SimConnection};

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
pub(crate) fn peer_simulates(world: &World, target: Entity) -> bool {
    let is_client = matches!(
        world.get_resource::<lunco_core::NetworkRole>().copied(),
        Some(lunco_core::NetworkRole::Client)
    );
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

/// Identify a warning by the endpoint that owns the failed port, not by the
/// port spelling alone. Global ids remain stable across network peers; the
/// entity bits are the only available identity for purely local scaffolding.
fn target_report_key(world: &World, target: &CompiledTarget) -> String {
    let identity = world
        .get::<lunco_core::GlobalEntityId>(target.entity)
        .map(|id| format!("gid:{}", id.get()))
        .unwrap_or_else(|| format!("entity:{}", target.entity.to_bits()));
    format!("target:{identity}:{}", target.name)
}

/// One algebraic loop found at compile time, spanning at least two participants.
#[derive(Clone)]
struct DetectedLoop {
    entity: Entity,
    global_id: Option<lunco_core::GlobalEntityId>,
    detail: String,
    force_producing: bool,
    requires_realtime_safe: bool,
    realtime_safe: bool,
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
    /// Algebraic loops in this fabric, recomputed on every rebuild.
    loops: Vec<DetectedLoop>,
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
        let mut q = world.query_filtered::<&SimConnection, With<BoundConnection>>();
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
        self.detect_algebraic_loops(world);
    }

    /// Find force-producing feedback SCCs over the explicit causal wire graph.
    ///
    /// The master is single-pass Jacobi with ZOH inputs: each feedthrough hop on
    /// a cycle costs one fixed step of delay, and nothing iterates the loop to
    /// convergence. That 1-step-delay behaviour is the documented contract and
    /// is NOT changed here — a detected loop is published as a topology
    /// diagnostic so the otherwise invisible coupling error is diagnosable,
    /// without pretending that any endpoint is missing a port.
    ///
    /// Single-entity self-wires (`netForce`→`force_y` on ONE entity — the
    /// balloon pattern, an engine exchanging with its own body) are the intended
    /// single-participant co-sim shape and are excluded: a reported cycle must
    /// span ≥ 2 participants (an SCC of ≥ 2 nodes, via iterative Tarjan).
    fn detect_algebraic_loops(&mut self, world: &World) {
        self.loops.clear();

        // Participant graph. Self-edges dropped (see doc above).
        let mut node_ix: HashMap<Entity, usize> = HashMap::new();
        let mut nodes: Vec<Entity> = Vec::new();
        let mut edges: Vec<Vec<usize>> = Vec::new();
        for w in &self.wires {
            let dst = self.targets[w.dst_index].entity;
            if w.src_entity == dst {
                continue;
            }
            for e in [w.src_entity, dst] {
                node_ix.entry(e).or_insert_with(|| {
                    nodes.push(e);
                    edges.push(Vec::new());
                    nodes.len() - 1
                });
            }
            let (s, d) = (node_ix[&w.src_entity], node_ix[&dst]);
            if !edges[s].contains(&d) {
                edges[s].push(d);
            }
        }

        // Iterative Tarjan SCC (explicit frame stack — no recursion depth limit).
        let n = nodes.len();
        let mut index = vec![usize::MAX; n];
        let mut low = vec![0usize; n];
        let mut on_stack = vec![false; n];
        let mut stack: Vec<usize> = Vec::new();
        let mut next_index = 0usize;
        let mut sccs: Vec<Vec<usize>> = Vec::new();
        for root in 0..n {
            if index[root] != usize::MAX {
                continue;
            }
            let mut call: Vec<(usize, usize)> = vec![(root, 0)];
            while let Some(frame) = call.last_mut() {
                let v = frame.0;
                if frame.1 == 0 {
                    index[v] = next_index;
                    low[v] = next_index;
                    next_index += 1;
                    stack.push(v);
                    on_stack[v] = true;
                }
                if frame.1 < edges[v].len() {
                    let w = edges[v][frame.1];
                    frame.1 += 1;
                    if index[w] == usize::MAX {
                        call.push((w, 0));
                    } else if on_stack[w] {
                        low[v] = low[v].min(index[w]);
                    }
                } else {
                    call.pop();
                    if let Some(parent) = call.last_mut() {
                        low[parent.0] = low[parent.0].min(low[v]);
                    }
                    if low[v] == index[v] {
                        let mut comp = Vec::new();
                        loop {
                            let w = stack.pop().expect("Tarjan stack underflow");
                            on_stack[w] = false;
                            comp.push(w);
                            if w == v {
                                break;
                            }
                        }
                        if comp.len() >= 2 {
                            sccs.push(comp);
                        }
                    }
                }
            }
        }

        for comp in sccs {
            let members: std::collections::HashSet<Entity> =
                comp.iter().map(|&i| nodes[i]).collect();
            // Every wire whose both endpoints sit inside the SCC IS the coupling;
            // wires are already in P10 order, so the description is deterministic.
            let mut parts: Vec<String> = Vec::new();
            let mut force_producing = false;
            let mut requires_realtime_safe = false;
            for w in &self.wires {
                let dst = &self.targets[w.dst_index];
                // A participant can feed a body-force port on itself (the
                // common Modelica-to-Avian projection), so the force edge is
                // intentionally not part of the multi-entity SCC above.
                // It still makes the SCC's control loop force-producing. The
                // realtime contract applies only when that force reaches a
                // client-predicted dynamic body; authoritative host/standalone
                // co-simulation is already fixed-step and must not be rejected
                // as though it were a prediction loop.
                if members.contains(&w.src_entity)
                    && (w.src_entity == dst.entity || members.contains(&dst.entity))
                    && is_physics_force_port(&dst.name)
                {
                    force_producing = true;
                    let client_predicts = matches!(
                        world.get_resource::<lunco_core::NetworkRole>().copied(),
                        Some(lunco_core::NetworkRole::Client)
                    );
                    let predicted_dynamic = world
                        .get::<avian3d::prelude::RigidBody>(dst.entity)
                        .is_some_and(|body| matches!(body, avian3d::prelude::RigidBody::Dynamic))
                        && world
                            .get::<lunco_core::NotPredictable>(dst.entity)
                            .is_none();
                    requires_realtime_safe |= client_predicts && predicted_dynamic;
                }
                if w.src_entity != dst.entity
                    && members.contains(&w.src_entity)
                    && members.contains(&dst.entity)
                {
                    parts.push(format!(
                        "{:?}:{} -> {:?}:{}",
                        w.src_entity, w.src_port, dst.entity, dst.name
                    ));
                }
            }
            // Canonical member: same network-stable key the P10 sort uses, so the
            // ledger key does not depend on archetype order.
            let entity = members
                .iter()
                .copied()
                .min_by_key(|e| {
                    (
                        world.get::<lunco_core::GlobalEntityId>(*e).map(|g| g.get()),
                        e.to_bits(),
                    )
                })
                .expect("SCC has >= 2 members");
            // `RealtimeSafe` is a promise made by a simulation program, not by
            // the physical body that receives its force. A normal feedback
            // loop therefore contains both a `SimComponent` and an Avian body;
            // requiring the marker on every SCC member rejects the documented
            // body-state -> program -> body-force shape. Every program in the
            // loop must make the promise, and a force loop with no program is
            // not safe by inference.
            let programs: Vec<Entity> = members
                .iter()
                .copied()
                .filter(|member| world.get::<crate::SimComponent>(*member).is_some())
                .collect();
            let realtime_safe = force_producing
                && !programs.is_empty()
                && programs
                    .iter()
                    .all(|program| world.get::<RealtimeSafe>(*program).is_some());
            self.loops.push(DetectedLoop {
                entity,
                global_id: world.get::<lunco_core::GlobalEntityId>(entity).copied(),
                detail: parts.join(", "),
                force_producing,
                requires_realtime_safe,
                realtime_safe,
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
/// Undriven input ports are never touched, so a manual `SetPorts` hold survives.
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
    mut wiring: Local<RebuildOnChange<BoundConnection, CompiledWiring>>,
    mut acc: Local<Vec<f64>>,
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
    if rewired {
        // Both ledgers are keyed by `(Entity, port)` and both RATCHET — `faults`
        // so a gate can ask "did anything never land", `landed` so a wire that
        // once worked is never re-reported. Topology loops are kept in their
        // own current-fabric collection. The diagnostics resource is reset at
        // `SceneTeardown`, while the liveness pass below retires endpoints that
        // are removed by an ordinary edit without a scene replacement.
        //
        // Despawn is the exact retirement condition, and a rewire is when it is
        // cheap to notice: prim despawn is one of the two things that rebuild the
        // fabric. Keys are collected before taking the resource borrow because the
        // liveness test needs `&World`.
        let dead: Vec<(Entity, String)> = {
            let diag = world.resource::<crate::diagnostics::CosimDiagnostics>();
            diag.faults
                .keys()
                .chain(diag.landed.iter())
                .filter(|(entity, _)| !world.entities().contains(*entity))
                .cloned()
                .collect()
        };
        if !dead.is_empty() {
            let mut diag = world.resource_mut::<crate::diagnostics::CosimDiagnostics>();
            for key in dead {
                diag.faults.remove(&key);
                diag.landed.remove(&key);
            }
        }

        // Algebraic loops describe the current fabric. Keep them in their own
        // diagnostic collection: they are topology facts, not missing-port
        // faults, and therefore must not make a scenario fail the
        // never-landed connection gate.
        let loops = compiled
            .loops
            .iter()
            .map(|loop_info| crate::diagnostics::AlgebraicLoopDiagnostic {
                entity: loop_info.entity,
                global_id: loop_info.global_id,
                detail: loop_info.detail.clone(),
                force_producing: loop_info.force_producing,
                rejected: loop_info.force_producing
                    && loop_info.requires_realtime_safe
                    && !loop_info.realtime_safe,
            })
            .collect();
        world
            .resource_mut::<crate::diagnostics::CosimDiagnostics>()
            .algebraic_loops = loops;
        for loop_info in &compiled.loops {
            if loop_info.force_producing
                && loop_info.requires_realtime_safe
                && loop_info.realtime_safe
            {
                if world
                    .resource_mut::<crate::diagnostics::CosimDiagnostics>()
                    .report_once(format!("loop:{detail}", detail = loop_info.detail))
                {
                    info!(
                        "[cosim] force loop accepted under the declared realtime lockstep contract: {}",
                        loop_info.detail
                    );
                }
                continue;
            }
            if world
                .resource_mut::<crate::diagnostics::CosimDiagnostics>()
                .report_once(format!("loop:{detail}", detail = loop_info.detail))
            {
                if loop_info.force_producing
                    && loop_info.requires_realtime_safe
                    && !loop_info.realtime_safe
                {
                    error!(
                        "[cosim] unsafe force-producing algebraic loop rejected: {}",
                        loop_info.detail
                    );
                } else {
                    warn!(
                        "[cosim] algebraic loop in the wiring — co-simulated with a 1-step delay: {}",
                        loop_info.detail
                    );
                }
            }
            if loop_info.force_producing
                && loop_info.requires_realtime_safe
                && !loop_info.realtime_safe
            {
                let raised = world
                    .get_resource_mut::<lunco_core::RuntimeFaults>()
                    .is_some_and(|mut faults| {
                        faults.raise(
                            "cosim-unsafe-force-loop",
                            Some(loop_info.entity),
                            "co-simulation wiring",
                            loop_info.detail.clone(),
                        )
                    });
                if raised {
                    if let Some(mut holds) = world.get_resource_mut::<lunco_physics::PhysicsHolds>()
                    {
                        holds.set(lunco_physics::PhysicsHolds::SAFETY_FAILURE, true);
                    }
                }
            }
        }
    }

    if compiled.targets.is_empty() {
        // No wires ⇒ nothing broken. Clear any report left from a prior fabric.
        let mut diag = world.resource_mut::<crate::diagnostics::CosimDiagnostics>();
        if !diag.broken.is_empty() || !diag.pending.is_empty() {
            diag.broken.clear();
            diag.pending.clear();
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
    // Terminal failures, rebuilt every tick so `GET /api/diagnostics` polls the
    // current fabric (see `CosimDiagnostics`).
    let mut broken: Vec<crate::diagnostics::BrokenConnection> = Vec::new();
    // A scene may wire an endpoint before its runtime contract is published.
    // Generated Modelica islands are the important case: their interface is
    // provisional while compiling, not an authoring error.
    let mut pending: Vec<crate::diagnostics::BrokenConnection> = Vec::new();
    // Targets that DID take their write this tick — the proof a wire is real, and
    // the only thing that can retract a fault (see below).
    let mut landed: Vec<(Entity, String)> = Vec::new();
    // Manual holds outrank the fabric — see `crate::PortHolds`. Expired first (on
    // the REAL clock, so a paused or warped sim cannot extend a hold), then
    // snapshotted, because the write loop below owns `&mut World`.
    let now_real = world
        .get_resource::<Time<bevy::time::Real>>()
        .map(|time| time.elapsed_secs_f64())
        .unwrap_or(0.0);
    let held: std::collections::HashMap<(Entity, String), f64> =
        match world.get_resource_mut::<crate::PortHolds>() {
            Some(mut holds) if !holds.is_empty() => {
                holds.expire(now_real);
                holds.snapshot()
            }
            _ => Default::default(),
        };
    for (i, t) in compiled.targets.iter().enumerate() {
        if !peer_simulates(world, t.entity) {
            continue;
        }
        // A HELD port is not driven by its wire. Without this, a `SetPorts` write on a
        // wired input is overwritten by the next propagation tick — the write
        // "succeeds" and nothing happens, which is indistinguishable from a
        // broken port to whoever sent it.
        let value = held
            .get(&(t.entity, t.name.clone()))
            .copied()
            .unwrap_or(acc[i]);
        let written = match t.resolved {
            // Fast path; on a stale handle fall back to the name write (short-
            // circuits when the slot write succeeds, so never double-writes).
            Some(r) => {
                registry.write_resolved(world, t.entity, r, value)
                    || registry.write_port(world, t.entity, &t.name, value)
            }
            None => registry.write_port(world, t.entity, &t.name, value),
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
        // An entity that exposes ports but not THIS name is a genuine case only
        // once its interface is terminal. A compiling model may expose a partial
        // surface while its generated contract is still landing.
        if written {
            landed.push((t.entity, t.name.clone()));
            continue;
        }
        let has_port_surface = !registry.entity_ports(world, t.entity).is_empty();
        let unresolved = crate::diagnostics::BrokenConnection {
            entity: t.entity,
            global_id: world.get::<lunco_core::GlobalEntityId>(t.entity).copied(),
            port: t.name.clone(),
            has_port_surface,
            dropped_value: acc[i],
        };
        let model_status = world
            .get::<crate::SimComponent>(t.entity)
            .map(|component| &component.status);
        let compiling = matches!(model_status, Some(crate::SimStatus::Compiling));
        // Only an explicitly published pending marker or a compiling Modelica
        // component is assembly progress. Structural Motor/Gearbox edges are
        // filtered during USD wire derivation, so a bare endpoint with no
        // surface is a terminal authoring fault rather than a forever-pending
        // wire.
        let surface_pending = world
            .get::<lunco_core::PortSurfacePending>(t.entity)
            .is_some();
        if compiling || surface_pending {
            pending.push(unresolved);
            continue;
        }

        broken.push(unresolved.clone());
        // Insertions are the failure event. They occur once per endpoint, not on
        // every propagation tick, and are what produces the warning.
        let key = (unresolved.entity, unresolved.port.clone());
        let already_landed = world
            .resource::<crate::diagnostics::CosimDiagnostics>()
            .landed
            .contains(&key);
        if !already_landed {
            let inserted = {
                let mut diag = world.resource_mut::<crate::diagnostics::CosimDiagnostics>();
                match diag.faults.entry(key) {
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        entry.insert(unresolved.clone());
                        true
                    }
                    std::collections::hash_map::Entry::Occupied(_) => false,
                }
            };
            let report_key = target_report_key(world, t);
            let should_report = inserted
                && world
                    .resource_mut::<crate::diagnostics::CosimDiagnostics>()
                    .report_once(report_key);
            if should_report {
                let label = world
                    .get::<Name>(t.entity)
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| format!("{:?}", t.entity));
                warn!(
                    "[cosim] connection targets unknown input port '{}' on {} ({:?}) — value dropped \
                     (declare the port or fix the wire)",
                    t.name, label, t.entity
                );
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
    diag.pending = pending;
    diag.broken = broken;
}

#[cfg(test)]
mod wire_order_tests {
    use super::*;
    use crate::SimComponent;
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
                world.spawn((
                    SimConnection {
                        start_element: src[gid],
                        start_connector: format!("out_{gid}"),
                        start_is_input: false,
                        end_element: sink,
                        end_connector: "force_y".into(),
                        scale: 1.0,
                        offset: 0.0,
                    },
                    BoundConnection,
                ));
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

    /// A HOLD outranks the wire into the same port, and hands it back when it
    /// expires.
    ///
    /// This is the failure the hold exists for: writing a wired input directly
    /// "succeeds" and is overwritten by the very next propagation tick, so a
    /// setpoint on a driven port lasts under 16 ms and looks, from the caller's
    /// side, exactly like a port that does not exist.
    #[test]
    fn a_hold_outranks_its_wire_until_it_expires() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        world.init_resource::<crate::diagnostics::CosimDiagnostics>();
        world.init_resource::<crate::PortHolds>();
        world.init_resource::<Time<bevy::time::Real>>();

        // One backend over `SimComponent`, so the sink has a real writable port.
        let mut registry = PortRegistry::default();
        crate::ports::register_builtin_port_backends(&mut registry);
        world.insert_resource(registry);

        let src = world
            .spawn(crate::SimComponent {
                outputs: std::collections::HashMap::from([("out".to_string(), 7.0)]),
                ..Default::default()
            })
            .id();
        let sink = world
            .spawn(crate::SimComponent {
                inputs: std::collections::HashMap::from([("demand".to_string(), 0.0)]),
                ..Default::default()
            })
            .id();
        world.spawn((
            SimConnection {
                start_element: src,
                start_connector: "out".into(),
                start_is_input: false,
                end_element: sink,
                end_connector: "demand".into(),
                scale: 1.0,
                offset: 0.0,
            },
            BoundConnection,
        ));

        let demand = |world: &World| -> f64 {
            world
                .get::<crate::SimComponent>(sink)
                .and_then(|c| c.inputs.get("demand").copied())
                .unwrap_or(f64::NAN)
        };

        world.run_system_once(propagate_connections).unwrap();
        assert_eq!(demand(&world), 7.0, "the wire drives the port");

        // Hold it somewhere else. The wire keeps producing 7.0 every tick.
        world
            .resource_mut::<crate::PortHolds>()
            .hold(sink, "demand", -1.0, 100.0);
        world.run_system_once(propagate_connections).unwrap();
        assert_eq!(
            demand(&world),
            -1.0,
            "a live hold outranks the wire — this is what makes a setpoint on a driven port stick"
        );

        // Past its deadline the port goes back to its wiring, with no release
        // call: an abandoned hold must not leave a vehicle stuck.
        world
            .resource_mut::<Time<bevy::time::Real>>()
            .advance_by(std::time::Duration::from_secs(200));
        world.run_system_once(propagate_connections).unwrap();
        assert_eq!(demand(&world), 7.0, "an expired hold releases the port");
        assert!(
            world.resource::<crate::PortHolds>().is_empty(),
            "the expired entry is dropped, not merely ignored"
        );
    }

    /// A target without a runtime port surface and without an explicit pending
    /// marker is a terminal dangling-wire fault.
    #[test]
    fn broken_wire_populates_diagnostics() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.init_resource::<PortRegistry>();
        world.init_resource::<crate::diagnostics::CosimDiagnostics>();

        let src = world.spawn(GlobalEntityId::from_raw(10)).id();
        // Sink has an id but no port backend of any kind.
        let sink = world.spawn(GlobalEntityId::from_raw(20)).id();
        world.spawn((
            SimConnection {
                start_element: src,
                start_connector: "out".into(),
                start_is_input: false,
                end_element: sink,
                end_connector: "nonexistent_port".into(),
                scale: 1.0,
                offset: 0.0,
            },
            BoundConnection,
        ));

        world.run_system_once(propagate_connections).unwrap();

        let diag = world.resource::<crate::diagnostics::CosimDiagnostics>();
        assert_eq!(diag.broken.len(), 1, "the unresolved target is terminal");
        let b = &diag.broken[0];
        assert_eq!(b.port, "nonexistent_port");
        assert_eq!(b.global_id, Some(GlobalEntityId::from_raw(20)));
        assert!(
            !b.has_port_surface,
            "empty registry ⇒ sink exposes no ports, so the wire is genuinely dangling"
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
        assert!(
            world
                .resource::<crate::diagnostics::CosimDiagnostics>()
                .pending
                .is_empty(),
            "no wires also clears assembly progress"
        );
    }

    /// An endpoint that explicitly publishes `PortSurfacePending` is assembly
    /// progress, not an authoring error, so it must never reach `faults`.
    #[test]
    fn an_endpoint_with_no_port_surface_is_not_recorded_as_a_fault() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.init_resource::<PortRegistry>();
        world.init_resource::<crate::diagnostics::CosimDiagnostics>();

        let src = world.spawn(GlobalEntityId::from_raw(10)).id();
        let sink = world
            .spawn((GlobalEntityId::from_raw(20), lunco_core::PortSurfacePending))
            .id();
        world.spawn((
            SimConnection {
                start_element: src,
                start_connector: "out".into(),
                start_is_input: false,
                end_element: sink,
                end_connector: "not_yet_loaded".into(),
                scale: 1.0,
                offset: 0.0,
            },
            BoundConnection,
        ));

        world.run_system_once(propagate_connections).unwrap();

        let diag = world.resource::<crate::diagnostics::CosimDiagnostics>();
        assert_eq!(diag.pending.len(), 1, "still visible as assembly progress");
        assert!(
            diag.faults.is_empty(),
            "an explicit pending marker keeps load progress out of the fault gate"
        );
    }

    /// A generated model's partial interface is expected while it compiles. Once
    /// the same contract reaches `Running`, a missing declared input is terminal
    /// and enters the fault ledger exactly once.
    #[test]
    fn compiling_model_waits_before_unknown_port_becomes_a_fault() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.init_resource::<crate::diagnostics::CosimDiagnostics>();
        let mut registry = PortRegistry::default();
        crate::ports::register_builtin_port_backends(&mut registry);
        world.insert_resource(registry);

        let src = world.spawn(GlobalEntityId::from_raw(10)).id();
        let sink = world
            .spawn((
                GlobalEntityId::from_raw(20),
                crate::SimComponent {
                    model_name: "GeneratedElectricalIsland".into(),
                    // The generated model publishes part of its interface while
                    // compiling.  That makes the missing `drive_left` name a
                    // terminal contract fault once the model reaches Running,
                    // rather than an entity that has not exposed any port
                    // surface yet.
                    inputs: std::collections::HashMap::from([("existing".into(), 0.0)]),
                    status: crate::SimStatus::Compiling,
                    ..Default::default()
                },
            ))
            .id();
        world.spawn((
            SimConnection {
                start_element: src,
                start_connector: "out".into(),
                start_is_input: false,
                end_element: sink,
                end_connector: "drive_left".into(),
                scale: 1.0,
                offset: 0.0,
            },
            BoundConnection,
        ));

        world.run_system_once(propagate_connections).unwrap();
        let diag = world.resource::<crate::diagnostics::CosimDiagnostics>();
        assert_eq!(diag.pending.len(), 1);
        assert!(diag.broken.is_empty());
        assert!(
            diag.faults.is_empty(),
            "unexpected faults: {:?}",
            diag.faults
        );

        world.get_mut::<crate::SimComponent>(sink).unwrap().status = crate::SimStatus::Running;
        world.run_system_once(propagate_connections).unwrap();
        let diag = world.resource::<crate::diagnostics::CosimDiagnostics>();
        assert!(diag.pending.is_empty());
        assert_eq!(diag.broken.len(), 1);
        assert_eq!(diag.faults.len(), 1);
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
        world.spawn((
            SimConnection {
                start_element: src,
                start_connector: "out".into(),
                start_is_input: false,
                end_element: sink,
                end_connector: "angle".into(),
                scale: 1.0,
                offset: 0.0,
            },
            BoundConnection,
        ));

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

    fn wire(world: &mut World, src: Entity, out: &str, dst: Entity, inp: &str) {
        world.spawn((
            SimConnection {
                start_element: src,
                start_connector: out.into(),
                start_is_input: false,
                end_element: dst,
                end_connector: inp.into(),
                scale: 1.0,
                offset: 0.0,
            },
            BoundConnection,
        ));
    }

    fn init_builtin_ports(world: &mut World) {
        let mut registry = PortRegistry::default();
        crate::ports::register_builtin_port_backends(&mut registry);
        world.insert_resource(registry);
    }

    fn loop_diagnostics(world: &World) -> Vec<String> {
        world
            .resource::<crate::diagnostics::CosimDiagnostics>()
            .algebraic_loops
            .iter()
            .map(|loop_diag| loop_diag.detail.clone())
            .collect()
    }

    /// M6: a feedthrough cycle spanning two participants is an algebraic loop —
    /// the single-pass ZOH master cannot iterate it to convergence — and must
    /// publish exactly ONE topology diagnostic, naming the wires on it, while
    /// leaving the missing-port ledger empty.
    #[test]
    fn a_two_entity_feedthrough_loop_is_recorded_as_one_topology_diagnostic() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        init_builtin_ports(&mut world);
        world.init_resource::<crate::diagnostics::CosimDiagnostics>();

        let a = world
            .spawn((GlobalEntityId::from_raw(10), lunco_core::PortSurfacePending))
            .id();
        let b = world
            .spawn((GlobalEntityId::from_raw(20), lunco_core::PortSurfacePending))
            .id();
        wire(&mut world, a, "out", b, "in");
        wire(&mut world, b, "out", a, "in");

        world.run_system_once(propagate_connections).unwrap();

        let loops = loop_diagnostics(&world);
        assert_eq!(loops.len(), 1, "one loop, one diagnostic: {loops:?}");
        assert!(
            world
                .resource::<crate::diagnostics::CosimDiagnostics>()
                .faults
                .is_empty(),
            "a topology loop is not a missing-port fault"
        );
        assert!(
            loops[0].contains("out") && loops[0].contains("in"),
            "the entry names the ports on the loop: {}",
            loops[0]
        );

        // Removing the back-edge dissolves the loop; the rebuild retracts it.
        let conns: Vec<Entity> = {
            let mut q = world.query_filtered::<Entity, With<SimConnection>>();
            q.iter(&world).collect()
        };
        world.entity_mut(conns[1]).despawn();
        world.run_system_once(propagate_connections).unwrap();
        assert!(
            loop_diagnostics(&world).is_empty(),
            "a loop entry is a fact about the current fabric — retracted on rewire"
        );
    }

    /// A control loop may reach a body force through a self-wired participant
    /// edge rather than through an inter-entity edge in the SCC. The force
    /// classification must still see it and accept it when every PROGRAM in
    /// the loop declares the realtime contract; the receiving body does not
    /// make that promise.
    #[test]
    fn realtime_safe_loop_with_self_wired_force_is_accepted() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        init_builtin_ports(&mut world);
        world.init_resource::<crate::diagnostics::CosimDiagnostics>();

        let a = world
            .spawn((
                GlobalEntityId::from_raw(10),
                SimComponent {
                    inputs: std::collections::HashMap::from([("in".into(), 0.0)]),
                    ..default()
                },
                RealtimeSafe,
                avian3d::prelude::RigidBody::Dynamic,
            ))
            .id();
        let b = world
            .spawn((
                GlobalEntityId::from_raw(20),
                SimComponent {
                    inputs: std::collections::HashMap::from([("in".into(), 0.0)]),
                    ..default()
                },
                RealtimeSafe,
            ))
            .id();
        wire(&mut world, a, "out", b, "in");
        wire(&mut world, b, "out", a, "in");
        wire(&mut world, a, "force", a, "force_y");

        world.run_system_once(propagate_connections).unwrap();

        assert!(
            !loop_diagnostics(&world).is_empty(),
            "the current topology remains observable when its force loop is accepted"
        );
        let diag = world.resource::<crate::diagnostics::CosimDiagnostics>();
        assert!(!diag.algebraic_loops[0].rejected);
        assert!(diag.faults.is_empty());
    }

    /// The physical body is not a program and must not need a realtime promise
    /// of its own. A program without that promise is still unsafe.
    #[test]
    fn force_loop_requires_realtime_safe_on_program_not_body() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        init_builtin_ports(&mut world);
        world.init_resource::<crate::diagnostics::CosimDiagnostics>();
        world.init_resource::<lunco_core::RuntimeFaults>();
        world.insert_resource(lunco_core::NetworkRole::Client);

        let body = world
            .spawn((
                GlobalEntityId::from_raw(10),
                avian3d::prelude::RigidBody::Dynamic,
            ))
            .id();
        let program = world
            .spawn((
                GlobalEntityId::from_raw(20),
                SimComponent {
                    inputs: std::collections::HashMap::from([("height".into(), 0.0)]),
                    ..default()
                },
                RealtimeSafe,
            ))
            .id();
        wire(&mut world, body, "height", program, "height");
        wire(&mut world, program, "netForce", body, "force_y");

        world.run_system_once(propagate_connections).unwrap();
        assert!(
            !loop_diagnostics(&world).is_empty(),
            "the current topology remains observable when its force loop is accepted"
        );
        assert!(
            !world
                .resource::<crate::diagnostics::CosimDiagnostics>()
                .algebraic_loops[0]
                .rejected
        );
        assert!(
            world
                .resource::<crate::diagnostics::CosimDiagnostics>()
                .faults
                .is_empty(),
            "unexpected faults: {:?}",
            world
                .resource::<crate::diagnostics::CosimDiagnostics>()
                .faults
        );
        assert!(world
            .resource::<lunco_core::RuntimeFaults>()
            .first
            .is_none());

        let mut unsafe_world = World::new();
        init_builtin_ports(&mut unsafe_world);
        unsafe_world.init_resource::<crate::diagnostics::CosimDiagnostics>();
        unsafe_world.init_resource::<lunco_core::RuntimeFaults>();
        unsafe_world.insert_resource(lunco_core::NetworkRole::Client);
        let body = unsafe_world
            .spawn((
                GlobalEntityId::from_raw(10),
                avian3d::prelude::RigidBody::Dynamic,
            ))
            .id();
        let program = unsafe_world
            .spawn((
                GlobalEntityId::from_raw(20),
                SimComponent {
                    inputs: std::collections::HashMap::from([("height".into(), 0.0)]),
                    ..default()
                },
            ))
            .id();
        wire(&mut unsafe_world, body, "height", program, "height");
        wire(&mut unsafe_world, program, "netForce", body, "force_y");

        unsafe_world.run_system_once(propagate_connections).unwrap();
        assert_eq!(
            loop_diagnostics(&unsafe_world).len(),
            1,
            "an unpromised program must still reject the force loop"
        );
        assert_eq!(
            unsafe_world
                .resource::<lunco_core::RuntimeFaults>()
                .first
                .as_ref()
                .map(|fault| fault.kind),
            Some("cosim-unsafe-force-loop")
        );
    }

    /// Authoritative standalone/host feedback is already a fixed-step causal
    /// exchange. It may be diagnosed as a topology loop, but it must not be
    /// rejected by the client-prediction realtime contract.
    #[test]
    fn authoritative_force_loop_is_not_rejected_as_prediction_failure() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        init_builtin_ports(&mut world);
        world.init_resource::<crate::diagnostics::CosimDiagnostics>();
        world.init_resource::<lunco_core::RuntimeFaults>();
        world.insert_resource(lunco_core::NetworkRole::Standalone);

        let body = world
            .spawn((
                GlobalEntityId::from_raw(10),
                avian3d::prelude::RigidBody::Dynamic,
            ))
            .id();
        let program = world
            .spawn((
                GlobalEntityId::from_raw(20),
                SimComponent {
                    inputs: std::collections::HashMap::from([("height".into(), 0.0)]),
                    ..default()
                },
            ))
            .id();
        wire(&mut world, body, "height", program, "height");
        wire(&mut world, program, "netForce", body, "force_y");

        world.run_system_once(propagate_connections).unwrap();

        let diag = world.resource::<crate::diagnostics::CosimDiagnostics>();
        assert_eq!(diag.algebraic_loops.len(), 1);
        assert!(!diag.algebraic_loops[0].rejected);
        assert!(
            diag.faults.is_empty(),
            "unexpected faults: {:?}",
            diag.faults
        );
        assert!(
            world
                .resource::<lunco_core::RuntimeFaults>()
                .first
                .is_none(),
            "authoritative feedback is not a client-prediction safety failure"
        );
    }

    /// M6: an entity wired to ITSELF (`netForce`→`force_y` on one entity — the
    /// balloon pattern) is the intended single-participant co-sim shape, not an
    /// algebraic loop. Reporting it would false-positive every balloon scene.
    #[test]
    fn a_single_entity_self_wire_is_not_a_loop_fault() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.init_resource::<PortRegistry>();
        world.init_resource::<crate::diagnostics::CosimDiagnostics>();

        let balloon = world
            .spawn((GlobalEntityId::from_raw(10), lunco_core::PortSurfacePending))
            .id();
        wire(&mut world, balloon, "netForce", balloon, "force_y");
        wire(&mut world, balloon, "height", balloon, "height");

        world.run_system_once(propagate_connections).unwrap();

        assert!(
            world
                .resource::<crate::diagnostics::CosimDiagnostics>()
                .faults
                .is_empty(),
            "self-wires on one entity are causal feedback, not missing ports"
        );
    }

    /// An acyclic chain (with a fan-in for good measure) also reports nothing.
    #[test]
    fn an_acyclic_chain_is_not_a_loop_fault() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.init_resource::<PortRegistry>();
        world.init_resource::<crate::diagnostics::CosimDiagnostics>();

        let a = world
            .spawn((GlobalEntityId::from_raw(10), lunco_core::PortSurfacePending))
            .id();
        let b = world
            .spawn((GlobalEntityId::from_raw(20), lunco_core::PortSurfacePending))
            .id();
        let c = world
            .spawn((GlobalEntityId::from_raw(30), lunco_core::PortSurfacePending))
            .id();
        wire(&mut world, a, "out", b, "in");
        wire(&mut world, b, "out", c, "in");
        wire(&mut world, a, "out", c, "bias"); // diamond edge, still acyclic

        world.run_system_once(propagate_connections).unwrap();

        assert!(
            world
                .resource::<crate::diagnostics::CosimDiagnostics>()
                .faults
                .is_empty(),
            "valid causal wires do not create synthetic loop faults"
        );
    }
}
