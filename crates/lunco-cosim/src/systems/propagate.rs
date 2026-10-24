//! Connection propagation — the co-simulation master's exchange step.
//!
//! Implements the FMI-CS "read outputs → write inputs" exchange over every
//! [`lunco_cosim_core::SimConnection`]. SSP unit conversion precedes the authored
//! gain and offset. Multiple wires into one input **sum** (a
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
use std::sync::Arc;

use bevy::prelude::*;

use lunco_core::RebuildOnChange;
use lunco_port_core::ports::{PortRegistry, ResolvedPort};

use lunco_cosim_core::{
    AlgebraicLoopDiagnostic, BoundConnection, BrokenConnection, CosimDiagnostics, PortHolds,
    RealtimeSafe, SimComponent, SimConnection, SimStatus,
};

use lunco_physics::force_ports::is_physics_force_port;

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
/// * it carries [`lunco_core_session::OwnedLocally`] — the body this session possesses
///   and predicts (input-replay reconciled), or a wheel of it
///   (`propagate_owned_to_wheels` mirrors the marker onto `ArticulatedLink`s), or
/// * it carries [`lunco_core_session::PredictedDynamic`] — a free body promoted to local
///   dynamic prediction (state reconciled), or
/// * it is **not replicated at all** ([`lunco_core_session::NetReplicate`] absent). This
///   clause is the load-bearing one for vessel control: the endpoints of a
///   rover's actuation graph are bare [`lunco_port_core::Port`] entities
///   (`lunco-usd-sim`'s generic USD wiring targets wheel drive/heading ports), which have
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
        world
            .get_resource::<lunco_core_session::NetworkRole>()
            .copied(),
        Some(lunco_core_session::NetworkRole::Client)
    );
    if !is_client {
        return true; // host / standalone: authoritative over everything
    }
    world
        .get::<lunco_core_session::OwnedLocally>(target)
        .is_some()
        || world
            .get::<lunco_core_session::PredictedDynamic>(target)
            .is_some()
        || world
            .get::<lunco_core_session::NetReplicate>(target)
            .is_none()
}

/// One compiled wire: source endpoint + affine gain + the *index* of its target
/// in [`CompiledWiring::targets`]. Connector names are owned here (cloned once at
/// compile time); resolved backends avoid repeating name scans in the tick loop.
///
/// `src_resolved` caches the FMI-style [`ResolvedPort`] handle when the owning
/// backend exposes slots, so the accumulate phase reads without re-resolving the
/// name. Input-side sources use their readable-input resolver; `None` is reserved
/// for a backend whose canonical access operation is name-based.
struct CompiledWire {
    src_entity: Entity,
    src_port: String,
    /// Read the source's input side — see [`SimConnection::start_is_input`].
    src_is_input: bool,
    src_resolved: Option<ResolvedPort>,
    /// Index into [`CompiledWiring::targets`] — the accumulator slot.
    dst_index: usize,
    scale: f64,
    offset: f64,
}

/// One compiled target: the input endpoint every wire into it accumulates onto,
/// with its resolved write handle (see [`CompiledWire::src_resolved`]).
struct CompiledTarget {
    entity: Entity,
    name: Arc<str>,
    resolved: Option<ResolvedPort>,
    has_port_surface: bool,
    waiting_for_binding: bool,
}

/// One force-producing feedback cycle found at compile time, spanning at least
/// two participants.
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
/// [`propagate_connections`] and rebuilt when either the
/// [`lunco_cosim_core::SimConnection`] set or port-surface revision changes.
///
/// Targets are interned to dense
/// indices so propagation accumulates into a plain `Vec<f64>` with no hashing.
/// Each endpoint is resolved to a [`ResolvedPort`] handle at compile time so the
/// hot loop exchanges by slot, not by name-scan across backends.
#[derive(Default)]
pub struct CompiledWiring {
    wires: Vec<CompiledWire>,
    /// Distinct targets, one accumulator slot each.
    targets: Vec<CompiledTarget>,
    /// Borrowed-name lookup used only for sparse manual holds; normal
    /// propagation remains a dense indexed pass.
    target_indices: HashMap<Entity, HashMap<Arc<str>, usize>>,
    /// Algebraic loops in this fabric, recomputed on every rebuild.
    loops: Vec<DetectedLoop>,
    contract_rejected: bool,
}

/// Shared propagation state for every schedule that replays the co-simulation
/// transaction. The normal fixed step and rollback replay call the same
/// propagation function; keeping the change detector and compiled fabric in a
/// resource prevents each schedule from maintaining a second wiring cache.
#[derive(Resource, Default)]
pub struct PropagationCache {
    wiring: RebuildOnChange<BoundConnection, CompiledWiring>,
    port_topology_revision: Option<u64>,
    scratch: PropagationScratch,
}

#[derive(Default)]
struct PropagationScratch {
    accumulator: Vec<f64>,
    unavailable: Vec<bool>,
    held_values: Vec<Option<f64>>,
    held_revision: Option<u64>,
    broken: Vec<BrokenConnection>,
    pending: Vec<BrokenConnection>,
    landed_targets: Vec<usize>,
    writes: lunco_port_core::ports::InputWriteBuffer,
    write_targets: Vec<usize>,
    /// Acknowledgements of successful ledger publication in this compiled fabric.
    /// These never admit a write or decide whether a failure is already landed.
    landing_published: Vec<bool>,
}

