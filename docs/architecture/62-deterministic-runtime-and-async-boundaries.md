# 62 — Deterministic Runtime and Async Boundaries

> Status: Design · Cross-domain ordering and preparation contract; the current runtime does not yet satisfy the whole-simulation guarantee · Audience: contributors changing USD, Modelica, SysML, Rhai, physics, or scene lifecycle

This document defines where work may run asynchronously and where its result
may affect the authoritative simulation. It composes the existing USD,
Modelica, SysML, Rhai, physics, time, and application-cycle owners. It defines
one execution contract across those owners without giving each subsystem its
own simulation clock or task scheduler.

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
Rendering and interpolated presentation samples are outside authoritative state.

The runtime remains event driven. It does not poll for readiness on every
simulation tick, and it does not add a second authoritative time source.

## 2. Cycles, clocks, and invocation context

A **clock** answers which time value and delta an operation observes. A
**cycle** answers when and in what order the operation runs. They are related,
but they are not interchangeable: a Bevy system set label does not create a
clock, and a clock domain does not schedule its consumers.

The runtime uses these existing cycle families:

| Cycle | Work | Clock contract |
|---|---|---|
| `Lifecycle` | scene/Twin admission and teardown | discrete boundary; no elapsed-time integration |
| `Simulation` | co-simulation, Rhai behavior, controllers, physics | fixed `SimTick`; domain time is derived from that tick |
| `Interaction` | avatar and camera interaction | wall-rooted `interaction` domain |
| `Command` / `Repl` | typed command admission and one-shot script evaluation | application/wall cadence; never advances simulation time |
| `Telemetry` | delivery of fixed-tick samples to retention and external subscribers | bounded application-frame work; each sample keeps its source tick and domain time |
| `Ui` | egui and workbench updates | host frame/input cadence |
| `Visualization` / `Presentation` | LOD selection, render preparation, visual projection | presentation cadence or an explicitly selected visual time domain |

`RuntimeCycleSet` names ordering lanes inside Bevy schedules. It does not by
itself isolate CPU cost or give `Visualization` an independent cadence. In
particular, UI work and LOD work both registered in `Update` still contend for
the same main-thread frame. Owners that need separate cadence must have an
explicit driver/clock boundary; expensive LOD selection and baking must leave
the UI-critical path, while its bounded result application stays on the owning
visualization boundary.

Application composition also selects which cycles exist. Do not install every
system in every host and rely on `run_if` checks to make unused capabilities
cheap. The GUI installs visual projections; the headless server and scene-test
host omit them. Terrain physics, analytic height queries, and collider admission
remain in the shared simulation composition, while camera-driven terrain LOD,
derived visual maps, and overlays belong to the presentation composition. A
server therefore does no terrain LOD selection or visual baking at all. Other
hosts can opt into a presentation capability when they actually render or
capture frames.

Application builders install the capabilities selected for that host, and each
feature-owned plugin registers its systems in the cycle it owns. The shared
cycle labels and ordering anchors do not install work by themselves; if a host
does not include a capability plugin, that capability contributes no systems
to its schedules. Authors select a host capability, not individual Bevy sets.

Rust owns the actual cycle boundary. Reuse Bevy's `First`/`PreUpdate`,
`FixedUpdate`, `Update`, `PostUpdate`, and the existing `InteractionSchedule`
where they already express the cadence; retain `RuntimeCycleSet` for ordering
and instrumentation metadata. Do not build a second general-purpose scheduler
or one scheduler per crate. Add a dedicated schedule/driver only when a cycle
needs an independent cadence, a real skip boundary, or a distinct overload
policy. A separate schedule still runs CPU work somewhere: UI-frame work must
stay short, and expensive visualization/analysis runs as bounded background
tasks with a small owner-boundary commit.

Telemetry reads values at their authoritative fixed tick, then delivers the
immutable samples through a bounded `Telemetry` cycle after simulation and UI
work. Its per-frame callback budget limits fan-out cost, and queue overload
drops old samples with an explicit warning without pausing physics. Scene
transitions clear the outgoing Twin's pending delivery queue and count those
samples as drops. Subscriber and history timing is therefore application-frame
timing; sample contents and source ticks remain simulation facts. Throughput
and frame-time acceptance for heavy telemetry loads remain a measured open item.

