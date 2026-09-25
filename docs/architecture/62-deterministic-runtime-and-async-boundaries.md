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
| `IdentityAdmission` | assign stable identities to entities created by lifecycle projection | discrete boundary; ordered after lifecycle projection |
| `EntityIndex` | publish API/path lookup indexes for admitted identities | discrete boundary; ordered after identity admission and before time advancement |
| `Simulation` | co-simulation, Rhai behavior, controllers, physics | fixed `SimTick`; domain time is derived from that tick |
| `Interaction` | avatar and camera interaction | wall-rooted `interaction` domain |
| `Command` / `Repl` | typed command admission and one-shot script evaluation | application/wall cadence; never advances simulation time |
| `Telemetry` | delivery of fixed-tick samples to retention and external subscribers | bounded application-frame work; each sample keeps its source tick and domain time |
| `Ui` | egui and workbench updates | host frame/input cadence |
| `Visualization` / `Presentation` | LOD selection, render preparation, visual projection | presentation cadence or an explicitly selected visual time domain; terrain cover reselection is capped at 30 Hz using `Time<Real>` |

`RuntimeCycleSet` names ordering lanes inside Bevy schedules. It does not by
itself isolate CPU cost or give `Visualization` an independent cadence. Terrain
cover reselection has an explicit 30 Hz wall-clock skip boundary, and its
immutable calculation uses shared bounded background admission. The owning
system checks the camera, surface, and operation signatures before committing a
result. Tile-mesh CPU bakes still use the per-terrain task queue; mesh upload,
visibility, and residency commit in `Update`. This removes the quadtree cover
walk from the UI frame, while tile-bake admission and per-frame ECS work still
share that frame.

The pre-simulation `PreUpdate` order is `Lifecycle` → `IdentityAdmission` →
`EntityIndex` → `TimeSpineSet`. Lifecycle projection creates the ECS entities;
the identity owner assigns their stable IDs; then the API registry publishes
path lookups for those identities. Only after those steps can the time spine
release the next fixed tick. This makes newly admitted referenced entities
visible through `find_path` on the first resumed tick, without a startup-only
route or a second scene-ready signal. The production `route_lifecycle` Rhai
gate exercises reference admission and verifies that first-tick observation.
The owner-level `drain_ref_spawns` test makes a later reference ready before its
authored predecessor, confirms neither prim is projected while the prefix is
incomplete, and checks both live-stage commits follow authored order after the
predecessor becomes ready.

Fixed-step time is not a wall-clock service guarantee. In the production GUI,
Bevy drains `FixedMain` synchronously before `Update`; LunCoSim's rate-scaled
delta guard permits up to 64 fixed steps in one app update at the highest
transport rate. Every step still receives the same `Time<Fixed>` delta, but a
long tick or catch-up burst delays UI/input work, and the raw-delta cap means
simulation time can fall behind wall time under sustained overload. Reducing
the step cap by discarding accumulated time would hide that lag by dropping
authoritative ticks, not fix it. Until ownership is split, report tick backlog
and deadline misses and describe this as bounded catch-up, not guaranteed
real-time cadence.

Hard UI responsiveness and wall-clock physics cadence require one dedicated
simulation owner that runs the **whole causal tick** in its own `App`/`World`:
co-simulation, Rhai hooks, event barriers, physics, and derived authoritative
state. The window app sends typed, sequence-stamped commands for a target
`SimTick` and reads immutable, tick-stamped snapshots without waiting on the
simulation owner. Rendering consumes the newest snapshots and interpolates
presentation; command acknowledgements and telemetry return through bounded,
nonblocking channels. Do not move only Avian to another thread while the
remaining authoritative systems mutate or query the same `World`. This is a
cross-crate ownership migration, not a new `RuntimeCycleSet` or a second
per-feature scheduler.

In that model, `Time<Fixed>` remains the constant integration delta. A
real-time pacing policy schedules ticks against a monotonic deadline; transport
rate changes the wall interval between fixed ticks, not their delta. If a
deadline is missed, keep the tick and report lag rather than silently skipping
it. Offline recording and deterministic tests use an explicit unpaced driver
that advances the same fixed ticks on demand. Neither policy blocks the UI
thread while waiting for simulation or visualization work.

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

The scene transaction coordinator owns the shared Twin scene generation. It
advances only after the matching active transition succeeds and emits
`SceneTransitionCommitted`; a failure or stale completion leaves the previous
generation in force. Scenario lifecycle reads this value for compile fences
and execution routes. Other Twin-scoped cycle owners use the same source instead
of keeping private scene counters. Before a successful scene commit, there is
no Twin execution route. Scenario drivers idle while the readiness gate is
closed and when their language has no attached scenarios; a missing generation
faults only when live scenario work needs a Twin route.