impl CompiledWiring {
    /// Recompile the fabric from the live [`SimConnection`] set and port
    /// surfaces. Runs only when wiring or the shared port-topology revision
    /// changes. Resolves every eligible endpoint to its [`ResolvedPort`] handle
    /// here, so the per-tick loop does not repeat name resolution.
    fn rebuild(&mut self, world: &mut World) {
        self.contract_rejected = false;
        self.wires.clear();
        self.targets.clear();
        self.target_indices.clear();

        // Share the registry snapshot so resolution below borrows `world`
        // immutably alongside the collected connections.
        let registry = world.resource::<PortRegistry>().clone();
        let mut q = world.query_filtered::<(Entity, &SimConnection), With<BoundConnection>>();
        let mut conns: Vec<_> = q
            .iter(world)
            .map(|(entity, spec)| (entity, spec.clone()))
            .collect();
        conns.sort_by(|a, b| a.1.compare_authoritative(&b.1, world));

        for (edge, c) in &conns {
            if c.start_element == Entity::PLACEHOLDER || c.end_element == Entity::PLACEHOLDER {
                continue;
            }
            let (scale, offset) = match c.resolved_transform(&registry, world) {
                Ok(transform) => transform,
                Err(detail) => {
                    crate::binding::reject_connection_contract(world, *edge, c, detail, None);
                    self.contract_rejected = true;
                    // No target receives a partial fabric when a changed port
                    // contract invalidates an already bound connection.
                    return;
                }
            };
            let dst_index = self
                .target_indices
                .get(&c.end_element)
                .and_then(|ports| ports.get(c.end_connector.as_str()))
                .copied()
                .unwrap_or_else(|| {
                    let i = self.targets.len();
                    let name: Arc<str> = Arc::from(c.end_connector.as_str());
                    // Resolve the target's input handle once (fast-path backends only).
                    let resolved = registry.resolve_input(world, c.end_element, &c.end_connector);
                    let has_port_surface = resolved.is_some()
                        || !registry.entity_port_owners(world, c.end_element).is_empty();
                    self.targets.push(CompiledTarget {
                        entity: c.end_element,
                        name: Arc::clone(&name),
                        resolved,
                        has_port_surface,
                        waiting_for_binding: false,
                    });
                    self.target_indices
                        .entry(c.end_element)
                        .or_default()
                        .insert(name, i);
                    i
                });
            // Resolve against the correct causality side once. Input writes and
            // readable input sources have distinct resolver semantics because a
            // writable input may intentionally have no readable value.
            let src_resolved = if c.start_is_input {
                registry.resolve_input_read(world, c.start_element, &c.start_connector)
            } else {
                registry.resolve_output(world, c.start_element, &c.start_connector)
            };
            self.wires.push(CompiledWire {
                src_entity: c.start_element,
                src_port: c.start_connector.clone(),
                src_is_input: c.start_is_input,
                src_resolved,
                dst_index,
                scale,
                offset,
            });
        }

        // An admitted source cannot stand in for another authored contribution
        // whose interface is still binding. Its target must retain the whole
        // previous input, just as it does for an unavailable bound sample.
        let mut pending = world.query_filtered::<&SimConnection, Without<BoundConnection>>();
        for wire in pending.iter(world) {
            if let Some(index) = self
                .target_indices
                .get(&wire.end_element)
                .and_then(|ports| ports.get(wire.end_connector.as_str()))
            {
                self.targets[*index].waiting_for_binding = true;
            }
        }
        // Source and target order was admitted through the shared connection
        // comparator before assigning dense slots. The tick does no sorting.
        self.detect_force_feedback_loops(world);
    }

