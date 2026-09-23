# 62 — Deterministic Runtime and Async Boundaries

> Status: Design · Cross-domain ordering and preparation contract; the current runtime does not yet satisfy the whole-simulation guarantee · Audience: contributors changing USD, Modelica, SysML, Rhai, physics, or scene lifecycle

This document defines where work may run asynchronously and where its result
may affect the authoritative simulation. It composes the existing USD,
Modelica, SysML, Rhai, physics, and time owners; it does not add a second
runtime scheduler.

## 1. The guarantee

For a given executable build, admitted scene revision, initial state, and
ordered authoritative input stream, each simulation tick must apply the same
state transitions in the same order. Wall-clock worker completion, ECS
archetype layout, hash-map iteration, render cadence, and UI timing must not
choose which values an authoritative hook or solver sees at a tick boundary.

This is an ordering and admission guarantee. It is not a promise of bitwise
identical floating-point physics or adaptive Modelica results across different
architectures, compiler versions, or solver profiles. Those numerical promises
need an explicit deterministic profile and their own cross-platform evidence.
Rendering and detached presentation clocks are outside authoritative state.

The runtime remains event driven. It does not poll for readiness on every
simulation tick, and it does not add a second per-frame clock.

## 2. One transaction owner

`lunco-core-runtime` owns the fixed-step tick and the generic simulation
barrier. Existing domain schedules remain the only execution graph:

1. Capture external commands and events as typed inputs with their authoritative
   tick and stable producer/order key.
2. Apply admitted lifecycle and authored changes at a simulation boundary.
   Prepared work is checked against its source revision and stale results are
   discarded.
3. Read the committed state snapshot for the tick. Sync Modelica and script
   inputs, propagate declared `SimConnection`s in stable connection order, and
   apply stateful backend inputs.
4. Dispatch off-thread participant work with a stable participant key and
   monotonically increasing step identity. A worker result is data; it does not
   commit itself into the live world.
5. Wait only for participants in the declared causal closure of authoritative
   state. Collect the complete required result set, validate session, source
   revision, step, and target tick, then publish results in stable identity
   order. Independent outputs remain explicit zero-order holds and cannot
   silently become same-tick state inputs.
6. Run world-mutating Rhai hooks serially by stable actor identity against the
   committed tick snapshot. Their typed commands and events enter the ordered
   input stream for a named later boundary.
7. Integrate physics at the fixed schedule boundary with an admitted solver
   profile, then close the tick. `SimTick` advances only when the transaction
   is allowed to progress.

The exact existing phase anchors and FMI-style exchange are documented in
[`22-domain-cosim.md`](22-domain-cosim.md). This document adds the rule that
asynchronous completion never selects the visible simulation tick.

## 3. Async preparation versus authoritative execution

| Domain/work | May run async | Must run at the owner boundary |
|---|---|---|
| USD | Asset I/O, dependency discovery, immutable layer parsing/composition, and send-safe projection-plan preparation | Check source generation; mutate the live, thread-affine stage and publish ECS projection in stable scene order |
| Modelica | Source I/O, declaration/interface extraction, parsing/lowering, solver construction, and requested numerical step | Check model generation/session/step; publish outputs and propagate ports at the fixed co-simulation boundary |
| SysML | Source-set I/O, parse, resolve, typed analysis, and requirement report preparation | Publish only the current source revision; verification that reads live simulation values consumes the committed tick snapshot |
| Rhai | Parse, import resolution, AST lowering, and immutable compile-artifact construction | Evaluate top-level initialization and lifecycle hooks against the live world in stable actor order; apply commands at their declared boundary |
| Physics | Preparation that does not read or mutate live solver state | Kinematics, contact solving, integration, and authoritative writes remain within the fixed physics schedule |
| Rendering and UI | Mesh/shader preparation, presentation, editor analysis, and persistence I/O | Read committed simulation state; presentation completion cannot advance or release authoritative time |

An async result carries the identity of the work that produced it: at minimum
Twin/scene generation, owner identity, source revision, and operation/step id.
The receiver rejects stale or out-of-order results at the owner. It does not
retry by polling, substitute a default, or infer a new identity from completion
time.

Parsing and compilation should move off the UI and fixed schedules when their
inputs can be captured immutably. A script's top-level body is executable
world behavior, not pure compilation, and remains serialized at activation.
Likewise, a live OpenUSD stage is thread-affine; moving immutable source
preparation off-thread does not mean sharing the stage object with a worker.

## 4. Dependencies are part of the schedule

The causal barrier is only sound when it contains every path by which a
participant result can affect authoritative state. `SimConnection` is the
declared data plane. Dynamic Rhai `get`/`port` reads and `set`/`port_set` writes
currently bypass that graph. Before whole-simulation determinism is claimed,
those accesses must either:

- declare typed dependencies that feed the same causal graph, or
- use a tick snapshot/action plan whose admitted reads and writes are explicit.

Choosing one of these policies belongs to Rhai and co-simulation owners. Rust
provides the typed snapshot, dependency fact, barrier, and validated application
mechanisms. Continuous calculations and physics remain in their domain owners.

Scene readiness follows the same rule. Initial composition must establish one
simulation start boundary: asynchronous load duration must not consume
authoritative ticks. Runtime referenced assets and other new participants must
be included in the dependency/readiness closure before they enter the live
stage. A revision that changes future simulation topology is admitted at a
declared boundary; worker completion itself is not that boundary.

## 5. Stable ordering and numeric scope

All observable batches use a total key owned by their domain. Current runtime
ordering includes:

- scenario actors: `GlobalEntityId`, then the world-local Bevy entity key for
  local-only hosts;
- telemetry delivery: simulation tick, source, name, severity, time, and a
  recursive order over the typed payload;
- USD-connected events: instance namespace, authored event prim path, source,
  and event name;
- serialized Modelica commands: `GlobalEntityId`, then the world-local entity
  key for local-only models;
- connection reductions: stable connection identity before floating-point
  accumulation.

The world-local entity key makes execution independent of ECS query layout in
one running world. It does not provide cross-session replay identity. Any actor
or model included in a cross-peer/replay guarantee needs its stable
`GlobalEntityId` or another source-owned, replicated identity.

Floating-point addition is order dependent. Every reduction that contributes
to authoritative state needs a stable input order. Parallel physics is
admissible only if the solver owner demonstrates an order-independent numeric
result for the selected operation or uses a deterministic reduction. A fixed
time step by itself is insufficient.

## 6. Current evidence and open gaps

The runtime already has a fixed co-simulation phase graph, generation/session
fences for Modelica work, async USD asset/projection paths, a Modelica worker,
revision-gated document handling, and explicit readiness/coupling barriers.
Scenario order, same-tick event delivery, connected-event emission, and
Modelica command submission are now canonicalized at their boundaries.

The whole-simulation guarantee remains open because:

1. Readiness holds currently freeze physics/scenario owners while
   `Time<Virtual>` and `SimTick` may continue; asynchronous initial load can
   therefore select the first live tick.
2. Runtime referenced assets can be projected as soon as their async load
   completes, and their pending dependency closure is not fully represented by
   readiness.
3. Dynamic Rhai port access is not represented in the Modelica causal graph.
4. Some heavy preparation remains synchronous: Rhai cache-miss compilation and
   module evaluation, SysML analysis/source-set discovery, initial USD document
   parse/overlay serialization, and USD Modelica interface extraction.
5. The production GUI does not pin Avian's compute pool; `PhysicsDeterminism`
   correctly reports that configuration as nondeterministic.
6. The command journal does not yet provide a whole-simulation authoritative
   input log and replay verdict, and adaptive Modelica is not a cross-machine
   bitwise deterministic solver.

These findings and their owner-specific file evidence are maintained in
[`../reviews/open-deterministic-simulation-contract.md`](../reviews/open-deterministic-simulation-contract.md).

## 7. Migration order

1. **Canonical boundary order.** Sort current scenario, event, and serialized
   Modelica work by stable keys; add production Rhai verdicts for observable
   event ordering. Keep source-owned identities and fail visibly when a
   required identity is invalid.
2. **Async immutable preparation.** Move Rhai AST/import preparation, SysML
   source analysis, initial USD document parsing/serialization, and Modelica
   declaration extraction to revision-stamped workers. Keep live-world
   initialization and live OpenUSD mutation on their owner schedules.
3. **Admission clock.** Add one reason-keyed simulation-progress barrier that
   includes scene readiness, referenced dependencies, causal workers, and
   revision admission. Start/restart at an explicit simulation boundary and
   ensure completion latency does not change `SimTick` or authored activation.
4. **Complete dependency closure.** Replace or formally declare dynamic script
   port access so every state dependency reaches the causal graph and every
   script write enters the typed deterministic action path.
5. **Physics profile.** Decide whether production uses a serial deterministic
   solver profile or deterministic parallel operations. Measure both throughput
   and replay state before selecting a default; report numerical scope by
   solver/build/platform.
6. **Replay evidence.** Record admitted inputs and deterministic result keys;
   add a production scene suite spanning USD projection, Modelica coupling,
   Rhai events, SysML revisioned verification, and Avian state. Compare state at
   tick boundaries, not wall-clock completion times.
7. **Performance validation.** After ordering and churn are stable, profile one
   settled production scene. Async preparation should reduce UI/fixed-schedule
   stalls while the ordered simulation boundary preserves correctness.

The simulator can claim whole-simulation determinism only when these phases
close with production evidence. Current stable ordering is a necessary first
phase, not that final claim.