Systems declare their cycle and read that cycle's clock. A synchronous helper,
Rhai function, or nested registered hook inherits the caller's context. An
event retains its producer stamp while its consumer also knows its execution
cycle. A discrete callback must not infer its timing from whichever Bevy
`Time<T>` happens to be accessible. `lunco-runtime-context` owns these typed
values, while `lunco-core` supplies the Bevy `RuntimeCycleSet` labels. Rhai gets
a read-only `execution_context()` view beside the existing `clock_snapshot()`.
Registered hooks receive one typed `HookInvocation`; isolated Rhai hooks expose
its context as an immutable `runtime_context` map, and native providers receive
the same map through ABI v2. A Rust hook owner with no classified cycle uses
`invoke_unclassified`; scheduled owners supply `invoke_with_context` from their
cycle inputs. The scheduled `readiness.action` policy is classified as
`Core/Simulation/Behavior`, with `Time<Fixed>` elapsed time and the latest
completed `SimTick`; its Rhai policy rejects calls from other cycles. The
post-solver `physics.body_escape` decision is classified as
the core simulation `Behavior` phase and stamped with `SimTick` plus the fixed
clock sample. Its Rhai policy rejects invocation outside that contract. The
Rhai map preserves route generations and sequences as native `u64` values.
Physics initialization is a separate discrete lifecycle call. It uses the
active scene transaction id while a new stage is being admitted and the last
committed scene id afterward, with `Twin/Lifecycle`, `Preparation`, and no
clock, delta, or sequence. Its selector is data for one declared Rhai seam,
not a dynamically constructed hook id. Stable USD paths and f64 poses cross
that boundary; process-local ECS ids do not.
Generated Modelica source synthesis uses the same active-or-committed Twin
generation in `Twin/Lifecycle/Preparation`, with no elapsed clock. The owner
captures this context before dispatching synthesis to the async worker, so both
the async startup path and synchronous live projection invoke the same policy
contract. Both shipped synthesis policies reject calls from scenario, UI, or
REPL cycles. Async results must still match that Twin generation as well as the
canonical USD generation or exact prepared instance plan before publication.
The settled USD scene-time owner uses the completed edge's `SceneTransitionId`
for `scene.time.select` in the same `Twin/Lifecycle/Preparation` context. The
typed selection carries that id through its deferred application; the time
owner ignores stale selection and terminal edges after a replacement begins.
Explicit policy inspection from `RunRhai` is separately stamped
`Application/Repl/Evaluation` and does not apply a scene-time decision.
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
matching terminal edge releases its hold. `SimulationProgress` aggregates
owner-local operations that must commit before authoritative virtual time can
advance; it is not a global readiness bit for every clock or subsystem.
Ordinary parsing, analysis, editor work, optional LOD refinement, and
physics-only readiness use their own owner boundary. Their selected owner
plugins control their cadence. GUI terrain cover selection runs at a 30 Hz
`Time<Real>` cadence; immutable cover preparation uses the shared background
queue and current results commit in `Update`. Tile-mesh bake completion and
residency also commit in `Update`. Offline capture bypasses the cadence and
prepares covers synchronously at each captured frame.
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
The Modelica execution owner also reconciles active, causally required
participants before the time spine. A participant whose current session still
needs compilation owns one `ModelicaPreparation` key; compile intent is admitted
in the lifecycle cycle, which continues while `Time<Virtual>` is held. The
worker result is committed by the Modelica response handler before the next
lifecycle pass releases that key. Intentionally paused and noncausal models do
not hold world time. This gate covers prepared solver state; the first normal
co-simulation step remains governed by the per-step barrier after activation.
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
| Rhai | Parse file-backed `.rhai` assets in Bevy's async asset-loading tasks; prepare inline roots and immutable compile artifacts through shared admission | Publish canonical source/AST revisions; validate and commit the dependency closure; evaluate imported module bodies, top-level initialization, and lifecycle hooks against the live world in stable actor order; apply commands at their declared boundary |
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
`Background` (prefetch, inactive documents, and optional analysis). Stable
operation identity orders requests within a class; aging or a reserved
background share prevents starvation. A priority changes which queued job
starts first; it never changes simulation order, event order, or which result
is valid. Domain owners keep their typed task handles, payloads, and result
validation. An owner may cancel a request while it is still queued. Once a
worker has started it, work that owns a simulation progress hold keeps that
exact hold until a current result commits or the operation is explicitly
retired; a stale completion cannot release a newer operation's hold. The
shared policy only admits bounded work onto the existing pool and reports
queue, in-flight, rejection, and queued cancellation counts.