Each Rust cycle boundary supplies the same typed execution context to its
systems and synchronous callees: owner scope/generation, cycle/phase, selected
clock sample, and logical sequence. Rhai reads that context from its caller;
it cannot choose a different clock for a nested function or hook. Bevy schedules
remain statically composed by owner crates, so an absent capability is absent
from the host schedule and an installed cycle has a visible owner.

Rhai scheduling metadata is admission data, not a request to mutate the Rust
scheduler. A script with an unknown peer scope or unsupported timing is skipped
and receives a document diagnostic for that source revision; the host and its
other scripts continue. The owner must not guess a scope or move the script to
another clock. Runtime callback errors also remain visible and local to their
owner, with required authoritative hooks holding or faulting that owner.

Every cycle exposes low-cost aggregate duration, work admitted/completed,
queue/in-flight depth, and deadline/overload counts through the existing
diagnostics boundary. Counters are updated at cycle/task boundaries rather than
collecting full per-system payloads or emitting a telemetry sample every frame.
Use those metrics to find the owner, then profile the production path with the
adjacent Tracy workflow. Run an unprofiled pass for frame-rate acceptance; the
profiler capture is diagnostic evidence, not the performance number.

Every cross-domain callback receives an immutable `RuntimeExecutionContext`
from its owning Rust cycle with:

- owner scope and generation;
- cycle and phase;
- the selected clock identity and resolved `{ time, delta }` sample;
- a logical sequence (`SimTick` for simulation, an owner sequence for
  application work, or none at a discrete lifecycle boundary);
- the producer stamp for event-driven calls.

Systems declare their cycle and read that cycle's clock. A synchronous helper,
Rhai function, or nested registered hook inherits the caller's context. An
event retains its producer stamp while its consumer also knows its execution
cycle. A discrete callback must not infer its timing from whichever Bevy
`Time<T>` happens to be accessible. Rust owns this typed contract; Rhai gets a
read-only `execution_context()` view beside the existing `clock_snapshot()`. The
Rhai map preserves route generations and sequences as native `u64` values.
An unclassified bridge call has no owner route; its `scope`, `cycle`, and
`generation` map values are Rhai unit instead of an invented Application route.
`sim_tick()`, `dt()`, and `elapsed_seconds()` return a Rhai error outside
simulation execution, stopping only that invocation. Missing mandatory
simulation clock resources still raise the existing scene runtime fault after
the cycle check.

The scenario owner currently supplies this context for fixed scenario passes,
paused lifecycle/event passes, and one-shot REPL/tool evaluations. Fixed passes
use the simulation tick and fixed-step clock; paused passes have no elapsed-time
clock or consumer sequence and preserve the telemetry event's simulation
producer route and tick. Event RNG uses that producer sequence even when its
callback runs in a later cycle; discrete lifecycle hooks use a stable
sequence-free seed.
Nested Rhai functions inherit the current phase. Peer selection uses the
scenario's `@scope host|client|both` directive. Continuous scenario behavior may
state its required cadence with `@timing simulation`; Rust still installs and
invokes the scenario from its owning schedules. Lifecycle hooks receive the
lifecycle or simulation context of the pass that invokes them. Unknown scope or
timing values skip only that scenario and publish document diagnostics once
for the source revision. The scenario driver refreshes these directives from the
current document generation, so a source edit cannot keep stale routing state.

Host setup installs cycle owners automatically from the selected application
composition/features; scenario authors do not manually assemble Bevy schedules.
Rhai metadata validates one scenario's declared policy; it never installs a
cycle, guesses another clock, panics, or stops unrelated cycles. Script runtime
errors remain diagnostics at the owning invocation and are not swallowed. A
missing or failed optional hook remains isolated and visible; a required
authoritative hook holds or faults its owner through the runtime-fault contract.

## 3. One authoritative simulation transaction