    /// Find force-producing feedback SCCs over the explicit causal wire graph.
    ///
    /// The master is single-pass Jacobi with ZOH inputs: each feedthrough hop on
    /// a cycle costs one fixed step of delay, and nothing iterates the loop to
    /// convergence. That 1-step-delay behaviour is the documented contract for
    /// this causal fabric. A causal cycle that does not reach a force or torque
    /// input is therefore ordinary dynamic feedback, not an unresolved
    /// algebraic loop and not a diagnostic. Only force-producing cycles need a
    /// topology entry for the client-prediction safety decision below.
    ///
    /// Single-entity self-wires (`netForce`→`force_y` on ONE entity — the
    /// balloon pattern, an engine exchanging with its own body) are the intended
    /// single-participant co-sim shape and are excluded: a reported cycle must
    /// span ≥ 2 participants (an SCC of ≥ 2 nodes, via iterative Tarjan).
    fn detect_force_feedback_loops(&mut self, world: &World) {
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
                        world
                            .get_resource::<lunco_core_session::NetworkRole>()
                            .copied(),
                        Some(lunco_core_session::NetworkRole::Client)
                    );
                    let predicted_dynamic = world
                        .get::<avian3d::prelude::RigidBody>(dst.entity)
                        .is_some_and(|body| matches!(body, avian3d::prelude::RigidBody::Dynamic))
                        && world
                            .get::<lunco_core_session::NotPredictable>(dst.entity)
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
                .filter(|member| world.get::<SimComponent>(*member).is_some())
                .collect();
            let realtime_safe = force_producing
                && !programs.is_empty()
                && programs
                    .iter()
                    .all(|program| world.get::<RealtimeSafe>(*program).is_some());
            if !force_producing {
                continue;
            }
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
/// which needs whole-world access. It caches the compiled fabric in the shared
/// [`PropagationCache`] resource and rebuilds it when bound connections or
/// `PortTopologyRevision` change, so calling this system alone (e.g. in tests,
/// without the full schedule) both compiles and propagates. It reuses the
/// compiled connection table rather than rebuilding a per-tick query snapshot:
///
/// 1. **Recompile-if-changed** — [`RebuildOnChange`] tracks bound-connection
///    changes; the owner-published port-topology revision invalidates the same
///    cache when a port surface changes. Numeric samples do not rebuild the
///    table. Fixed and rollback schedules share this one compiled reader table.
/// 2. **Seed** — every target's accumulator slot to `0.0`, so a target whose
///    source vanished cleanly returns to zero.
/// 3. **Accumulate** — read slot-backed sources through their resolved backend
///    handles and named-only owners through their canonical named operation;
///    sum `src*scale+offset` into `acc[dst_index]`.
/// 4. **Write** — write each target once through its resolved handle where
///    available, otherwise through its owning backend's named operation. A
///    target with no such input port is a dangling wire — reported, not silently
///    dropped.
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
pub fn propagate_connections(world: &mut World) {
    world.resource_scope(|world, mut cache: Mut<PropagationCache>| {
        let topology_revision = world
            .get_resource::<lunco_port_core::ports::PortTopologyRevision>()
            .map(|revision| revision.0);
        if cache.port_topology_revision != topology_revision {
            cache.wiring.invalidate();
            cache.port_topology_revision = topology_revision;
        }
        let PropagationCache {
            wiring, scratch, ..
        } = &mut *cache;
        propagate_connections_with_cache(world, wiring, scratch);
    });
}

fn propagate_connections_with_cache(
    world: &mut World,
    wiring: &mut RebuildOnChange<BoundConnection, CompiledWiring>,
    scratch: &mut PropagationScratch,
) {
    scratch.broken.clear();
    scratch.pending.clear();
    scratch.landed_targets.clear();
    // Share the registry snapshot so the write phase can take `&mut World`
    // without holding a resource borrow or copying the backend table.
    let registry = world.resource::<PortRegistry>().clone();

    // Phase 1: recompile the fabric iff the connection set or port surface changed. The compiled
    // fabric is owned by the shared cache (no world borrow), so the phases below keep
    // `&mut World` for the resolver.
    let topology_span = bevy::log::info_span!("cosim_exchange_topology").entered();
    let mut rewired = false;
    let compiled = wiring.get_or_rebuild(world, |compiled, world| {
        rewired = true;
        compiled.rebuild(world)
    });
    if compiled.contract_rejected {
        return;
    }
    if rewired {
        scratch.landing_published.clear();
        scratch
            .landing_published
            .resize(compiled.targets.len(), false);
        // Retained diagnostics and successful publication acknowledgements
        // share endpoint lifetimes. Topology loops have their own collection.
        // Scene teardown clears all three; ordinary endpoint retirement clears
        // only that endpoint's retained records.
        //
        // Despawn is the exact retirement condition, and a rewire is when it is
        // cheap to notice: prim despawn is one of the two things that rebuild the
        // fabric. Keys are collected before taking the resource borrow because the
        // liveness test needs `&World`.
        let dead: Vec<Entity> = {
            let diag = world.resource::<CosimDiagnostics>();
            diag.faults
                .keys()
                .chain(diag.landed.keys())
                .filter(|entity| !world.entities().contains(**entity))
                .copied()
                .collect()
        };
        if !dead.is_empty() {
            let mut diag = world.resource_mut::<CosimDiagnostics>();
            for entity in dead {
                diag.forget_entity(entity);
            }
        }

        // Force-producing feedback cycles describe a client-prediction safety
        // decision. Ordinary causal cycles are valid dynamic feedback and are
        // intentionally absent from this collection.
        let loops = compiled
            .loops
            .iter()
            .map(|loop_info| AlgebraicLoopDiagnostic {
                entity: loop_info.entity,
                global_id: loop_info.global_id,
                detail: loop_info.detail.clone(),
                force_producing: loop_info.force_producing,
                rejected: loop_info.force_producing
                    && loop_info.requires_realtime_safe
                    && !loop_info.realtime_safe,
            })
            .collect();
        world.resource_mut::<CosimDiagnostics>().algebraic_loops = loops;
        for loop_info in &compiled.loops {
            if loop_info.force_producing
                && loop_info.requires_realtime_safe
                && loop_info.realtime_safe
            {
                if world
                    .resource_mut::<CosimDiagnostics>()
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
                .resource_mut::<CosimDiagnostics>()
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
                crate::failure::stop(
                    world,
                    "cosim-unsafe-force-loop",
                    Some(loop_info.entity),
                    "co-simulation wiring",
                    loop_info.detail.clone(),
                );
                return;
            }
        }
    }
    drop(topology_span);

    if compiled.targets.is_empty() {
        // No wires ⇒ nothing broken. Clear any report left from a prior fabric.
        let mut diag = world.resource_mut::<CosimDiagnostics>();
        if !diag.broken.is_empty() || !diag.pending.is_empty() {
            diag.broken.clear();
            diag.pending.clear();
        }
        return;
    }

    // Phase 2: seed accumulator slots.
    let accumulate_span = bevy::log::info_span!("cosim_exchange_accumulate").entered();
    scratch.accumulator.clear();
    scratch.accumulator.resize(compiled.targets.len(), 0.0);
    scratch.unavailable.clear();
    scratch.unavailable.extend(
        compiled
            .targets
            .iter()
            .map(|target| target.waiting_for_binding),
    );

    // Phase 3: accumulate. Use a resolved handle whenever the winning backend
    // supports slots. Port-surface revisions recompile the handles before this
    // transaction, so a stale locator is never silently rerouted by name.
    for w in &compiled.wires {
        let read_src = |w: &CompiledWire| {
            lunco_cosim_core::initialization::read_causal_sample(
                &registry,
                world,
                w.src_entity,
                &w.src_port,
                w.src_is_input,
            )
        };
        let src = match &w.src_resolved {
            Some(resolved) => registry.read_resolved(world, w.src_entity, resolved),
            None => read_src(w),
        };
        let Some(src) = src else {
            scratch.unavailable[w.dst_index] = true;
            if !w.src_is_input
                && world
                    .get::<lunco_cosim_core::RequiredOutputSamples>(w.src_entity)
                    .is_some_and(|required| required.0.iter().any(|name| name == &w.src_port))
            {
                crate::failure::stop(
                    world,
                    "cosim-required-sample-unavailable",
                    Some(w.src_entity),
                    "co-simulation exchange",
                    format!("required output '{}' has no physical sample", w.src_port),
                );
                return;
            }
            continue;
        };
        scratch.accumulator[w.dst_index] += src * w.scale + w.offset;
    }
    drop(accumulate_span);

    // Phase 4: preflight every eligible target before applying the input batch.
    // Gated per target (see `peer_simulates`), never by process role.
    // Terminal failures, rebuilt every tick so `GET /api/diagnostics` polls the
    // current fabric (see `CosimDiagnostics`).
    // A scene may wire an endpoint before its runtime contract is published.
    // Generated Modelica islands are the important case: their interface is
    // provisional while compiling, not an authoring error. These buffers retain
    // capacity between ticks and are swapped with the published snapshots below.
    // Controller holds outrank authored-program setpoints, which outrank the
    // wiring fabric — see `lunco_cosim_core::PortHolds`. Both are explicit
    // intents that remain live across fixed ticks. Explicit port/control release
    // clears requested holds; authority lifecycle release clears controller
    // holds only; scene teardown clears both layers.
    let holds_span = bevy::log::info_span!("cosim_exchange_holds").entered();
    let holds = world.get_resource::<PortHolds>();
    let hold_revision = holds.map_or(0, PortHolds::revision);
    if rewired || scratch.held_revision != Some(hold_revision) {
        scratch.held_values.clear();
        scratch.held_values.resize(compiled.targets.len(), None);
        if let Some(holds) = holds.filter(|holds| !holds.is_empty()) {
            for (entity, name, value) in holds.iter() {
                if let Some(index) = compiled
                    .target_indices
                    .get(&entity)
                    .and_then(|ports| ports.get(name))
                {
                    scratch.held_values[*index] = Some(value);
                }
            }
        }
        scratch.held_revision = Some(hold_revision);
    }
    drop(holds_span);
    let write_span = bevy::log::info_span!("cosim_exchange_write").entered();
    scratch.write_targets.clear();
    let committed = registry.input_transaction(world, &mut scratch.writes, |transaction| {
        for (i, t) in compiled.targets.iter().enumerate() {
            if !peer_simulates(transaction.world(), t.entity) {
                continue;
            }
            if scratch.unavailable[i] && scratch.held_values[i].is_none() {
                scratch.pending.push(BrokenConnection {
                    entity: t.entity,
                    global_id: transaction
                        .world()
                        .get::<lunco_core::GlobalEntityId>(t.entity)
                        .copied(),
                    port: Arc::clone(&t.name),
                    has_port_surface: t.has_port_surface,
                    dropped_value: None,
                    failure: Some("connected source sample unavailable".into()),
                });
                continue;
            }
            // A HELD port is not driven by its wire. Without this, a `SetPorts` write on a
            // wired input is overwritten by the next propagation tick — the write
            // "succeeds" and nothing happens, which is indistinguishable from a
            // broken port to whoever sent it.
            let value = scratch.held_values[i].unwrap_or(scratch.accumulator[i]);
            let prepared = match &t.resolved {
                Some(resolved) => transaction.stage_resolved(t.entity, resolved, value),
                None => transaction.stage_input(t.entity, Arc::clone(&t.name), value),
            };
            let failure = match prepared {
                Ok(()) => {
                    scratch.write_targets.push(i);
                    continue;
                }
                Err(error) => error,
            };
            let unresolved = BrokenConnection {
                entity: t.entity,
                global_id: transaction
                    .world()
                    .get::<lunco_core::GlobalEntityId>(t.entity)
                    .copied(),
                port: Arc::clone(&t.name),
                has_port_surface: t.has_port_surface,
                dropped_value: Some(value),
                failure: Some(failure.to_string()),
            };
            let model_status = transaction
                .world()
                .get::<SimComponent>(t.entity)
                .map(|component| &component.status);
            let compiling = matches!(model_status, Some(SimStatus::Compiling));
            // Only an explicitly published pending marker or a compiling Modelica
            // component is assembly progress. Structural Motor/Gearbox edges are
            // filtered during USD wire derivation, so a bare endpoint with no
            // surface is a terminal authoring fault rather than a forever-pending
            // wire.
            let surface_pending = transaction
                .world()
                .get::<lunco_port_core::PortSurfacePending>(t.entity)
                .is_some();
            if compiling || surface_pending {
                scratch.pending.push(unresolved);
                continue;
            }

            return Err((i, value, failure.to_string()));
        }
        Ok(())
    });
    match committed {
        Ok(()) => {
            for index in scratch.write_targets.iter().copied() {
                if !scratch.landing_published[index] {
                    scratch.landed_targets.push(index);
                }
            }
        }
        Err((index, value, failure)) => {
            let target = &compiled.targets[index];
            scratch.landing_published[index] = false;
            scratch.broken.push(crate::failure::reject_input(
                world,
                "cosim-input-write-rejected",
                target.entity,
                Arc::clone(&target.name),
                Some(value),
                failure,
            ));
        }
    }
    drop(write_span);

    // Publish only committed successes. A failed preflight or commit cannot
    // acknowledge another target from the same rejected communication point.
    let _span = bevy::log::info_span!("cosim_exchange_diagnostics").entered();
    let mut diag = world.resource_mut::<CosimDiagnostics>();
    for index in scratch.landed_targets.drain(..) {
        let target = &compiled.targets[index];
        diag.mark_landed_shared(target.entity, &target.name);
        diag.remove_fault(target.entity, &target.name);
        scratch.landing_published[index] = true;
    }
    std::mem::swap(&mut diag.pending, &mut scratch.pending);
    std::mem::swap(&mut diag.broken, &mut scratch.broken);
}

#[cfg(test)]
mod wire_order_tests {
    use super::*;
    use SimComponent;
    use lunco_core::GlobalEntityId;