Native Bevy hosts install `AsyncWorkAdmission` from `lunco-core-runtime`. Each
request carries a stable key containing scope generation, owner identity,
source revision, and operation id. The resource bounds queued and admitted
work, applies the three priorities with reserved interactive/background
service, and exposes aggregate queue counters. It can withdraw queued work but
does not preempt a running task. Owners still validate and commit their typed
results at their own boundary. Modelica document parsing, inline Rhai roots and
source-matched import misses, Twin SysML source-set analysis, and default USD
Twin source parsing plus persistent-source serialization use this path. USD
source parse results are checked against the current asset text before the
registry applies path identity and dirty-document policy. Persistent snapshots
are cloned after runtime-overlay restoration and serialized away from the main
schedule; the owner commits the Twin overlay only if the document generation
still matches. Runtime sidecar reads/parsing remain synchronous, and this USD
path reports the missing worker transport on wasm rather than blocking the page.
File-backed Rhai assets are parsed and const-folded in the asynchronous Bevy
asset loader; the source asset publishes its canonical id, exact text, AST, and
literal import dependencies together. The owner qualifies the default Bevy
source path as `lunco://` for imports and the prepared-AST cache; `twin://`
identities retain their source scheme. Activation owners commit the complete
loaded dependency closure before binding or starting a source; later asset
events publish revisions and hot reload. Startup and Twin tools plus the prelude
consume that AST, while the admitted scenario worker reuses it when the source
matches and parses only inline roots or an asset not yet committed at its owner
boundary.
Native live Modelica source-root commands use the worker's bounded preparation
pool to read source bytes, extract bound-input defaults, and parse each source
set without touching the Rumoca session. Files are sorted by URI before
preparation. A dedicated Rumoca actor owns the mutable compiler session and
shared DAE cache. Source-root installation and ordinary `Compile` requests use
one FIFO mailbox; the worker commits actor results in submission order and
fences compile artifacts by entity session and library generation. This keeps
Rumoca's stateful work off the Modelica command owner while preserving one
compiler session and deterministic source-root-before-dependent-compile order.
Immutable DAE lowering and persistent solve-cache reads, decoding, encoding,
and writes run on the bounded solve-preparation pool. The command owner only
commits the ready solve model. Actor admission is bounded across submitted
compiles, root installs, and lowerings.
Parameter updates, resets, and cache-invalidating Step auto-init are
continuations on that same FIFO. The command owner keeps servicing other
entities while Rumoca compiles and the bounded pool lowers immutable DAE data;
the original Step resumes only after the matching session and library
generation commit. Step-triggered rebuilds share the bounded admission count,
so a scene with many models cannot overfill the pipeline. Root discovery
enumerates bundled filenames without loading their text; a load reads only the
selected flat model or package. For wasm, the host reads storage-backed roots
and sends text to the Modelica Web Worker, where preparation and installation
run; that storage read remains synchronous at the browser host boundary.
First-compile intent is admitted by `request_modelica_compiles` in
`ModelicaSet::AdmitCompileRequests`, inside the application lifecycle cycle.
`ModelicaExecutionPlugin` consumes the typed `CompileRequested` intent and the
worker owner resolves the current document snapshot and dispatches the
compiler command without UI resources; the UI command only resolves the
selected class and publishes intent. Solver stepping remains in
`spawn_modelica_requests` inside `FixedUpdate`. Because this request is emitted
only for an unpaused model, it carries resume intent through compilation so a
successful first compile does not leave the model paused. The compile request
can therefore be dispatched while `Time<Virtual>` is held. Compile-result
commit validates both worker session and captured document generation. If the
document changed during compilation, the old result is discarded and an active
model remains held with its run intent for the current revision. The scene
admission hold still needs to include reference closure, Modelica preparation,
Rhai activation, and physics readiness in one transaction.