`lunco-core-runtime` owns the fixed-step tick, the reason-keyed
`SimulationProgress` admission gate, and the generic per-step simulation
barrier. Scene lifecycle holds carry a monotonic `SceneTransitionId`; only the
matching terminal edge releases its hold. The admission gate covers work that
changes which authoritative scene can run. Ordinary parsing, analysis, editor
work, and optional LOD refinement do not acquire this gate. Their selected
owner plugins control their cadence; GUI LOD still shares the `Update` schedule
with UI until its heavy selection work moves to a bounded worker path (D16).
An active reference spawn on the mounted primary `UsdSceneRoot` acquires a
`SceneReferences` operation key when the typed structural change is admitted.
Its hold follows the prepared closure through live-stage authoring and ECS root
projection. Preview and additive document references keep their own projection
lifecycle and diagnostics; they do not hold or fault the primary simulation.
An inactive or removed primary root releases its exact key until that operation
has faulted. A primary closure or projection failure records a `RuntimeFault`,
a path-addressed diagnostic, and a persistent progress hold; scene teardown
clears both before a replacement scene runs. `UsdSceneRuntimePlugin` installs
the progress resource it needs, so selecting that capability is sufficient.
The causal transaction follows explicit owner phases:

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
6. Run stateful Rhai hooks against the committed tick snapshot. Until their
   reads are immutable and their writes are typed intents, execute them serially
   by stable actor identity. Merge any intents in that same stable order at a
   named boundary.
7. Integrate physics at the fixed schedule boundary with an admitted solver
   profile, then close the tick. `SimTick` advances only when the transaction
   is allowed to progress.

The exact existing phase anchors and FMI-style exchange are documented in
[`22-domain-cosim.md`](22-domain-cosim.md). This document adds the rule that
asynchronous completion never selects the visible simulation tick.

## 4. Async preparation, priority, and result commit

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

Independent preparation is allowed to run in parallel on the existing worker
pools. Submission uses one shared bounded admission policy with three semantic
priorities: `SimulationRequired` (only work that gates a declared simulation
boundary), `Interactive` (work for the visible/active Twin or viewport), and
`Background` (prefetch, inactive documents, and optional analysis). Stable FIFO
identity orders requests within a class; aging or a reserved background share
prevents starvation. A priority changes which queued job starts first; it never
changes simulation order, event order, or which result is valid. Domain owners
keep their typed task handles, payloads, and result validation. The shared
policy only admits bounded work onto the existing pool and reports queue/in-
flight status.

Results are committed only by their owner at a named cycle boundary. Results
for presentation may be adopted when current and useful. Results that change
authoritative topology or inputs carry a declared target simulation boundary;
that boundary waits for the complete required set, validates every revision,
and commits in stable owner/identity order. Completion time and worker arrival
order never choose a tick. Unrelated work never joins that admission hold.

Parsing and compilation should move off the UI and fixed schedules when their
inputs can be captured immutably. A script's top-level body is executable
world behavior, not pure compilation, and remains serialized at activation.
Likewise, a live OpenUSD stage is thread-affine; moving immutable source
preparation off-thread does not mean sharing the stage object with a worker.

## 5. Rhai execution and safe parallelism

Rhai source parsing, import resolution, and immutable compilation artifacts may
be prepared asynchronously. Scenario `this` state, top-level initialization,
and live-world calls remain owned by the script activation/execution boundary.
All functions and hooks inherit the invocation context; the scenario owner
assigns each callback to its Rust-owned cycle. Event handlers read both the
event's origin stamp and the consumer's current cycle. Source metadata may
validate an author's required cadence, but it does not install or move a hook.

Current Rhai world verbs can read and mutate live ECS/port state, and `cmd()`
effects are visible to later actors in the same pass. Their existing stable
actor order is therefore a behavior contract. Parallel scenario execution is
safe only after the owner supplies an immutable tick snapshot, isolates each
actor's persistent state, buffers typed commands/events per actor, and applies
those buffers in stable identity and per-actor sequence order. Until that
boundary exists, stateful hooks stay serial. Pure preparation and truly
independent calculations can already run concurrently.