    fn diagnostic_publication_world(source_count: usize, fanout: usize) -> (World, Vec<Entity>) {
        use lunco_port_core::ports::PortDirection;
        use lunco_port_core::{Port, PortSurface, PortSurfacePort};
        let mut world = World::new();
        world.init_resource::<lunco_port_core::ports::PortTopologyRevision>();
        world.init_resource::<PropagationCache>();
        world.init_resource::<CosimDiagnostics>();
        world.init_resource::<PortHolds>();
        world.init_resource::<lunco_cosim_core::BindingRevision>();
        let mut registry = PortRegistry::default();
        crate::ports::register_builtin_port_backends(&mut registry);
        world.insert_resource(registry);
        let mut endpoints = Vec::new();
        for _ in 0..source_count {
            let source_port = world.spawn(Port { value: 2.0 }).id();
            let source = world
                .spawn(PortSurface::new(HashMap::from([(
                    "signal".into(),
                    PortSurfacePort::new(source_port, PortDirection::Out),
                )])))
                .id();
            for _ in 0..fanout {
                let endpoint = world.spawn(Port::default()).id();
                let target = world
                    .spawn(PortSurface::new(HashMap::from([(
                        "command".into(),
                        PortSurfacePort::new(endpoint, PortDirection::In),
                    )])))
                    .id();
                wire(&mut world, source, "signal", target, "command");
                endpoints.push(endpoint);
            }
        }
        (world, endpoints)
    }