Generated domain projection follows the same path: it publishes the validated
source and interface, the document owner links a generated Modelica document,
and the next lifecycle admission emits `CompileRequested`. Projection does not
send a worker command before the model has a linked document. This preserves
the compile dispatcher's source-generation and session fences and makes the
generated participant's readiness state visible to lifecycle admission.
Twin SysML source-set analysis is read-only preparation for the active Twin, so
it uses `Interactive` priority and does not acquire `SimulationProgress`.
`AnalyzeSysml` reports `Pending` until the current snapshot is committed. A
simulation scenario promotes that work to a startup prerequisite only when its
`simulation_dependencies` plan names the SysML analysis input. The generic
`SimulationDependencyStates` owner registry publishes Pending, Ready, and
Failed edges; the scenario keeps its existing `ScriptPreparation` hold through
that dependency and resumes when an owner state revision changes. The wait
reason includes the missing input. Twin analysis by itself never holds the
whole-world clock.
`SceneValidationPlugin` is the composition owner for the optional SysML
runtime plugin, so every host installs that integration once through the same
feature path.
Twin lifecycle policy selects the checked manifest source set and requests one
preparation; Bevy loads its `twin://` assets, the worker parses and resolves the
complete immutable snapshot, and the SysML owner commits only the current
Twin-id/root/operation. Runtime `AnalyzeSysml` and `ValidateSysml` queries read
that committed snapshot and report pending, failed, or unprepared state
explicitly. The production Rhai contract at
`assets/scenarios/tests/sysml_twin_analysis.rhai` checks the committed
requirement facts and the unmounted-Twin diagnostic. Source-asset changes admit
a new revision, and `TwinClosed` retires queued work and fences late results for
that Twin. An empty selected set commits an empty analysis without dispatching
a worker. The `twin.lifecycle` hook receives `Twin/Lifecycle` with the mounted
`TwinId` as its generation, no elapsed clock, and an explicit `Start`, `Event`,
or `Stop` phase. Its retained `policy_status().lifecycle.runtime_context` makes
the owner stamp inspectable from Rhai and the API. Identical Rhai source misses share one
immutable compile result. Rhai drains worker results into a scene-wide
preparation barrier and commits the complete ready set in stable actor order
before `TimeSpineSet` releases simulation time. Its exact progress holds remain
active through dependency planning, top-level initialization, and the first
`on_start`; cache hits use the same activation boundary without a worker.
Scenario compilation captures the transitive literal-import closure from one
revisioned source snapshot and reuses matching source-asset ASTs, preparing any
missing ASTs on the worker. At commit, the owner validates every discovered
source or missing-source input and publishes the ASTs to a prepared-only
scenario resolver. A module absent from that prepared closure fails visibly
instead of being compiled during a lifecycle call. Module body evaluation
remains synchronous at the owner lifecycle boundary because it can execute
authored world behavior. Standalone `SysmlDocument` edits capture immutable
source and origin facts for shared `AsyncWorkAdmission`; the owner commits only
for the exact current generation and origin URI. `InspectSysmlDocument`
distinguishes pending, ready, and failed states, and editor verification treats
pending as retryable. This analysis remains editor-only and never holds
simulation progress. Terrain cover preparation now uses shared admission;
remaining USD and Modelica library preparation and terrain tile-bake admission
remain owner-local or synchronous. On wasm, native admission rejects CPU work
until a Web Worker transport exists; terrain cover preparation therefore stays
synchronous on that host rather than silently running on the browser main
thread.

Results are committed only by their owner at a named cycle boundary. Results
for presentation may be adopted when current and useful. Results that change
authoritative topology or inputs carry a declared target simulation boundary;
that boundary waits for the complete required set, validates every revision,
and commits in stable owner/identity order. Completion time and worker arrival
order never choose a tick. Unrelated work never joins that admission hold.

Dynamic referenced spawns on the mounted primary USD stage fetch and prepare in
parallel, while live-stage mutations and terminal failures follow reference
operation order. The owner drains only the completed prefix for that stage and
retains later ready or failed outcomes until earlier active references resolve.
A failed prefix operation faults and holds the simulation before any successor
can commit. Preview and non-primary stage projections do not join this
simulation boundary.

Parsing and compilation should move off the UI and fixed schedules when their
inputs can be captured immutably. A script's top-level body is executable
world behavior, not pure compilation, and remains serialized at activation.
Likewise, a live OpenUSD stage is thread-affine; moving immutable source
preparation off-thread does not mean sharing the stage object with a worker.

## 5. Rhai execution and safe parallelism