Registered Rust/Rhai hooks use the same rule: the owning system supplies the
execution context and deterministic order. A hook implementation does not
select a clock or schedule itself. Hook contracts that affect authoritative
state declare their owner, inputs, output/action plan, install scope, and
failure behavior as required by the hook review.

## 6. Co-simulation and dependency closure

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

The composed dependency graph also defines safe parallelism. Participants in
the same dependency layer may calculate concurrently from one immutable input
snapshot. A dependent layer starts only after its predecessor values are
committed for the same communication point. The master collects all results
required by authoritative sinks, validates participant/session/source/step
identity, then publishes in stable participant order. Independent participants
may retain an explicit last validated output while the world advances. Each
Modelica local solver clock is subordinate to its declared communication point
and target `SimTick`; worker wall time never advances it into live state.

Physics remains a deterministic fixed-step owner. Background worker admission
must leave measured CPU capacity for the render and simulation threads; parallel
solver operations are enabled only under a measured deterministic reduction
contract. The first safe optimization is parallel preparation and independent
co-simulation layers, not concurrent writes into the live ECS world.

Scene readiness follows the same rule. Initial composition must establish one
simulation start boundary: asynchronous load duration must not consume
authoritative ticks. Runtime referenced assets and other new participants must
be included in the dependency/readiness closure before they enter the live
stage. A revision that changes future simulation topology is admitted at a
declared boundary; worker completion itself is not that boundary.

## 7. Telemetry and observation work

Telemetry has two execution contracts. Discrete mission events that Rhai or
another authoritative consumer uses retain their producer tick and stable
producer/order key, then run in the consuming deterministic simulation phase.
Logging, formatting, API fan-out, serialization, and persistence must not run
inside the physics transaction; they consume an independent observation copy.
Missing required events or queue overflow is a structured fault, never silent
loss or a reason to block the physics step on I/O.

Continuous `SampledParameter` records are observations of committed state, not
simulation inputs. Their value and `SimTick` stamp are captured at the declared
sample boundary so a sample cannot combine values from different ticks. Capture
uses the authored channel rates and clock bindings, walks the cached channel
plan for due checks and live reads, then queues a small typed record for bounded
post-simulation delivery and retention. Reducing that fixed-path walk and live
read cost remains open. API subscriptions, display decimation, log formatting,
recording encoders, and file/network I/O run after capture on their owning
application or background cycle. Live display may report dropped observations;
a configured lossless recording must instead fault explicitly if its bounded
queue cannot keep up.

Telemetry collection is bounded by the configured channel cap and per-channel
rate. It must not enumerate unrelated diagnostics, rescan port backends, or
rebuild UI summaries on each physics tick. UI plots consume the retained
`SignalRegistry` history by revision and derive decimated points at visualization
cadence. The shared causal event order remains authoritative even when optional
telemetry consumers are delayed.

## 8. Stable ordering and numeric scope

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

## 9. Current evidence and open gaps

The runtime already has a fixed co-simulation phase graph, generation/session
fences for Modelica work, async USD asset/projection paths, a Modelica worker,
revision-gated document handling, and explicit readiness/coupling barriers.
Scenario order, same-tick event delivery, connected-event emission, and
Modelica command submission are now canonicalized at their boundaries. The
scene lifecycle holds `SimulationProgress` from transition start through its
authoritative asset and visual-projection terminal edge. The gate exposes its
owner and operation key plus a user-facing wait reason; matching failure and
completion edges release only their own operation. `TimeTransport` mode and
rate remain the user's intent while this gate pauses `Time<Virtual>`.

The whole-simulation guarantee remains open because:

1. Readiness and scene lifecycle do not yet share one complete admission
   transaction through reference closure, Modelica preparation, physics
   admission, and the first communication point. The lifecycle hold currently
   ends at the asset/visual-projection terminal edge.
2. Runtime referenced assets can be projected as soon as their async load
   completes, and their pending dependency closure is not fully represented by
   readiness.