    /// Isolates steady exchange and its diagnostic publication with inline surfaces.
    #[test]
    #[ignore = "manual diagnostic publication performance measurement"]
    fn settled_diagnostic_publication_benchmark() {
        for (sources, fanout) in [(1, 128), (32, 4), (128, 1)] {
            let (mut world, endpoints) = diagnostic_publication_world(sources, fanout);
            propagate_connections(&mut world);
            let began = std::time::Instant::now();
            for _ in 0..6000 {
                propagate_connections(&mut world);
            }
            let elapsed = began.elapsed();
            assert_eq!(
                world.resource::<CosimDiagnostics>().landed.len(),
                endpoints.len()
            );
            for endpoint in endpoints {
                assert_eq!(
                    world
                        .get::<lunco_port_core::Port>(endpoint)
                        .unwrap()
                        .value
                        .to_bits(),
                    2.0f64.to_bits()
                );
            }
            println!(
                "DIAGNOSTIC_PUBLICATION_BENCH sources={sources} fanout={fanout} elapsed_ns={} writes=768000 final_bits={:016x}",
                elapsed.as_nanos(),
                2.0f64.to_bits()
            );
        }
    }

    /// The private publication cursor follows the fabric and scene resource seams.
    #[test]
    fn landing_publication_acknowledgements_reset_with_fabric_and_scene() {
        use bevy::ecs::system::RunSystemOnce;
        let (mut world, endpoints) = diagnostic_publication_world(1, 128);
        propagate_connections(&mut world);
        assert!(
            world
                .resource::<PropagationCache>()
                .scratch
                .landing_published
                .iter()
                .all(|published| *published)
        );
        let missing = world.spawn(lunco_port_core::Port { value: 7.0 }).id();
        let source = world.spawn(participant(&[], &["out"])).id();
        wire(&mut world, source, "out", missing, "value");
        propagate_connections(&mut world);
        let published = &world
            .resource::<PropagationCache>()
            .scratch
            .landing_published;
        assert_eq!(
            published.iter().filter(|published| **published).count(),
            endpoints.len()
        );
        assert_eq!(published.iter().filter(|published| !**published).count(), 1);
        assert!(
            world
                .resource::<CosimDiagnostics>()
                .pending
                .iter()
                .any(|item| item.entity == missing && item.dropped_value.is_none())
        );
        // The real teardown boundary also runs before the scene's entities are reclaimed.
        world.run_system_once(crate::reset_scene_state).unwrap();
        assert!(
            world
                .resource::<PropagationCache>()
                .scratch
                .landing_published
                .is_empty()
        );
        assert!(world.resource::<CosimDiagnostics>().landed.is_empty());
        propagate_connections(&mut world);
        assert_eq!(
            world.resource::<CosimDiagnostics>().landed.len(),
            endpoints.len()
        );
        assert!(
            world
                .resource::<CosimDiagnostics>()
                .pending
                .iter()
                .any(|item| item.entity == missing && item.dropped_value.is_none())
        );
    }

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
            world.init_resource::<lunco_port_core::ports::PortTopologyRevision>();
            init_builtin_ports(&mut world);

            // Three distinct sources with stable network ids 10/20/30.
            let mut src = std::collections::HashMap::new();
            for gid in [10_u64, 20, 30] {
                src.insert(
                    gid,
                    world
                        .spawn((
                            GlobalEntityId::from_raw(gid),
                            participant(&[], &[&format!("out_{gid}")]),
                        ))
                        .id(),
                );
            }
            let sink = world.spawn(participant(&["force_y"], &[])).id();

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
                .map(|w| {
                    (
                        world.get::<GlobalEntityId>(w.src_entity).map(|id| id.get()),
                        w.src_port.clone(),
                    )
                })
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