Rhai inline-root parsing and immutable compilation artifacts are prepared
through shared admission. File-backed source assets are parsed in Bevy's async
asset-loading task and publish text plus source-matched AST at the asset
boundary; tools and prelude installation reuse that AST instead of compiling on
the update thread. The owner buffers completed artifacts until every
currently admitted scenario compile is ready, then commits by stable actor
identity before the time spine. A progress hold remains through dependency
planning, top-level initialization, and the first `on_start`, so physics cannot
consume a tick before activation. Scenario `this` state and live-world calls
remain owned by the script activation/execution boundary. A paused Update
activation still assigns dependency planning, initialization, and `on_start` the
Simulation clock and current sequence; discrete events retain Lifecycle context.
Literal transitive module sources are captured from one revisioned snapshot;
matching asset ASTs are reused and missing ASTs are prepared on the worker. The
scenario resolver accepts only owner-committed ASTs. Imported module bodies
still evaluate on the serialized lifecycle path because they can execute world
behavior. All functions and hooks inherit the invocation context; the scenario
owner assigns each callback to its Rust-owned cycle. Event handlers read both
the event's origin stamp and the consumer's current cycle. Source metadata may
validate an author's required cadence, but it does not install or move a hook.

Before a scenario's first lifecycle hook, a source may define the optional,
scenario-scoped `simulation_dependencies(me, ctx)` hook. The second argument is
the validated scenario parameter map. It returns a map with
`modelica_entities: [global_entity_id, ...]` and
`required_inputs: [#{ owner: "domain.owner", identity: "stable-key" }, ...]`.
The owner resolves the entity ids against the live registry, requires each to
identify a live Modelica participant, and adds them to the shared simulation
barrier. An unresolved id or a live non-Modelica entity is a terminal source
diagnostic, not a successful barrier contribution. Each required input
names an owner registered in `SimulationDependencyStates`. A missing owner is
a terminal diagnostic; an absent or Pending key from a registered owner keeps
the scenario's existing activation hold until the next owner-state revision;
Ready admits initialization, and Failed reports the producer diagnostics.
The dependency plan runs once per source/parameter revision, and readiness is
checked again only after an owner publishes a change. Omitting the hook declares
no Rhai dependencies; the USD causal graph still applies. The hook resolves
identities from the composed world and parameters; commands, direct mutations,
emitted events, and live port access are rejected in this phase. While a plan is
pending, all Modelica participants remain synchronized and simulation time
stays held by the scenario's exact preparation key. The production sensor scene
verifies declared Modelica reads, rejects a live port read during planning, and
rejects a live non-Modelica id before the scenario can enter `on_start`.
This hook runs before mutable top-level initialization, so derive its result
from `me`, scenario parameters, and read-only world queries rather than
top-level initialization effects. Once all declared inputs are Ready, the owner
commits the Modelica participant set, runs top-level initialization in the
`Initialization` phase, and dispatches `on_start` in stable actor order.

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
participant result can affect authoritative state. USD `SimConnection`s and
Rhai scenario dependencies both contribute to the same barrier projection. A
scenario that reads or writes a Modelica port, or consumes a Modelica-produced
event that can affect its behavior, lists that producer in the
`modelica_entities` field of `simulation_dependencies(me, ctx)`. Rhai keeps the
selection policy; Rust resolves the returned ids, requires each to be a live
Modelica participant, and adds that scenario's contribution to the shared
barrier. A live non-Modelica target is a source diagnostic, not an empty
dependency. Required owner inputs occupy
the same plan but do not add Modelica barrier participants. While a dependency
plan is pending, all Modelica
participants are synchronized. After admission, direct simulation-clock access
to an unbarriered Modelica port or event fails at the scripting owner with a
diagnostic that names the missing hook. Presentation reads continue to observe
committed state without joining the authoritative barrier. Continuous
calculations and physics remain in their domain owners.

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
uses one deadline heap per authored clock binding, visits only due channels,
and queues a small typed record for bounded post-simulation delivery and
retention. Stable identity order decides the sequence when multiple channels are
due together; clock rewinds reset cadence without catch-up bursts. The fixed
pass still visits active clock lanes, and measured physics cost remains open.
API subscriptions, display decimation, log formatting, recording encoders, and
file/network I/O run after capture on their owning application or background
cycle. Live display may report dropped observations; a configured lossless
recording must instead fault explicitly if its bounded queue cannot keep up.

Telemetry collection is bounded by the configured channel cap and per-channel
rate. It must not enumerate unrelated diagnostics, rescan port backends, or
rebuild UI summaries on each physics tick. UI plots consume the retained
`SignalRegistry` history by revision and derive decimated points at visualization
cadence. The shared causal event order remains authoritative even when optional
telemetry consumers are delayed.

## 8. Stable ordering and numeric scope

All observable batches use a total key owned by their domain. Current runtime
ordering includes:

- scenario actors: the source-owned `GlobalEntityId` component, read directly
  instead of through the Update-synchronized API lookup index; hosts without a
  global identity use their Bevy entity key only within that running World and
  are outside cross-session replay ordering;
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
authoritative asset and structural-projection terminal edge. CPU-generated
render meshes stream independently and remain visible to presentation readiness.
The gate exposes its owner and operation key plus a user-facing wait reason;
matching failure and completion edges release only their own operation.
`TimeTransport` mode and rate remain the user's intent while this gate pauses
`Time<Virtual>`.

The whole-simulation guarantee remains open because:

1. Scene start composes owner-specific deterministic boundaries rather than one
   global readiness bit. The root USD loader fetches and composes its available
   dependency closure before publishing the stage asset; the lifecycle progress
   key holds through structural projection; active primary references and
   causal Modelica compilation hold exact `SimulationProgress` keys; the USD
   terrain bridge and progress scan run in `PreUpdate` before `TimeSpineSet`,
   including a pending Twin manifest scan, so authored terrain data and
   collider work hold entity-keyed `SimulationProgress` before the first
   eligible fixed tick and through the web worker's coarse-to-full result; physics
   admission uses the physics readiness owner; and scenarios open only after
   their readiness state clears. These owners intentionally control different
   clocks. CPU render-mesh construction no longer delays authoritative scene
   admission. The production sensor fixture captures its first simulation
   behavior call and verifies finite reads from both declared Modelica
   participants and admitted physics ports. Its Rhai test also attaches two
   scenario actors: the lower `GlobalEntityId` actor writes a temporary marker,
   and the higher actor must observe and restore it during `on_start`. The
   initial altimeter miss remains explicitly invalid until the first Avian ray
   sample; a present zero is not treated as a valid range. The production
   harness `scripts/compare_deterministic_startup_dependencies.py` runs the
   scene twice at Compute width 1 and twice at the default width 24, then
   compares the exact first-behavior trace for scene generation, actor identity,
   IMU, altimeter, and contact values. The 2026-09-25 main-integrated build
   matched at tick 10 across all four runs and passed the authored actor-order
   verdict at Compute widths 1 and 24, with snapshot digest
   `8ad6116be742467fe9d20f40e6e4b8e9c88ad59d587917550a08a0a3e7fe6bf5`.
   Scene-readiness holds varied from 1,164 to 1,370 Update passes; participant
   readiness took 9 passes, and each run took 1.0–1.2 seconds. This is
   same-build evidence for one authored dependency graph; it does not force
   opposite Modelica completion order or establish complete scene closure.
   The first normal co-simulation step uses the per-step barrier after
   activation.
2. Dynamic references on the mounted primary scene hold admission through
   closure preparation and live projection; preview and additive mounts remain
   independent. Initial USD composition dependencies are fetched and composed
   by the root asset loader before the stage asset reaches structural
   projection.
3. Rhai port and event reads require a declared `simulation_dependencies`
   closure and unbarriered access fails visibly. A complete typed action path
   for every script write remains open.
4. Twin `AnalyzeSysml`/`ValidateSysml` queries now read committed async source
   snapshots; read-only analysis uses interactive admission and does not hold
   simulation time. Scenario plans can now wait on generic owner-published
   readiness keys without polling or making editor analysis a global hold. The
   production `sysml_async_analysis` scene verifies that a declared Twin
   analysis dependency admits the script only after its source-set snapshot is
   ready. Standalone SysML document analysis uses shared async
   admission and exact generation/origin fencing. Initial USD document
   parse/overlay serialization remain synchronous. Rhai module body evaluation
   remains on the owner lifecycle path; it can
   execute world behavior even though parsing and compilation are asynchronous.
   Native Modelica source interfaces and their sorted required-root sets are
   extracted once on Bevy's async-compute pool while the source asset loads;
   co-simulation, member discovery, and the web workbench reuse that
   revision-matched interface. Live compile producers derive and admit compiler
   dependencies from the parsed document AST before `Compile` on one ordered
   worker channel; generated source documents also expose their authored root
   manifest for class resolution. Document compiles derive requirements from
   their primary and sibling parsed source set. Worker root preparation commits in admission order; the live
   compile entry point rejects unadmitted roots, and a failed root is retained
   as a terminal compiler state for dependent compiles. Native root installs and
   ordinary compiles run on the single Rumoca actor, while immutable DAE lowering
   uses the bounded solve-preparation pool, including persistent solve-cache
   I/O. Reset, UpdateParameters, and
   cache-invalidating Step auto-init are continuations on the same actor FIFO;
   entity/session/library-generation fences apply before commit, and
   Step-triggered work shares the bounded admission count. Native compile and
   lowering no longer block the Modelica command owner. Bevy's wasm task pool
   runs on the browser main thread, so the web loader still needs a Modelica Web
   Worker handoff for a fully non-blocking parse and compile path.