3. Dynamic Rhai port access is not represented in the Modelica causal graph.
4. Some heavy preparation remains synchronous: Rhai cache-miss compilation and
   module evaluation, SysML analysis/source-set discovery, and initial USD
   document parse/overlay serialization. Native Modelica source interfaces are
   extracted once on Bevy's async-compute pool while the source asset loads;
   co-simulation, member discovery, and the web workbench reuse that
   revision-matched interface. Bevy's wasm task pool runs on the browser main
   thread, so the web loader still needs a Modelica Web Worker handoff for a
   fully non-blocking parse.
5. The production GUI does not pin Avian's compute pool; `PhysicsDeterminism`
   correctly reports that configuration as nondeterministic.
6. The command journal does not yet provide a whole-simulation authoritative
   input log and replay verdict, and adaptive Modelica is not a cross-machine
   bitwise deterministic solver.
7. `RuntimeCycleSet` is ordering vocabulary rather than an independent cadence
   driver. Typed context now reaches scenario preparation/start/event/behavior/
   stop calls and one-shot Rhai evaluation, but other hook owners still need
   adoption. GUI UI and LOD still share the main `Update` schedule, although
   server hosts now omit the visual plugin entirely.
8. Async task admission and priority are local to individual owners. The shared
   Bevy pool can be saturated by background work, and completion commits are
   not yet governed by one cross-owner budget/order contract.
9. Rhai hooks and co-simulation still have live-world access paths that prevent
   safe parallel evaluation even where the dependency graph contains
   independent actors.
10. Telemetry sampling still walks its cached channel list for due checks in
    the fixed simulation cycle. Subscriber callbacks now run in a bounded
    post-simulation telemetry cycle, but capture cost and end-to-end observer
    throughput have not been measured against physics.
11. Cycle duration, queue pressure, and overload counters are not yet exposed
    together at the diagnostics boundary, so optimization cannot target an
    owner using comparable cycle evidence.

These findings and their owner-specific file evidence are maintained in
[`../reviews/open-deterministic-simulation-contract.md`](../reviews/open-deterministic-simulation-contract.md).

## 10. Migration order

1. **Canonical boundary order.** Sort current scenario, event, and serialized
   Modelica work by stable keys; add production Rhai verdicts for observable
   event ordering. Keep source-owned identities and fail visibly when a
   required identity is invalid.
2. **Cycle and clock context.** Extend the typed invocation context from
   scenario and REPL owners to every callback owner, then expose it read-only to
   Rhai. Give UI and visualization independent cadences while keeping both
   presentation-only.
3. **Async work admission.** Add shared bounded priority admission over the
   existing worker pools. Move immutable Rhai, SysML, USD, and Modelica
   preparation to workers; preserve owner-specific typed results and commits.
4. **Telemetry observation boundary.** Keep authoritative event delivery in
   stable simulation order; make continuous sampling due-driven and bounded,
   then move logging, fan-out, encoding, and persistence to the observation
   cycle with explicit overload reporting.
5. **Complete dependency closure.** Replace or formally declare dynamic script
   port access so every state dependency reaches the causal graph and every
   script write enters the typed deterministic action path.
6. **Parallel actor execution.** Buffer Rhai actor effects and co-simulation
   participant results, then merge by stable identity at explicit boundaries.
   Keep any actor serial while it has immediate live-world dependencies.
7. **Complete admission and physics profile.** Hold the first simulation
   boundary through scene references, required Modelica preparation, and physics
   readiness. Measure the serial and any deterministic parallel solver profile
   before selecting the production default.
8. **Replay evidence.** Record admitted inputs and deterministic result keys;
   add a production scene suite spanning USD projection, Modelica coupling,
   Rhai events, SysML revisioned verification, and Avian state. Compare state at
   tick boundaries, not wall-clock completion times.
9. **Performance validation.** Profile one settled production scene and one
   cold-start scene. Verify worker priority, UI frame cost, and simulation
   throughput together; async work must improve responsiveness without hiding
   solver cost.

The simulator can claim whole-simulation determinism only when these phases
close with production evidence. Current stable ordering is a necessary first
phase, not that final claim.