    #[test]
    fn port_topology_revision_recompiles_resolved_map_slots() {
        use bevy::ecs::system::RunSystemOnce;
        use lunco_port_core::ports::PortTopologyRevision;

        let mut world = World::new();
        world.init_resource::<CosimDiagnostics>();
        world.init_resource::<PropagationCache>();
        world.init_resource::<PortTopologyRevision>();
        let mut registry = PortRegistry::default();
        crate::ports::register_builtin_port_backends(&mut registry);
        world.insert_resource(registry);

        let source = world
            .spawn(SimComponent {
                outputs: std::collections::HashMap::from([("out".to_owned(), 7.0)]).into(),
                ..Default::default()
            })
            .id();
        let target = world
            .spawn(SimComponent {
                inputs: std::collections::HashMap::from([("demand".to_owned(), 0.0)]).into(),
                ..Default::default()
            })
            .id();
        world.spawn((
            SimConnection {
                start_element: source,
                start_connector: "out".into(),
                start_is_input: false,
                end_element: target,
                end_connector: "demand".into(),
                scale: 1.0,
                offset: 0.0,
            },
            BoundConnection,
        ));

        world.run_system_once(propagate_connections).unwrap();
        assert_eq!(
            world
                .get::<SimComponent>(target)
                .unwrap()
                .inputs
                .get("demand"),
            Some(&7.0)
        );

        // Removing and re-adding the same name leaves the authored surface
        // unchanged but retires that exact slot. The owner publishes one shared
        // topology revision; propagation recompiles once at its deterministic
        // boundary before using the endpoint again.
        {
            let mut component = world.get_mut::<SimComponent>(source).unwrap();
            component.outputs.remove("out");
            component.outputs.insert("out".into(), 7.0);
        }
        world.resource_mut::<PortTopologyRevision>().bump();
        world.run_system_once(propagate_connections).unwrap();

        assert_eq!(
            world
                .get::<SimComponent>(target)
                .unwrap()
                .inputs
                .get("demand"),
            Some(&7.0)
        );
    }

    #[test]
    fn readable_input_source_uses_the_compiled_input_slot() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.init_resource::<lunco_port_core::ports::PortTopologyRevision>();
        world.init_resource::<CosimDiagnostics>();
        world.init_resource::<PropagationCache>();
        let mut registry = PortRegistry::default();
        crate::ports::register_builtin_port_backends(&mut registry);
        world.insert_resource(registry);

        let source = world
            .spawn(SimComponent {
                inputs: std::collections::HashMap::from([("command".to_owned(), 0.75)]).into(),
                ..Default::default()
            })
            .id();
        let target = world
            .spawn(SimComponent {
                inputs: std::collections::HashMap::from([("demand".to_owned(), 0.0)]).into(),
                ..Default::default()
            })
            .id();
        world.spawn((
            SimConnection {
                start_element: source,
                start_connector: "command".into(),
                start_is_input: true,
                end_element: target,
                end_connector: "demand".into(),
                scale: 2.0,
                offset: 0.25,
            },
            BoundConnection,
        ));

        world.run_system_once(propagate_connections).unwrap();

        assert_eq!(
            world
                .get::<SimComponent>(target)
                .unwrap()
                .inputs
                .get("demand"),
            Some(&1.75)
        );
    }

    /// A persisted intent outranks the wire into the same port, and hands it
    /// back only when explicitly released.
    ///
    /// This is the failure the hold exists for: writing a wired input directly
    /// "succeeds" and is overwritten by the very next propagation tick, so a
    /// setpoint on a driven port lasts under 16 ms and looks, from the caller's
    /// side, exactly like a port that does not exist.
    #[test]
    fn a_hold_outranks_its_wire_until_explicit_release() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        world.init_resource::<lunco_port_core::ports::PortTopologyRevision>();
        world.init_resource::<CosimDiagnostics>();
        world.init_resource::<PropagationCache>();
        world.init_resource::<PortHolds>();
        world.init_resource::<Time<bevy::time::Real>>();

        // One backend over `SimComponent`, so the sink has a real writable port.
        let mut registry = PortRegistry::default();
        crate::ports::register_builtin_port_backends(&mut registry);
        world.insert_resource(registry);

        let src = world
            .spawn(SimComponent {
                outputs: std::collections::HashMap::from([("out".to_string(), 7.0)]).into(),
                ..Default::default()
            })
            .id();
        let sink = world
            .spawn(SimComponent {
                inputs: std::collections::HashMap::from([("demand".to_string(), 0.0)]).into(),
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
                .get::<SimComponent>(sink)
                .and_then(|c| c.inputs.get("demand").copied())
                .unwrap_or(f64::NAN)
        };

        world.run_system_once(propagate_connections).unwrap();
        assert_eq!(demand(&world), 7.0, "the wire drives the port");

        // Hold it somewhere else. The wire keeps producing 7.0 every tick.
        world.resource_mut::<PortHolds>().hold(sink, "demand", -1.0);
        world.run_system_once(propagate_connections).unwrap();
        assert_eq!(
            demand(&world),
            -1.0,
            "a live hold outranks the wire — this is what makes a setpoint on a driven port stick"
        );

        // The intent remains live across arbitrary fixed ticks. An explicit
        // release hands the port back to its authored wire.
        world.resource_mut::<PortHolds>().release(sink, "demand");
        world.run_system_once(propagate_connections).unwrap();
        assert_eq!(demand(&world), 7.0, "an explicit release restores the wire");
        assert!(
            world.resource::<PortHolds>().is_empty(),
            "the released entry is dropped, not merely ignored"
        );
    }