5. The simulation composition records the observed Compute pool width in
   `PhysicsComputeProfile`; `clock_snapshot()` returns `physics_profile_known`
   and the optional `physics_compute_threads` value, which the production
   scripting task scene checks. The scene-test `--threads 0` profile uses
   Bevy's default `TaskPoolOptions`, matching GUI `DefaultPlugins`; the normal
   headless server entry point currently pins one Compute thread. Bevy's default
   assigns 25% of available threads to IO and AsyncCompute each, clamped to one
   through four, and gives Compute the remaining cores. Avian uses Compute for
   parallel broad-phase, narrow-phase, and constraint work. Broad-phase chunk
   results retain input order before contact-graph insertion; narrow-phase
   status bitsets combine before serial graph/solver updates; the active
   collision filter is read-only. USD projection supplies `PhysicsOrderKey`
   from the instance root and authored prim path. The physics owner validates
   joint keys after Avian prepares solver data and before its substep loop;
   native joints, motor warm-start, custom prismatic correction, raycast
   suspension and tire forces, jointed tire forces, and raycast wheel
   mass-property folds use that key order. Missing, duplicate, or empty keys
   raise a runtime fault and hold physics. The production
   `multi_rover_stress_20` Rhai replay gate compares rover physics and Modelica
   snapshots. On the 2026-09-25 main-integrated build, a four-run matrix at
   Compute widths 1 and 24 matched all six checkpoint snapshots, 32 per-tick
   samples, 240 Modelica participant snapshots, and all 20 articulated-body
   states in every run, with digest
   `0a080decafbe69264944d141da6afa3e02a87514e2d1da4e92d2317713f82dd1`.
   Every run produced the authored Rhai stress verdict. Scene-readiness holds
   spanned 3,760–4,645 Update passes; physics-admission holds were 5 passes,
   participant-readiness holds were 11–12, and each scene run took 26.8–27.1
   seconds including startup. This is same-build evidence for that fixture and
   those pool widths; it does not establish whole-simulation or cross-machine
   determinism. The current profile records effective pool width but does not
   select a deterministic Avian solver profile; that guarantee still needs a
   measured production choice or deterministic reductions.
6. The command journal does not yet provide a whole-simulation authoritative
   input log and replay verdict. Networking rollback's bounded per-vessel input
   frames retain ordered, latched `SetPorts` setpoints for owned-body replay,
   but do not capture scene lifecycle, authored external commands, or all Rhai
   and Modelica state. Adaptive Modelica is not a cross-machine bitwise
   deterministic solver.
7. `RuntimeCycleSet` is ordering vocabulary rather than an independent cadence
   driver. Typed context reaches scenario preparation/start/event/behavior/stop
   calls and one-shot Rhai evaluation. Generic registered hooks now receive a
   typed `HookInvocation`; Rhai hooks expose its immutable `runtime_context`
   map and native hook ABI v2 transports the same map. The Rhai world bridge
   forwards the active context for nested `invoke_hook` calls. The physics
   escape owner also supplies its core simulation `Behavior` context from
   `SimTick` and `Time<Fixed>`; the authored policy rejects other cycles. The
   scheduled `link.connected` owner supplies the active/committed scene route and
   its `Simulation/Preparation` or `Simulation/Behavior` context. Because its
   sweep runs in `Update`, behavior carries the latest completed `SimTick` and
   `WorldTime` without inventing a per-call delta; an installed policy fault
   rejects that edge. Time-free offline, lifecycle, and command decisions keep
   their discrete owner context or remain explicitly unclassified. The settled
   USD scene-time owner invokes its policy with the exact transition generation
   in `Twin/Lifecycle/Preparation`; `SceneTimeSelection` preserves that id
   through apply, and the time owner ignores stale completion/apply edges. Its
   authored policy is also inspectable in `Application/Repl/Evaluation`. Other
   scheduled Rust hook owners remain open until their cycle owners classify them.
   GUI UI and LOD still share the main `Update` schedule, although server hosts
   now omit the visual plugin entirely.
8. Async task admission and priority are local to individual owners. The shared
   Bevy pool can be saturated by background work, and completion commits are
   not yet governed by one cross-owner budget/order contract.
9. Rhai hooks and co-simulation still have live-world access paths that prevent
   safe parallel evaluation even where the dependency graph contains
   independent actors.
10. Telemetry sampling now uses per-clock deadline heaps and only reads due
    channels in the fixed simulation cycle. Subscriber callbacks run in a
    bounded post-simulation telemetry cycle, but capture cost and end-to-end
    observer throughput have not been measured against physics.
11. Cycle duration, queue pressure, and overload counters are not yet exposed
    together at the diagnostics boundary, so optimization cannot target an
    owner using comparable cycle evidence.
12. Bevy's fixed loop executes synchronously on the GUI app thread before
    `Update`. The transport policy allows a 64-step catch-up burst at its
    highest rate; a fixed delta preserves numerical step size but does not
    promise 60 wall-clock physics ticks per second or responsive UI during a
    long tick/burst. No integration-level tick-deadline or backlog result is
    published to distinguish a slow solver from lost wall-time admission.
13. World-bound REPL requests use a bounded 64-entry FIFO, reject excess
    commands visibly, drain one request per `Update`, and cap each live-world
    invocation at 100,000 Rhai operations. This bounds interpreter work per
    application frame while preserving serial command order. A single native
    bridge call can still be expensive, and scenario hooks remain serialized
    in their owning schedules. Terrain lockstep mode waits for all in-flight
    bakes inside `Update`, and ordinary mode commits every completed bake in
    one pass, so worker completion bursts can still produce a main-thread
    hitch.
14. The async admission queue limits its own in-flight requests to four and
    priority selects queued work only. It cannot preempt running jobs, and many
    visualization/preparation producers still submit directly to Bevy pools;
    there is no measured cross-owner CPU reservation for UI and simulation.

These findings and their owner-specific file evidence are maintained in
[`../reviews/open-deterministic-simulation-contract.md`](../reviews/open-deterministic-simulation-contract.md).

## 10. Migration order

1. **Canonical boundary order.** Sort current scenario, event, and serialized
   Modelica work by stable keys, including input assignments inside each worker
   step request; add production Rhai verdicts for observable
   event ordering. Keep source-owned identities and fail visibly when a
   required identity is invalid.
2. **Cycle and clock context.** Extend the typed invocation context from
   scenario and REPL owners to every callback owner, then expose it read-only to
   Rhai. Give UI and visualization independent cadences while keeping both
   presentation-only.
3. **Async work admission.** Use shared bounded priority admission over the
   existing worker pools. Modelica document parsing, Rhai inline roots and
   asset AST cache misses, and Twin SysML source-set analysis use native
   admission. Twin and document analysis use `Interactive` priority and never
   acquire the simulation progress gate. File-backed Rhai AST parsing uses the
   Bevy asset task pool. Native Modelica source-root file reads,
   input-default extraction, and parsing now run on the bounded Modelica
   preparation pool; the sole Rumoca session owner commits prepared roots in
   admission order and holds compile, parameter-update, and reset commands
   until root commits finish. Measure the remaining non-preemptible session
   installation and compile work before splitting those operations further.
   Move USD, remaining Modelica, and visualization preparation to workers while
   preserving owner-specific typed results and commits. Keep Web Worker
   admission explicit for wasm hosts.
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
7. **First-tick admission and physics profile.** Prove that the first
   authoritative fixed tick follows every owner-specific readiness fact
   required by the active scene and scenario. Keep simulation-required async
   work on exact `SimulationProgress` keys, physics readiness on the physics
   gate, and presentation readiness on its own status path. Preserve the
   schedules needed to finish physics admission and first solver exchange;
   preparation must remain live while simulation time is held. Measure the
   serial and any deterministic parallel solver profile before selecting the
   production default.
8. **Real-time owner isolation.** Route main-world command reads/writes through
   typed tick-stamped requests and immutable snapshots, then move the whole
   authoritative tick to one paced simulation owner. Keep fixed `dt`, report
   deadline misses/backlog, and let realtime, unpaced capture, and test drivers
   choose wall pacing without changing tick order or discarding ticks. Retain
   the current same-world path until every authoritative consumer crosses this
   boundary; do not run only the solver concurrently.
9. **Replay evidence.** Record admitted inputs and deterministic result keys;
   add a production scene suite spanning USD projection, Modelica coupling,
   Rhai events, SysML revisioned verification, and Avian state. Compare state at
   tick boundaries, not wall-clock completion times.
10. **Performance validation.** Profile one settled production scene and one
   cold-start scene. Verify worker priority, UI frame cost, and simulation
   throughput together; async work must improve responsiveness without hiding
   solver cost.

The simulator can claim whole-simulation determinism only when these phases
close with production evidence. Current stable ordering is a necessary first
phase, not that final claim.