    #[test]
    fn incomplete_fanin_preserves_the_input_until_all_sources_publish() {
        let mut world = World::new();
        init_builtin_ports(&mut world);
        world.init_resource::<PortHolds>();
        world.init_resource::<CosimDiagnostics>();
        let pending = world.spawn(participant(&[], &["out"])).id();
        let mut ready = SimComponent::default();
        ready.outputs.insert("out".into(), 3.0);
        let ready = world.spawn(ready).id();
        let target = world.spawn(participant(&["demand"], &[])).id();
        world
            .get_mut::<SimComponent>(target)
            .unwrap()
            .inputs
            .set("demand", 7.0);
        let pending_edge = wire(&mut world, pending, "out", target, "demand");
        wire(&mut world, ready, "out", target, "demand");
        propagate_connections(&mut world);
        assert_eq!(
            world
                .get::<SimComponent>(target)
                .unwrap()
                .inputs
                .get("demand"),
            Some(&7.0)
        );
        assert_eq!(world.resource::<CosimDiagnostics>().pending.len(), 1);
        assert!(
            world.resource::<CosimDiagnostics>().pending[0]
                .dropped_value
                .is_none()
        );
        world
            .resource_mut::<PortHolds>()
            .hold(target, "demand", 11.0);
        propagate_connections(&mut world);
        assert_eq!(
            world
                .get::<SimComponent>(target)
                .unwrap()
                .inputs
                .get("demand"),
            Some(&11.0),
            "holds={:?}, cached={:?}, pending={:?}, broken={:?}",
            world.resource::<PortHolds>().snapshot(),
            world.resource::<PropagationCache>().scratch.held_values,
            world.resource::<CosimDiagnostics>().pending,
            world.resource::<CosimDiagnostics>().broken
        );
        world.resource_mut::<PortHolds>().release(target, "demand");
        world
            .get_mut::<SimComponent>(pending)
            .unwrap()
            .outputs
            .insert("out".into(), 2.0);
        world.entity_mut(pending_edge).remove::<BoundConnection>();
        world
            .resource_mut::<lunco_port_core::ports::PortTopologyRevision>()
            .bump();
        propagate_connections(&mut world);
        assert_eq!(
            world
                .get::<SimComponent>(target)
                .unwrap()
                .inputs
                .get("demand"),
            Some(&11.0),
            "a source outside the bound fabric cannot be replaced by a partial fan-in"
        );
        world.entity_mut(pending_edge).insert(BoundConnection);
        world
            .resource_mut::<lunco_port_core::ports::PortTopologyRevision>()
            .bump();
        propagate_connections(&mut world);
        assert_eq!(
            world
                .get::<SimComponent>(target)
                .unwrap()
                .inputs
                .get("demand"),
            Some(&5.0)
        );
        assert!(world.resource::<CosimDiagnostics>().pending.is_empty());
    }

    fn participant(
        inputs: &[&str],
        outputs: &[&str],
    ) -> (SimComponent, lunco_cosim_core::DeclaredOutputPorts) {
        (
            SimComponent {
                inputs: inputs.iter().map(|name| (name.to_string(), 0.0)).collect(),
                ..Default::default()
            },
            lunco_cosim_core::DeclaredOutputPorts {
                names: outputs.iter().map(|name| name.to_string()).collect(),
            },
        )
    }

    fn wire(world: &mut World, src: Entity, out: &str, dst: Entity, inp: &str) -> Entity {
        world
            .spawn((
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
            ))
            .id()
    }

    fn init_builtin_ports(world: &mut World) {
        world.init_resource::<lunco_port_core::ports::PortTopologyRevision>();
        world.init_resource::<PropagationCache>();
        let mut registry = PortRegistry::default();
        crate::ports::register_builtin_port_backends(&mut registry);
        world.insert_resource(registry);
    }

    fn loop_diagnostics(world: &World) -> Vec<String> {
        world
            .resource::<CosimDiagnostics>()
            .algebraic_loops
            .iter()
            .map(|loop_diag| loop_diag.detail.clone())
            .collect()
    }

    /// A cycle spanning two explicit causal participants is dynamic feedback.
    /// The single-pass ZOH exchange gives it the documented step semantics, so
    /// it must not be reported as an unresolved algebraic loop.
    #[test]
    fn a_two_entity_causal_feedback_loop_is_not_an_algebraic_diagnostic() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.init_resource::<lunco_port_core::ports::PortTopologyRevision>();
        init_builtin_ports(&mut world);
        world.init_resource::<CosimDiagnostics>();

        let a = world
            .spawn((
                GlobalEntityId::from_raw(10),
                participant(
                    &["in", "bias", "force_y", "height"],
                    &["out", "netForce", "height"],
                ),
            ))
            .id();
        let b = world
            .spawn((
                GlobalEntityId::from_raw(20),
                participant(
                    &["in", "bias", "force_y", "height"],
                    &["out", "netForce", "height"],
                ),
            ))
            .id();
        wire(&mut world, a, "out", b, "in");
        wire(&mut world, b, "out", a, "in");

        world.run_system_once(propagate_connections).unwrap();

        let loops = loop_diagnostics(&world);
        assert!(
            loops.is_empty(),
            "ordinary causal feedback is not an algebraic diagnostic: {loops:?}"
        );
        assert!(
            world.resource::<CosimDiagnostics>().faults.is_empty(),
            "a topology loop is not a missing-port fault"
        );
        // Removing the back-edge keeps the causal fabric clean after rebuild.
        let conns: Vec<Entity> = {
            let mut q = world.query_filtered::<Entity, With<SimConnection>>();
            q.iter(&world).collect()
        };
        world.entity_mut(conns[1]).despawn();
        world.run_system_once(propagate_connections).unwrap();
        assert!(
            loop_diagnostics(&world).is_empty(),
            "removing the back-edge keeps the causal fabric clean"
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
        world.init_resource::<lunco_port_core::ports::PortTopologyRevision>();
        init_builtin_ports(&mut world);
        world.init_resource::<CosimDiagnostics>();

        let a = world
            .spawn((
                GlobalEntityId::from_raw(10),
                SimComponent {
                    inputs: std::collections::HashMap::from([("in".into(), 0.0)]).into(),
                    ..default()
                },
                lunco_cosim_core::DeclaredOutputPorts {
                    names: ["out".into(), "force".into()].into(),
                },
                RealtimeSafe,
                avian3d::prelude::RigidBody::Dynamic,
            ))
            .id();
        let b = world
            .spawn((
                GlobalEntityId::from_raw(20),
                SimComponent {
                    inputs: std::collections::HashMap::from([("in".into(), 0.0)]).into(),
                    ..default()
                },
                lunco_cosim_core::DeclaredOutputPorts {
                    names: ["out".into()].into(),
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
        let diag = world.resource::<CosimDiagnostics>();
        assert!(!diag.algebraic_loops[0].rejected);
        assert!(diag.faults.is_empty());
    }

    /// The physical body is not a program and must not need a realtime promise
    /// of its own. A program without that promise is still unsafe.
    #[test]
    fn force_loop_requires_realtime_safe_on_program_not_body() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.init_resource::<lunco_port_core::ports::PortTopologyRevision>();
        init_builtin_ports(&mut world);
        world.init_resource::<CosimDiagnostics>();
        world.init_resource::<lunco_core::RuntimeFaults>();
        world.insert_resource(lunco_core_session::NetworkRole::Client);

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
                    inputs: std::collections::HashMap::from([("height".into(), 0.0)]).into(),
                    ..default()
                },
                lunco_cosim_core::DeclaredOutputPorts {
                    names: ["netForce".into()].into(),
                },
                RealtimeSafe,
            ))
            .id();
        wire(&mut world, body, "position_y", program, "height");
        wire(&mut world, program, "netForce", body, "force_y");

        world.run_system_once(propagate_connections).unwrap();
        assert!(
            !loop_diagnostics(&world).is_empty(),
            "the current topology remains observable when its force loop is accepted"
        );
        assert!(!world.resource::<CosimDiagnostics>().algebraic_loops[0].rejected);
        assert!(
            world.resource::<CosimDiagnostics>().faults.is_empty(),
            "unexpected faults: {:?}",
            world.resource::<CosimDiagnostics>().faults
        );
        assert!(
            world
                .resource::<lunco_core::RuntimeFaults>()
                .first
                .is_none()
        );

        let mut unsafe_world = World::new();
        unsafe_world.init_resource::<lunco_port_core::ports::PortTopologyRevision>();
        init_builtin_ports(&mut unsafe_world);
        unsafe_world.init_resource::<CosimDiagnostics>();
        unsafe_world.init_resource::<lunco_core::RuntimeFaults>();
        unsafe_world.insert_resource(lunco_core_session::NetworkRole::Client);
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
                    inputs: std::collections::HashMap::from([("height".into(), 0.0)]).into(),
                    ..default()
                },
                lunco_cosim_core::DeclaredOutputPorts {
                    names: ["netForce".into()].into(),
                },
            ))
            .id();
        wire(&mut unsafe_world, body, "position_y", program, "height");
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
        world.init_resource::<lunco_port_core::ports::PortTopologyRevision>();
        init_builtin_ports(&mut world);
        world.init_resource::<CosimDiagnostics>();
        world.init_resource::<lunco_core::RuntimeFaults>();
        world.insert_resource(lunco_core_session::NetworkRole::Standalone);

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
                    inputs: std::collections::HashMap::from([("height".into(), 0.0)]).into(),
                    ..default()
                },
                lunco_cosim_core::DeclaredOutputPorts {
                    names: ["netForce".into()].into(),
                },
            ))
            .id();
        wire(&mut world, body, "position_y", program, "height");
        wire(&mut world, program, "netForce", body, "force_y");

        world.run_system_once(propagate_connections).unwrap();

        let diag = world.resource::<CosimDiagnostics>();
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
        world.init_resource::<lunco_port_core::ports::PortTopologyRevision>();
        init_builtin_ports(&mut world);
        world.init_resource::<PropagationCache>();
        world.init_resource::<CosimDiagnostics>();

        let balloon = world
            .spawn((
                GlobalEntityId::from_raw(10),
                participant(
                    &["in", "bias", "force_y", "height"],
                    &["out", "netForce", "height"],
                ),
            ))
            .id();
        wire(&mut world, balloon, "netForce", balloon, "force_y");
        wire(&mut world, balloon, "height", balloon, "height");

        world.run_system_once(propagate_connections).unwrap();

        assert!(
            world.resource::<CosimDiagnostics>().faults.is_empty(),
            "self-wires on one entity are causal feedback, not missing ports"
        );
    }

    /// An acyclic chain (with a fan-in for good measure) also reports nothing.
    #[test]
    fn an_acyclic_chain_is_not_a_loop_fault() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.init_resource::<lunco_port_core::ports::PortTopologyRevision>();
        init_builtin_ports(&mut world);
        world.init_resource::<PropagationCache>();
        world.init_resource::<CosimDiagnostics>();

        let a = world
            .spawn((
                GlobalEntityId::from_raw(10),
                participant(
                    &["in", "bias", "force_y", "height"],
                    &["out", "netForce", "height"],
                ),
            ))
            .id();
        let b = world
            .spawn((
                GlobalEntityId::from_raw(20),
                participant(
                    &["in", "bias", "force_y", "height"],
                    &["out", "netForce", "height"],
                ),
            ))
            .id();
        let c = world
            .spawn((
                GlobalEntityId::from_raw(30),
                participant(
                    &["in", "bias", "force_y", "height"],
                    &["out", "netForce", "height"],
                ),
            ))
            .id();
        wire(&mut world, a, "out", b, "in");
        wire(&mut world, b, "out", c, "in");
        wire(&mut world, a, "out", c, "bias"); // diamond edge, still acyclic

        world.run_system_once(propagate_connections).unwrap();

        assert!(
            world.resource::<CosimDiagnostics>().faults.is_empty(),
            "valid causal wires do not create synthetic loop faults"
        );
    }
}
