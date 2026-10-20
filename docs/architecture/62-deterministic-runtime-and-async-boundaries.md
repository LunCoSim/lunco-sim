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

`lunco-time::simulation_presentation_fraction` owns the bounded interval between
completed physics states. The presentation-time sample and application physical
easing both consume that owner. Retained fixed backlog never becomes an
extrapolation fraction and is not discarded by a presentation read. The shared
headless/headful application composes native endpoint capture, invalidation and
FixedFirst restore with its easing consumer through the public nonlinear-easing
markers. Invalid endpoints fault and hold physics before quaternion math. This
is a continuous clock/geometry invariant, with no Twin-specific Rhai policy.

The workspace enables Avian's maintained `enhanced-determinism` feature. In
addition to deterministic math, it selects Parry's ordered contact-subdetector
caches, including heightfield manifolds. Terrain collider bakes commit complete
required cohorts in canonical coordinate order. Missing resident collision
support holds shared simulation admission as well as physics; guidance and
engine state cannot run ahead while bodies wait for terrain. Fixed admission
refreshes the physics hold after asynchronous coupling completion so force
delivery and integration consume the same clock state.

Griffin same-host evidence on 2026-10-05: source-pinned fresh runs have exactly
equal final pose/speed/upright values at tick 5110, and one retained warm-reload
observer matches 91 samples across two 5400-tick replacements. Logs:
`target/griffin-enhanced-determinism-replay-driver.log` and
`target/griffin-egress-contact-deterministic-49817.log`. This does not establish
cross-platform equality or acceptance of the whole mission.

The runtime remains event driven. It does not poll for readiness on every
simulation tick, and it does not add a second authoritative time source.

Passive joint damping belongs to the ordered `lunco-physics` substep owner.
The `LunCoJointDampingAPI` rates are in 1/s. Point-velocity and angular-velocity
decay use exponential flows in that order, once per actual physics substep;
position-closure iterations do not integrate them again. Point impulses act at
one common midpoint of the current joint anchors and include their moments on
both bodies. Effective inertia, locks and dominance define the movable response.
The numerical owner validates the complete velocity candidate before either
endpoint write; invalid state raises a runtime fault and safety hold. The
`native_force_drives` authored gate owns runtime acceptance, while the shared
joint module tests its numerical kernels and conservation properties.

Hard joint closure is prepared by `lunco-physics::joint_projection` after native
joint preparation, in the same stable per-type order used by the ordinary pass.
Its fixed-size geometric rows cover fixed, revolute, prismatic, spherical and
distance joints. Only zero-compliance channels enter repeated closure or CCD;
motors and compliant channels retain one ordinary integration per substep.
Rows retain full, axial or perpendicular constrained subspaces as typed
geometry. Their current scalar gradients and native inertia responses enter
`lunco-physics::constraint_response` together with admitted continuous-contact
inequalities. Responsive body islands bound solver work; row order and
principal-pivot choices remain deterministic. Two sparse body reductions apply
the native response and its transpose without forming a dense row matrix.
Maintained faer LSMR runs with explicit `Par::Seq`, 1e-12 absolute/relative normal-residual
stopping bounds and the owner's iteration bound, including
rank-deficient hinge/latch rows. This avoids demanding machine-epsilon
normal-equation residuals from ill-conditioned redundant contact systems.
Each nonlinear correction is checked against the geometric tolerances;
least-squares convergence alone does not admit a pose. Unilateral rows release
pulling reactions and redundant slack. Initial unilateral admission and later
re-admission both require error beyond the row's declared residual tolerance.
Excluded rows stay in the complete response check and enter if coupled motion
crosses that bound. Active rows still target zero error; solver precision and
geometric writeback tolerances do not change. Allocation failure, failed numerical
convergence or an unresolved native fixed point rejects the operation visibly.

The native zero-guess solve owns minimum-norm multiplier selection in the
diagonally scaled coordinates. Preserving a body correction or equation residual
alone does not preserve contact mechanics: the contact multiplier feeds native
friction. A numerical acceleration must preserve that reaction selection in
rank-deficient systems. The mechanism regression with two identical hard rows
and one identical contact row requires the contact multiplier to remain `1/3`
when their total response is one.

The owner sums every generalized impulse before integrating any body's
rotation, using the native positional and angular impulse kernels. It validates
all body candidates before mutation and writes only prepared native solver
state. Joint and contact reactions use the same solved multipliers. Current
material trajectories, joint rows and full native geometry are revalidated
before pose writeback. Any failure faults and holds that writeback boundary.
Motors, compliant channels, locked axes, dominance and the existing convergence
tolerances retain their owner contracts.

Hook review: this owner consumes current f64 errors, gradients, native inertia
and typed convergence bounds and produces coupled numerical corrections and
reactions. Continuous constrained dynamics stay in Rust, installed in the shared
headless/headful physics composition with per-substep lifetime. Failure raises
a structured physics fault and safety hold. Production acceptance includes
`continuous_articulated_mass`, `continuous_contacts`, the joint-kind and detach
gates, and the held-contact-load Rhai observer.

Prismatic alignment and travel-limit directions rotate with the first joint
frame. Their differential includes the attachment separation: the first angular
lever is `r1 + separation`, matching the coordinate drive's Jacobian. This retains
angular coupling after free slider travel. A central finite-difference regression
checks both geometric row types.

Hard orientation and position rows share the coupled response owner above.
Their current native inertia and constrained directions enter the same solve,
including interactions with contact rows. The native point-response
decomposition remains owned by joint damping, with f64 eigenbasis projection
and its checked rank policy.

The native-drive Rhai gate establishes its analytical mass/inertia baseline
from the first completed physics step, after native mass preparation, then
compares twenty consecutive samples. Tick-zero scenario initialization does
not supply prepared native mass properties. Its angular-momentum tolerance
accounts for the native shape mass properties' f32 source precision; the
coordinate-law and completed-reaction checks retain their dimensional bounds.

The shared headless/headful physics composition disables Avian's XPBD plugin.
The application joint owner composes its public preparation and impulse kernels
with ordered drive, geometric and closure passes, f64 velocity projection
before contact relaxation, and reaction finalization. The native contact solver
stays installed. The application finalizer stages all joint reactions before
publishing any of them. `JointForces` retains the completed report while a body
sleeps; disabled joints clear their report. A failed candidate faults and holds
physics before native body and contact writeback.
Each guarded solver phase evaluates its own fault condition at entry; a shared
condition cached before earlier phases cannot protect against a later failure.

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
| `Ui` | egui/workbench updates and authored pointer/menu tool hooks | host frame/input cadence; typed tool hooks run after picking in `PreUpdate`, before fixed simulation, while egui paints in `PostUpdate`; foreground context-menu bounds are registered as chrome in `ScenePickGate`, so a popup inside a scene viewport owns its clicks; pointer-move hooks run only for an opted-in document interaction, with samples coalesced as presentation updates |
| `Visualization` / `Presentation` | LOD selection, render preparation, visual projection | presentation cadence or an explicitly selected visual time domain; terrain cover reselection is capped at 30 Hz using `Time<Real>` |

Default API-launched scenarios use the persistent `WorldRoot` as their
Application host. Script-command admission marks that host authoritative;
the session's identity admission and entity index supply its stable identity
before declared simulation queries consume it. Twin scenario hosts use the
same session mechanism and retain their explicit Twin lifetime.

The USD-to-telemetry bridge samples connected co-simulation event edges only
while the virtual simulation clock advances. It runs after `SimTickSet` and the
full `ScriptingSet`, and stamps each event from `MissionClock` at that exact
`SimTick`. When scenario execution is held during startup, the telemetry edge
may precede `on_start`; scenario inboxes do not replay pre-start edges, so
`on_start` reads the current state from its owning subsystem.

Derived route geometry is presentation work. Rhai selects route points and
submits them through `UpdateUsdCurveView`; the route-follow program is the
single live ribbon-refresh owner, and the waypoint editor submits no duplicate
view request after committing an edit. The UI presentation owner captures the
route-parent pose and immutable terrain facts, coalesces requests and admits
at most two preparations. The terrain owner then prepares spatially indexed
sparse stroke records on bounded workers and publishes only current source
revisions. Its fragment shader consumes that image on the ground's own LOD
surface; camera changes and elevation edits do not schedule curve preparation.
A scene without declared terrain uses its authored 3D curve. Fewer than two
points removes the view immediately. These products cannot edit USD, choose
route state or delay a simulation tick. `InspectUsdCurveView` includes current
surface-image publication in readiness and reports segment count, projection,
binding count and terminal failures. The
`usd.document.projected` telemetry event carries `changed_prim_paths` for the
reconciled stage batches and fires after the affected referenced roots have
reached their live instance projection. The pending roots come from the edit's
typed reference-add operations, so a root is tracked even before its async asset
admission reaches the stage sink. Pending references elsewhere in the same
stage do not delay this edit's completion or release of its document-projection
hold. The document key covers native closure preparation and the active
reference roots entering ECS.
Authored policy filters that typed path set
before it re-reads route topology, so unrelated edits in the same document do
not cause a route scan in the fixed simulation cycle.

Document-backed reference batches admit their complete native dependency closure
before typed replay. Native reference arcs are authored before any sibling
relationship or joint can be projected. Render, physics, authored-runtime and
query readers use the same scene-stage owner. The native change sink and generic
projection queue own bounded ECS materialization; retained source handles own
closure lifetime. No instance read-mode transition is needed after an edit.
The document key retains its scoped admission hold, while malformed or policy-rejected closures terminate with a
structured stage-projection diagnostic before an intermediate topology is
published.

An edit to a document's root scene is already owned by that scene's canonical
stage. Dependent-stage refresh applies only when the changed layer is composed
under a different recipe root; the source scene is not rebuilt as its own
dependent asset.

Scenario lifecycle has one readiness boundary: the optional, read-only
`simulation_dependencies` plan declares prerequisites; after those inputs are
committed, per-instance module initialization runs once and `on_start` runs
once as the ready/activation callback. `on_event` handles a delivered edge in
the scenario owner's next eligible pass. `on_start` and `on_event` inherit the
cycle chosen by the Rust owner; event producers do not choose the consumer's
clock. Do not add `on_init` or `on_ready` aliases: top-level initialization and
`on_start` already own those distinct responsibilities. A stateful scenario
also does not register callbacks in arbitrary clocks. Work in another cycle
belongs to that cycle's owner and uses a typed, owner-scheduled hook or a
separate script owner with its own state and inbox.

Required-input readiness is checked after `simulation_dependencies` returns.
That hook declares exact owner keys and discovers live identities without
reading pending owner facts. Source-selected entity access is committed with
`track_entity_read`/`track_entity_write` during admitted initialization or
startup before the first access. Twin SysML keys use the current mount authority
returned by `twin_name()`.


`RuntimeCycleSet` names ordering lanes inside Bevy schedules. It does not by
itself isolate CPU cost or give `Visualization` an independent cadence. Terrain
cover reselection has an explicit 30 Hz wall-clock skip boundary, and its
immutable calculation uses shared bounded background admission. The owning
system checks the camera, surface, and operation signatures before committing a
result. Native tile-mesh bakes use the same bounded admission at `Interactive`
priority, with the owner's nearest-first rank followed by stable operation
identity. The terrain owner validates each completion against its current key,
then uploads and publishes meshes in stable coordinate order under a per-frame
budget. Camera, generation, and surface changes withdraw queued work; already
running work can finish but its stale result is discarded. Removing the terrain
owner cancels its queued visualization preparation and retires queued results.
Browser builds keep their async cache path until a Web Worker transport exists.
These CPU jobs no longer enter the frame's Bevy task queue directly. Selection,
mesh upload, visibility, and residency changes still use the `Update`
visualization cycle.

Active Twin policy manifest and source reads use the shared bounded admission
queue on native hosts. Only immutable indexed paths cross to the worker; the
scripting lifecycle owner validates Twin identity, root, and operation before
committing the typed result. The active Twin's `assets_mounted` policy plan
waits for activation, while `TwinAssetMounted` remains immediate for other
consumers. A `TwinPolicyPreparation` progress hold prevents the first
authoritative tick and full-readiness result from racing ahead of that commit;
the app/UI schedule continues. Browser builds retain the WebStorage read path
until a browser worker transport is available; see
[`hook-policies.md`](hook-policies.md).

Native application policy discovery, manifest/source reads, and Rhai hook
compilation, optional-owner filtering, owner-contract/entry-arity validation,
and installer-order selection are prepared as one fixed startup job on
`AsyncComputeTaskPool` while the runtime finishes installing its plugins. This
keeps the lifecycle commit deterministic while removing source I/O, Rhai
compilation/evaluation, owner/entry validation, and source-text round trips
through the startup script from the app thread. The authored function receives
ordered hook identities while Rust retains the prepared definitions and
validated callables. `PreStartup` validates the typed selection and its
manifest records, then commits hooks in that authored order before any
`Startup` consumer. The one-shot job is bounded to one bundle; a missing pool
or worker failure becomes an application policy diagnostic instead of a
synchronous fallback.

Modelica runtime telemetry is event-gated after worker responses and on document
metadata or telemetry-settings changes. Unchanged render frames do not rescan its
variables; each recorded sample still carries the solver's landed model time.

Sparse structural lifecycle work is gated by its pending marker before
entering an owner. Joint admission readiness uses one disjunction across
`PendingUsdJoint` and the shared `PendingJointAdmission` marker; when there is
no candidate, idle updates skip the scene-projection and solver-endpoint checks.
Candidate presence reruns that reconciliation before the admission commit, so
the stable joint batch boundary is unchanged. Joint retirement uses the
existing `PhysicsJointDetachRequested` component condition at `PreUpdate` and
`FixedPreUpdate`, so idle boundaries skip the exclusive graph transaction and
its world query. Both call the same graph-safe owner outside native physics.
`PreUpdate` services paused/preparation-time commands; `FixedPreUpdate` retires
requests from a completed simulation tick before the next tick, independently
of how many fixed cycles share a render frame. This is a topology-commit
invariant, not a scenario policy hook. The authored `joint_detach_clock` and
`joint_detach_paused` observers qualify the public command path through
`scripts/api/test_joint_detach_clock.py`.

Co-simulation endpoint lifecycle publication retains Bevy's native exclusive
`QueryState` for changed `SimComponent` owners. Each pass updates archetype
membership and reads live status with the same system change ticks, collects
transitions before structural mutation, then publishes them immediately in
`Update` before the `PostUpdate` binding transaction. The query cache does not
retain endpoint values or change binding revision admission.

The pre-simulation `PreUpdate` order is `Lifecycle` → `IdentityAdmission` →
`EntityIndex` → `SimulationAdmissionSet`. Lifecycle projection creates the ECS
entities; the identity owner assigns their stable IDs; then the API registry
publishes path lookups for those identities. `ClockProjectionSet` runs in
`First`, before `TimeSystems` samples the next virtual delta. Modelica completion
admission uses the core runtime's `SimulationCompletions` schedule, serviced
after message rotation and before `ClockProjectionSet` in `First`. The native
driver also services that same schedule on response readiness while waiting for
the next fixed deadline. The shared fixed runner services it once before checking
a held pending cycle, so receipts completed during native integration can release
the next cycle in the same update. Neither path waits on unfinished work or
reruns the application update.
The completion systems retain one state and their existing session, source,
step, and communication-endpoint fences. Readiness does not consume or apply
results, advance clocks, or choose commit order. Only a fully
released causal barrier allows the fixed runner to resume. The fixed runner
then checks current transport, scene-time, progress, and coupling admission
before every complete `FixedMain` cycle. A barrier raised by one cycle stops
the next cycle and keeps running demand in `Time<Fixed>::overstep` until the
owner releases it. This makes newly admitted referenced entities
visible through `find_path` on the first resumed tick, without a startup-only
route or a second scene-ready signal. The identity owner collects assignments
in query iteration order and commits the same-component IDs through Bevy's
fallible batch command before `EntityIndex`. The production `route_lifecycle`
Rhai gate exercises reference admission and verifies that first-tick observation.
Reference batches wait for their entire admitted closure before ordered native
replay. Their document projection lease waits for active reference roots to
reach the generic live ECS projection; worker completion cannot publish an
intermediate sibling joint topology. `clock_snapshot().simulation_progress`
reports the held state and ordered owner/operation/reason facts.

Scene, scene-time, terrain, Modelica initialization, and primary physics
admission all hold the shared fixed clock until their owners publish readiness.
USD physics projection follows the same lifecycle boundary: the BigSpace bridge
seeds body poses in `PreUpdate`, then the joint owner's `JointPreparation` set
resolves authored joints in `Update` after USD simulation projection and before
`JointAdmission`.
If a dynamic endpoint needs an Avian `BodyIslandNode` before its joint can be
admitted, the USD owner stages that body as dynamic only after
`PhysicsHolds::BODY_ADMISSION` is active. An unfrozen body keeps its
`ShouldBeDynamic` admission marker until authored joint and differential
markers clear, so the fixed clock cannot run between endpoint staging and
constraint admission. Joints in a policy-paused assembly leave the shared
admission batch, allowing unrelated valid joints to commit. A body already
frozen for a scoped readiness wait may complete its local state transition
while the shared hold waits on the pending constraints. Scene readiness and the
first cycle release only after the complete body and constraint boundary closes.
Terrain initial-pose support validation also waits for the active root's
structural and USD simulation projection to settle before checking colliders;
bounded projection order cannot admit a body while its authored support
collider is still absent.
USD `EmbeddedScenarioPath` publication immediately reserves one actor-owned
`ScriptPreparation` hold through `ScenarioPreparationAdmissions`. The same key
passes through document admission, compilation and `on_start`; document-only
cancellation cannot release a source still owned by the loader. Pending actor
sources also block startup activation, so asset completion order cannot choose
which actor initializes first. Replacement resets the loader's terminal state
while retaining its actor hold; removal and despawn retire the actor's source
admission. Independent `RunScenarioAsset` requests use allocator-owned
`ScriptSourcePreparation` keys until their serial attachment transfers
admission to compilation; discarding a request releases only its own key.

Root and recursive imports must be loaded and published in main-world `Assets`
before attachment. `lunco-scripting-rhai-world` validates the complete source
closure before committing either the source or prepared AST registry. Native
load completion alone is insufficient; pending publication retains the request
and hold. Invalid closures commit neither registry. Built-in preludes and Twin
tool libraries use that same closure owner. Load, identity or import-closure
failure raises `scenario-source-failed` at the scripting owner and retains the
authored input and its admission until replacement or removal. Failed sources
are terminal for that revision rather than retried every frame.

Rhai compilation, dependency planning, initialization and `on_start` run in an
ordered pre-tick lifecycle pass; the preparation hold remains active until
`on_start` completes. A fresh process starts `on_start` at `SimTick=0` and admits
its first `on_tick` at `SimTick=1`, without advancing physics or Modelica during
scene admission. UI and asset work continue while authoritative time is held.
The preparation pass borrows canonical script documents while checking revision
and eligibility. It copies source and asset identity only when an actual compile
or capacity retry is admitted; unchanged actors do not copy their source per
frame. Worker ordering and the lifecycle commit boundary remain unchanged.

Fixed-step time is not a wall-clock service guarantee. In the production GUI,
the time owner drains `FixedMain` synchronously before `Update`, up to 64
complete fixed cycles per app update at every transport rate. The virtual
clock admits the complete running delta even while a solver result is pending.
The coupling barrier gates fixed execution without discarding that demand;
user pause and preparation holds freeze pacing. Budget exhaustion leaves pending
duration in `Time<Fixed>::overstep`. Every step still receives the same
`Time<Fixed>` delta. Render transform easing bounds its interpolation fraction
to `[0, 1]` independently of that backlog: queued cycles cannot extrapolate
positions or rotations beyond completed endpoints. Typed scene-tool and menu hooks run in a separate bounded UI queue after picking in
`PreUpdate`, before that fixed loop, so they do not wait behind general REPL
requests or the current frame's fixed-step work. The UI and simulation still
share the GUI thread: a long tick or catch-up burst delays the next native input
poll and the next visible frame. Under sustained overload, retained simulation
demand can grow faster than the owner drains it. Discarding that balance would
drop authoritative ticks. `SimulationTimingProfile` is the shared,
bounded observation path until ownership is split: it reports the most recent
240 completed `FixedMain` tick service times and rate-derived service budgets,
plus per-app-update fixed-loop duration, completed-step count, remaining
pending simulation duration (whole ticks and fractional overstep), and
simulation-time demand omitted if a consumer alters `Time<Virtual>::max_delta`.
The profile is read through telemetry's query registry, does not emit per-tick
events, and never feeds simulation decisions. Its capped-time total describes
wall-time demand already clipped, distinct from the recoverable pending duration.
The time owner's configured admission produces zero clipped demand. The profile
still does not move fixed work off the GUI thread or guarantee real-time cadence.

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
scheduler. A script with an unsupported `@peer` target or timing is skipped
and receives a document diagnostic for that source revision; the host and its
other scripts continue. The owner must not guess a scope or move the script to
another clock. Runtime callback errors also remain visible and local to their
owner, with required authoritative hooks holding or faulting that owner.

Cycle labels establish ownership and ordering, but the runtime does not yet
provide a complete cross-owner duration, queue, and overload view. Owners with
measured hot paths should expose bounded aggregates at task boundaries rather
than retaining traces or emitting telemetry every frame. Use those owner metrics
to find a candidate, then profile the production path with the adjacent Tracy
workflow. Run a separate unprofiled pass for frame-rate acceptance; the profiler
capture is diagnostic evidence, not the performance number.

The live Modelica worker exposes per-participant step diagnostics through
`CosimStatus`: solver-step service duration, dispatch-to-response latency, and
the native worker's pending-task count when the solver starts. These are
recorded once per completed solver step and reset with the Modelica session.
`modelica_construction_profile` separates native construction queue wait from
solver construction, metadata collection and the initial checked observation.
The worker measures these at task boundaries and returns them through its
existing typed preparation lifecycle. The receiver applies the same session
and source-generation fencing as other preparation facts. Only an installed
compiled participant exposes the profile; unavailable queue measurements remain
unavailable. Logs and Modelica notices retain model-attributed construction facts
even when a temporary initialization participant is subsequently retired.
Successful coupled certification also retains those timings and worker-issued
solver identity in scene-scoped `last_coupled_initialization` facts. These
diagnostics change neither ordered admission nor simulation time; unavailable
measurements remain unavailable without blocking certification.
Set `include_values: false` when reading fleet diagnostics to omit complete
input/output maps and verbose model/error strings while retaining compact model
status, counts, and step metrics. Rhai profilers can also set
`include_entities: false` to receive the aggregate `modelica_step_profile`
without constructing per-participant strings.
Response latency includes worker queueing, transport, and owner response
handling; it is not a pure queue-wait measurement. Browser worker queue depth is
not currently observable and is reported as unavailable. Compare these values
on representative scenes before changing the serialized Modelica worker or
introducing a parallel solver; use Tracy to determine whether solver service or
other owner work dominates.

Deferred world-bound Rhai calls snapshot their typed `CommandOrigin::Rhai`
into `WorldScriptExecution` at admission. That origin carries the caller's
runtime context, actor, session authority and client-local restriction across
reflected dispatch as well as queue execution. UI/REPL queue membership is a
scheduling choice and cannot change the nested call's phase, clock, sequence or
producer stamp. Host submissions without a Rhai origin receive the owning
UI/REPL cycle context. Nested synchronous world scopes restore all caller
bridge state on exit, including after an evaluation error.

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
no scene-bound Twin scenario route. Discrete Twin lifecycle callbacks carry the
stable owner ID and use generation zero until a scene generation exists.
Scene-owned scenario execution waits for the scene readiness gate. Independent
Twin and Application presentation programs can prepare, start and receive
events without a scene. They wait for their own declared prerequisites;
scene-bound and simulation-timed programs also wait for global scene readiness.
The scenario driver captures one `ScenarioLease` containing the exact Twin
owner and optional scene revision through compilation and dependency admission.
An absent committed generation stays `None` in that lease; only the existing
route/stamp ABI encodes that absence as generation zero. A committed Clear may
have a generation without a mounted USD payload. Scene mounting and lifecycle
revision are separate owner facts. Captured Twin identity and scene
revision must both still match before a worker result is consumed. Application
leases ignore scene revisions. A missing generation faults only when live
scene-owned work requires one after its execution gate has opened.

Scenario ownership is explicit on the host entity. `TwinOwnedScript` carries a
stable `TwinId`; `SceneOwnedScript` marks a program projected from the current
USD scene. The two markers may coexist. An unmarked application scenario has
neither marker, so a Twin transition cannot restart or close it. Core Rhai
policy runtimes also remain outside the scenario driver's Twin teardown.
`on_stop` uses the stored outgoing Twin ID before its source document and asset
handles are released. Twin tutorial assets use isolated hosts, preventing their
replacement from stopping an application scenario on `WorldRoot`.
An isolated host created by `RunScenarioAsset` carries
`Provenance::Authoritative`. The session identity-admission owner assigns its
global identity, and the API registry indexes it before declared simulation
reads execute. A Twin ownership marker alone is not an actor identity: without
the admitted identity, the Rhai `me` sentinel is zero and physics access cannot
resolve the calling scenario. Retaining a scenario across replacement retains
that host identity while rebinding its scene dependencies.

Systems declare their cycle and read that cycle's clock. A synchronous helper,
Rhai function, or nested registered hook inherits the caller's context. An
event retains its producer stamp while its consumer also knows its execution
cycle. A discrete callback must not infer its timing from whichever Bevy
`Time<T>` happens to be accessible. `lunco-runtime-context` owns these typed
values, while `lunco-core` supplies the Bevy `RuntimeCycleSet` labels. Rhai gets
a read-only `execution_context()` view beside the existing `clock_snapshot()`.
Registered hooks receive one typed `HookInvocation`; isolated Rhai hooks expose
its context as an immutable `runtime_context` map, and native providers receive
the same map through ABI v2. Before a registered hook runs, the shared hook
registry validates that a selected clock matches the owner cycle, clocked
contexts carry a finite non-negative time sample, discrete contexts carry no
elapsed sample, and Core/Application routes use generation zero. Invalid
context is a hook error; the registry does not guess or replace the clock. A
Rust hook owner with no classified cycle uses
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
the async startup path and live projection invoke the same policy contract.
Immutable prepared plans run full graph extraction, Rhai policy, and source
validation on workers. A live canonical OpenUSD reader stays main-thread-owned;
its typed network snapshot is sent through the bounded worker admission path,
which performs policy and source validation off-cycle. Completed networks
publish in request order, at most one per Update, after matching the Twin,
canonical USD generation and exact admitted source revision. Both shipped synthesis
policies reject calls from scenario, UI, or REPL cycles.
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
the cycle check. `dt()` reads the configured `Time<Fixed>` timestep, including
lifecycle initialization at SimTick zero; an unadvanced delta is not the
simulation's configured integration step. The shared Rhai value bridge keeps
unsigned counters and identities typed and compares them exactly with signed
integer literals, without conversion through floating point.

The scene-test runner checks the runtime owner's terminal fault after every
preparation update as well as at stepped completion. Both paths use the same
expected-fault classification and verification-report owner; a startup fault
cannot become a readiness timeout or stand alongside a passing verdict.

The scenario owner currently supplies this context for fixed scenario passes,
paused lifecycle/event passes, and one-shot REPL/tool evaluations. Fixed passes
use the simulation tick and fixed-step clock; paused passes have no elapsed-time
clock or consumer sequence and preserve the telemetry event's simulation
producer route and tick. Event RNG uses that producer sequence even when its
callback runs in a later cycle; discrete lifecycle hooks use a stable
sequence-free seed.
Nested Rhai functions inherit the current phase. Runtime owner scope
(`Core`, `Application`, or `Twin`) is assigned by the Rust owner and remains
independent of cycle and clock. The authored `@peer host|client|both` directive
selects the network peer for a scenario; it does not select the runtime owner
scope. Continuous scenario behavior may state its required
cadence with `@timing simulation` (the default) or `@timing presentation`.
Both use one lifecycle FSM and native task interpreter. Presentation behavior
receives `Time<Real>` frame samples and a frame sequence; waits use that typed
context. Its independent bounded event queue prevents a paused simulation pass
from consuming presentation events. Subscription exists only while the language
has presentation actors, avoiding event clones in simulation-only hosts.
While the scene gate is closed, discrete events enter only the presentation
queue. No simulation-event backlog accumulates across that gate.
Presentation programs submit typed commands; direct world writes are rejected.
Rust installs and invokes both cadences from their owning schedules. Lifecycle
hooks receive the lifecycle, presentation or simulation
context of the pass that invokes them. Unknown execution targets, timing values, or
metadata directives skip only that scenario and publish document diagnostics
once for the source revision. The scenario driver refreshes these directives
from the current document generation, so a source edit cannot keep stale
routing state.

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
A reference batch on the primary scene shares its document-projection key from
closure preparation through native authoring and active-root ECS projection.
Preparation reads the current asset store and load state as well as lifecycle
messages. Failures remain visible at the projection owner. Preview and additive
documents retain their own projection lifecycle and cannot hold the primary.
Persistent edits to the mounted primary document also acquire a coalesced
`UsdDocumentProjection` key as soon as the document registry revision changes.
Change detection admits edits from UI/command cycles in the shared
`SimulationProgressAdmissionSet` after entity indexing and before the `PreUpdate`
simulation-admission boundary, and admits edits issued inside a fixed
Rhai/event pass in the same lane in `FixedLast`. The fixed runner completes the
cycle that issued the edit, then stops before another one while preserving any
remaining catch-up time in `Time<Fixed>::overstep`. The key remains held until the exact
or newer document generation is reflected in the ECS projection cursor;
view-layer edits whose typed operation suffix is available do not hold
simulation. If that suffix is unavailable, the owner conservatively admits the
coarse projection as causal work. The steady-frame path does not poll document
contents. Any `FixedLast` owner that may discover new causal work must publish
its hold through `SimulationProgressAdmissionSet` before the clock closes the
fixed-loop burst.
The production `route_lifecycle` Rhai gate covers both edit origins: a fixed
simulation hook admits referenced prims, and an Application/Repl evaluation
admits a new referenced prim through `ApplyUsdOps`. The Repl check confirms the
public entity index has not caught up immediately after the document edit,
observes simulation paused while that projection is pending, and requires the
first later simulation tick to see the projected prim. It also checks the
Application/Repl execution context and the event's producer tick.
An inactive or removed primary root releases its exact key until that operation
has faulted. A primary closure or projection failure records a `RuntimeFault`,
a path-addressed diagnostic, and a persistent progress hold; scene teardown
clears both before a replacement scene runs. `UsdSceneRuntimePlugin` installs
the progress resource it needs, so selecting that capability is sufficient.
The Modelica execution owner also reconciles active, causally required
participants before the simulation-admission boundary. A participant whose current session still
needs compilation owns one `ModelicaPreparation` key; compile intent is admitted
in the lifecycle cycle, which continues while `Time<Virtual>` is held. The
worker result is committed by the Modelica response handler before the next
lifecycle pass releases that key. Concurrent requests with the same prepared
solve key share one immutable worker result, but retain their individual IDs in
the ordered commit queue. Intentionally paused and noncausal models do not hold
world time. This gate covers prepared solver state; the first normal
co-simulation step remains governed by the per-step barrier after activation.
A terminal Modelica error also releases the preparation key, while the
readiness owner keeps the failure visible as `program_failed` and applies its
configured failed-participant action.
This includes a USD program rejected before it has a `ModelicaModel`: its
`SimComponent::Error` is already a terminal preparation fact. The USD readiness
producer derives one ticket per program from the current solver or pre-solver
state. Removing the program closes that exact ticket even when its physical
subject survives; removing its source components also retires the ticket.
This adds no policy hook: `lunco-readiness` still owns `readiness.action`, its
typed facts, installation and failure semantics, and `ignore_failed_models`.
The `program_attach_command` production Rhai gate observes valid admission,
pre-solver rejection, its named readiness fact and removal recovery.
An initially discovered USD Modelica component network owns a separate
`UsdDomainProjection` key from root discovery through source-class resolution,
synthesis, and publication of its `SimComponent` port surface. Its binding
epoch remains open for that interval. The key is released at the same
`SimulationProgressAdmissionSet` boundary where the Modelica participant's
`ModelicaPreparation` key is admitted, so `Time<Virtual>`, `SimTick`, Modelica,
Rhai, and physics resume together only after the generated interface exists and
any causally required Modelica compile has settled. Live replacement keeps the
installed interface active while preparing its replacement; it does not reopen
initial-scene admission.
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
5. Wait for every active participant due at this communication point. Validate
   session, source revision, step, and endpoint before crossing the next fixed
   boundary. A slower declared period holds the last committed output between
   points; worker completion timing never selects an authoritative input.
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

External `SimulateIntentEdge` and `SimulateIntent` commands targeting fixed
simulation state enter the bounded `lunco-core-session::PendingSessionInputs`
queue. The session owner assigns the next fixed tick and a shared per-tick
sequence, then validates scene generation and stable target identity, captures
the record, and publishes its typed commit event before fixed-step producers
run while simulation time is running. A pause leaves admitted inputs queued for
the next running fixed tick. The controller applies semantic actions at that
boundary. The held-state commit publishes `intent.hold`; edge delivery also
records its `CausalTrace`.
Raw-file runtime `SpawnEntity` requests use the same queue and order allocator.
Their records retain producer provenance, correlation, stable scene-root and
active-frame identities, original f64 pose, and a reserved root identity. The
scene-command owner revalidates those facts and commits the spawn at its
assigned tick before identity admission; `NetSpawn` uses the reserved identity.
Spawn identity collision checks query the lifecycle-maintained
`ApiEntityRegistry` and a hash index of queued runtime roots, so admission does
not walk all live entities or scan the pending input queue. The API registry
updates on `GlobalEntityId` component insertion, replacement, and removal.
Document-backed spawns remain `ApplyUsdOps` entries in the Twin journal and do
not also enter the session stream. Physical-frame snapshots are sampled and
captured at their consuming fixed tick with the same per-tick allocator after
queued session events. Simulation-clock Rhai actions stay in their derived
behavior pass, and local-embodiment input stays on the interaction cadence.
Live port-inspector writes and releases use the same queue with a `LocalUser`
producer identity. `PanelCtx::trigger_command` scopes that session origin over
deferred workbench dispatch so the cosim owner can admit and capture each
action. `ControlAuthorityChanged` admits an internal `ReleaseControlInputs`
lifecycle event for each live released endpoint. It joins the shared input
queue for the next fixed tick and clears controller-owned input holds and
simulated intents while preserving authored program setpoints and physics state.
Previously admitted inputs retain their tick and sequence and commit before
the release. The session owner captures the lifecycle record at that commit
when recording is active. Missing target, generation, tick, or order state
reports a runtime error and leaves existing holds intact. The release does not
write replacement endpoint values or modify velocity. Twin policy writes any
stop setpoint explicitly through `SetPorts`.
Unclassified direct port events remain outside this stream.

## 4. Async preparation, priority, and result commit

| Domain/work | May run async | Must run at the owner boundary |
|---|---|---|
| USD | Asset I/O, dependency discovery, immutable layer parsing/composition, and send-safe projection-plan preparation; dependent-stage document layers compose directly to shared `Sdf Data` for the live patch, then changed bytes and the future-mount plan are prepared at background priority; simulation topology classification runs from an exact canonical recipe snapshot through shared bounded admission | Check source revisions, operation/revision, and target-plan identity; mutate the live, thread-affine stage and publish ECS projection in stable scene order. USD simulation topology holds its mounted root's exact progress key until a current-generation index is committed; an active dependent-stage refresh holds only through the live patch and ECS handoff |
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

Document startup captures codec-correct root bytes and separate runtime/view
session opinions on its preparation worker. The Twin projection lease owns
publication of that immutable recipe and retains its asset handle; loader
completion checks that the captured input is still current and live. The native
stage remains thread-affine. Incremental replay and deferred reference
materialization select the operation's native edit target with a scoped OpenUSD
edit context. A replacement's root notice enters native asset admission before
ECS reset, projection, and document synchronization acknowledgment.

The USD simulation projector captures the native root/session identities and
admitted dependency buffers, plus separate `Sdf Data` copies of layers changed
since admission. The canonical stage's change sink owns that changed-layer set;
unedited binary layers are not decoded for capture. OpenUSD stages remain
thread-affine. Native encoding runs on the admitted worker with the codec
selected by each original layer identifier, followed by composed joint,
wheel-attachment, vehicle, and simulation-candidate preparation. No layer merge
or text-under-binary substitution crosses this boundary. A canonical stage
without an admitted resolver closure rejects capture explicitly. The mounted
root holds one `UsdSimulationTopology` progress key only until its first
topology index is committed, so startup cannot begin simulation with missing
facts. Later generations retain the last committed index while replacement
facts are prepared; existing simulation keeps advancing and new or reprojected
simulation prims stay queued until commit. Results are accepted only when the
stage-asset plan identity and canonical generation still match. A current
primary-stage preparation failure, including an unavailable worker transport
on browser hosts, faults and safety-holds that scene; scene teardown releases
the initial progress key.

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
results at their own boundary. `PostUpdate` starts queued native preparation;
worker and external-permit completion refill released capacity immediately on
the existing async-compute pool. Queue selection and slot reservation share
one short critical section, preserving the global bound and priority service.
Jobs are spawned after releasing that lock. The queue owns its limits;
completion guards retain only a weak queue reference. Resource retirement stops refill and
drops queued captures outside the lock, including captures that own permits.
Execution can advance between host frames; simulation admission, scope, clock,
source validation and ordered result commitment remain at the domain owner.
Modelica document parsing, inline Rhai roots and
source-matched import misses, Twin SysML source-set analysis, and default USD
Twin source parsing plus persistent-source serialization use this path. USD
source parse results are checked against the current asset text before the
registry applies path identity and dirty-document policy. Persistent snapshots
are cloned after runtime-overlay restoration and serialized away from the main
schedule; the owner commits the Twin overlay only if the document generation
still matches. Runtime sidecar reads/parsing remain synchronous, and this USD
path reports the missing worker transport on wasm rather than blocking the page.
Modelica parsing derives simulation demand from exact document-registry links
to non-editor participants. Those documents use `SimulationRequired`, the
focused editor document uses `Interactive`, and other documents use
`Background`. Stable priority/document/generation ordering also applies before
the Modelica owner's local in-flight bound. Editor focus therefore cannot
demote a source required by scene admission. Queue wait is measured separately
from parse/install time; result validation and ordered publication retain their
existing owner boundaries.
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
Application roots and Twin roots have separate owners. Twin root identities
include the stable Twin ID, and `TwinClosed` submits unload operations through
the same actor FIFO. Monotonic per-root operation IDs fence late file reads,
queued installs, and unload completions; removing a root recomputes the
compiler's effective root set and defaults from the remaining contributions.
If the Modelica channel is temporarily unavailable, unloads remain queued ahead
of later root installs. Application roots stay installed across Twin closure.
Immutable DAE lowering and persistent solve-cache reads, decoding, encoding,
and writes run on a bounded preparation pool, isolated from the persistent
scene and document construction/execution owners. The typed compile command
captures its actor's `ModelicaExecutionDomain`; cached rebuilds preserve it.
The document dispatcher uses the existing document-actor marker, while USD
participants admit the scene domain. This partitions execution capacity without
changing clocks, runtime source attribution or authoritative result ordering.
Native sessions are constructed, stepped and retired on their assigned thread;
the command owner receives typed handles and checked observations. One complete
transaction per initialized entity can execute concurrently within a round, but
results admit in round order with unchanged communication points and micro-steps.
Actor admission is bounded across submitted compiles, root installs, lowerings
and constructions to the largest pool count plus two
staged operations. This lookahead keeps the serialized Rumoca owner preparing
later independent programs while the pool lowers existing DAEs, while keeping
retained compilation state bounded. Solve results still commit in submission
order.

The native execution host captures `lunco_modelica_worker::worker::ModelicaWorkerLimits`
before launching its worker. `preparation` and scene `live_execution` each use a typed
`WorkerThreadBudget`: at most six threads, subtracting two reported CPU threads
before applying the cap, and retaining one worker on small hosts. The defaults
include one `document_execution` owner, admitting up to thirteen threads. Hosts can tune
each budget independently before installing `ModelicaExecutionPlugin`; changes
after launch do not resize the pools. Zero capacity or unavailable host CPU
capacity fails visibly before the pools start. Preparation cannot block a
live mailbox through cache I/O, source parsing or lowering. Document construction
cannot block a scene mailbox. Construction can share a thread with other sessions
in its own domain; scene topology admission retains its existing simulation hold.
Pool capacity does not change clocks, micro-step math
or authoritative commit order.

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
worker owner validates the pinned document runtime lifetime, resolves the current
document snapshot and same-owner sibling overlays, then dispatches the compiler
command without UI resources; the UI command only resolves the
selected class and publishes intent. Solver stepping remains in
`spawn_modelica_requests` inside `FixedUpdate`. Because this request is emitted
only for an unpaused model, it carries resume intent through compilation so a
successful first compile does not leave the model paused. The compile request
can therefore be dispatched while `Time<Virtual>` is held. Compile-result
commit validates worker session, pinned runtime lifetime, and captured document
generation. Session epochs persist on the entity across participant retirement,
so an old response cannot match a recreated Modelica component. If the
document changed during compilation, the old result is discarded and an active
model remains held with its run intent for the current revision. The scene
admission hold still needs to include reference closure, Modelica preparation,
Rhai activation, and physics readiness in one transaction.

Successful Modelica worker results carry `ModelicaLiveSolverSnapshot` from the
same resolved `LiveBuildPlan` that constructed the stepper. It retains the
solver id, declared capabilities, live/predicted profile, exact backend-neutral
solver parameters, and sorted instance parameter overrides. The response bridge
stores it on `ModelicaModel` after worker-session fencing; new compile results
also pass the source-generation check, and failed compile or step results clear
the snapshot. The existing `compiled_generation` remains a separate source
revision fact for a baseline collector to compare against the current document.
A session-wide baseline collector and initial authoritative-state snapshot
still need to consume these owner facts.

Generated domain projection follows the same path: it publishes the validated
source and interface with the exact source mount's runtime lifetime. The document
owner links and registers that attribution before the next lifecycle admission
emits `CompileRequested`, including sources from replicated scenes. Projection does not
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
a worker. The `twin.lifecycle` hook receives `Twin/Lifecycle` with generation
zero and the mounted `TwinId` in `owner_id`, no elapsed clock, and an explicit
`Start`, `Event`, or `Stop` phase. Its retained `policy_status().lifecycle.runtime_context` makes
the owner stamp inspectable from Rhai and the API. Identical Rhai source misses share one
immutable compile result. Rhai drains worker results into a scene-wide
preparation barrier and commits the complete ready set in stable actor order
before the fixed runner admits simulation time. Its exact progress holds remain
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
simulation progress. Terrain cover preparation and native tile-mesh baking use
shared admission. Remaining USD and Modelica library preparation remain
owner-local or synchronous. On wasm, native admission rejects CPU work until a
Web Worker transport exists; terrain cover preparation remains explicit and
synchronous on that host, and tile bakes retain their explicit browser cache
path.

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

The language-neutral telemetry inbox is bounded and drained at the scenario
pass boundary even when no script actors exist. Idle traffic is unobservable
by actors created later and is discarded without resetting its sequence or a
latched overflow fault. A driver with no actors preserves events when another
language has a potential consumer. Overflow remains a scene-scoped error in
`RuntimeDiagnostics`, holding event delivery until scene teardown; continuing
physics and green readiness alone do not establish a healthy event stream.

Rhai inline-root parsing and immutable compilation artifacts are prepared
through shared admission. File-backed source assets are parsed in Bevy's async
asset-loading task and publish text plus source-matched AST at the asset
boundary; tools and prelude installation reuse that AST instead of compiling on
the update thread. The owner buffers completed artifacts until every
currently admitted scenario compile is ready, then commits by stable actor
identity before the fixed runner admits simulation. After the scene readiness gate opens, the
scenario's progress hold remains through dependency planning, top-level
initialization, and the first `on_start`, so fixed ticks wait for activation.
Scene and physical admission do not consume fixed ticks; the process-wide
`SimTick` remains monotonic across scene epoch changes. Scenario `this` state and live-world calls remain
owned by the script activation/execution boundary. A paused Update
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

A compiled scenario may also define `on_visualization(me, ctx)`. The owner calls
it once in `PreUpdate`, after the lifecycle, document, and terrain admission
scans and time-spine projection, but before the first `FixedUpdate`. The owner
routes it through the Twin `Visualization` cycle after scene, reference,
document, and terrain preparation holds clear. Modelica holds do not block
this presentation path. Its execution context has the presentation clock, no
simulation tick, and the visualization phase. The callback may prepare
disposable visual views; authoritative actions, top-level initialization,
dependency planning, and `on_start` remain behind the simulation admission
boundary. Rhai mutation guards reject world writes and events in this phase;
only `ApplyUsdTransientOps` is available, and the USD owner directs those edits
to the disposable view layer. A new source or scene generation admits one new
visualization call.

Before a scenario's first lifecycle hook, a source may define the optional,
scenario-scoped `simulation_dependencies(me, ctx)` hook. The second argument is
the validated scenario parameter map. It returns a map with
`modelica_entities: [global_entity_id, ...]`,
`entity_reads: [global_entity_id, ...]`,
`entity_writes: [global_entity_id, ...]`, and
`query_reads: ["PublicQueryName", ...]`, plus
`required_inputs: [#{ owner: "domain.owner", identity: "stable-key" }, ...]`.
All five arrays are required even when empty, so an incomplete source cannot
silently run with missing causal edges. The owner resolves every listed id
against the live registry. Directional access arrays can name any live entity.
Every active Modelica participant already shares the fixed-step barrier;
a declared id grants the scenario access to that participant.
`modelica_entities` remains the direction-independent dependency for
participants whose ports or events the scenario uses. Unresolved ids are
terminal source diagnostics. Each required input
names an owner registered in `SimulationDependencyStates`. A missing owner is
a terminal diagnostic; an absent or Pending key from a registered owner keeps
the scenario's existing activation hold until the next owner-state revision;
Ready admits initialization, and Failed reports the producer diagnostics.
The dependency plan runs once per source/parameter revision, and readiness is
checked again only after an owner publishes a change. Omitting the hook declares
no Rhai access dependencies; the shared simulation clock still applies. The hook resolves
identities from its `me` id, parameters, and stable owner metadata. The built-in
`usd_path(id)` reads the stable identity-to-`UsdPrimPath` index, so it can resolve
the scenario host path during planning without reading pose or component state.
Commands, direct mutations, emitted events, and live entity or port reads are
rejected in this phase. While a plan is pending, all Modelica participants remain
synchronized and simulation time
stays held by the scenario's exact preparation key. The production sensor scene
verifies declared Modelica reads, rejects a live port read during planning and
a live non-Modelica id before `on_start`, and proves that one scenario cannot
read, write, or consume events from a Modelica participant declared only by
another scenario.
This hook runs before mutable top-level initialization, so derive its result
from `me`, scenario parameters, identity metadata, and permitted scene-generation
queries rather than top-level initialization effects. Once all declared inputs are Ready, the owner
commits the Modelica participant set, runs top-level initialization in the
`Initialization` phase, and dispatches `on_start` in stable actor order. The
top-level body runs with the scenario's stable entity identity and deterministic
initialization random stream, so its `query_reads` declarations authorize the
same broad providers used by later scenario hooks.
After an ordered simulation command materializes a new entity, the caller can
extend its committed directional plan with `track_entity_read` or
`track_entity_write` before accessing that live id. A tracked id that is or
becomes a Modelica participant retains the same shared clock; tracking grants
scenario access without changing its timing contract.
The update occurs synchronously in the caller's serialized simulation order.

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

Rhai task trees are the reusable sequencing mechanism for authored behavior
and ordered asynchronous checks. `wait_for`/`wait_for_from` suspend on an
owner-published event; `wait_until` samples a predicate at deterministic task
cadence when no suitable event exists; `wait(seconds)` advances against the
simulation clock. `reactive_seq([check(guard), body])` rechecks a cheap guard
each task pass and cancels the running body when the guard fails. An event
handler may update the guarded state, and the task observes that change on its
next owning-cycle pass. Tests may keep a bounded fixed-step `on_tick`
watchdog to produce a useful timeout verdict. Do not add an app-global timer
that invokes Rhai callbacks outside their owning cycle. A future reusable task
deadline, if needed by production behavior, must name its clock and define
cancellation and failure semantics in the task tree.

## 6. Co-simulation and dependency closure

Every active Modelica participant synchronizes at its declared communication
points, regardless of whether USD wiring reaches a native force consumer.
Different rates change the due set within the same fixed master clock. A
scenario that needs direction-independent access lists the live participant in
`modelica_entities`; directional `entity_reads` and `entity_writes` grant only
their named direction. Rhai chooses access policy and Rust validates the ids.
Required owner inputs use the same plan. Scenario preparation retains its exact
progress hold until its access and readiness plan is committed. Direct
simulation access to undeclared state fails at the scripting owner. Presentation
reads observe committed state without changing participant clock membership.

The shared barrier synchronizes solver work; it does not grant script access.
For Modelica participants, the scripting owner checks port reads through
`get`, `port`, and `ReadPorts`, port writes, targeted commands, and event
delivery against the calling scenario's own committed dependency plan.
API query providers declare how their simulation reads are accounted for.
Entity-targeted providers report stable ids and the bridge checks each id
against this scenario's `entity_reads` or Modelica dependency. Mounted-scene
queries without an explicit document id are bound to the active committed Twin
generation; an old route or an in-progress scene transition is rejected.
Broader providers use their public query name in `query_reads`, which declares
the provider's full owner snapshot as a coarse read dependency. Unknown
providers default to this conservative scope. Broad queries are rejected while
`simulation_dependencies` is being resolved, because that plan has not yet been
committed. This covers
`QueryEntity`, `QueryPhysicsState`, `ReadPorts`, `GetPort`, `SolarPose`, and
single-entity `ListPorts` calls. Provider-wide declarations do not identify a
spatial region or make live-world hooks safe to parallelize; current stateful
hooks remain serial until they read immutable snapshots and publish ordered
typed actions.
Directional `entity_reads` and `entity_writes` authorize direct reflected
and port access; the direction-independent Modelica list authorizes both
directions. A Modelica entity in either directional set also contributes to
the scenario barrier. A participant in USD wiring or another scenario's
plan still must be declared by the scenario that consumes it. Directional
sets add barrier membership only for ids that are current Modelica
participants; they do not make live-World evaluation safe for parallel actors.
The typed action/replay path and finer spatial-region declarations remain open
dependency surfaces.

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

Authored joint readiness has two distinct facts: resolved body topology and
Avian solver-island admission. Initial-pose validation waits only for the
topology projection, and uses the same joint-pair collision exclusions as the
runtime solver when checking support contact. Avian island admission still
waits for the validated initial pose. This keeps startup validation from
waiting on an admission step that itself depends on validation.

Raycast support footprints are validated against the composed Avian colliders.
The initial probe uses the hit collider's world normal to identify its support
surface; it does not depend on `LocalGravity`, which is produced by the first
fixed simulation cycle after admission. This keeps authored-pose validation
inside lifecycle preparation without inventing a gravity sample or advancing a
physics clock to obtain one.

The articulated rigid colliders are also checked against live static support
colliders with Avian's exact narrow phase. This catches an authored rover whose
axle probe starts inside a ramp and therefore produces no ray hit. A measured
overlap follows the same initialization policy as other support penetration;
the owner pauses the assembly in place and publishes its diagnostic before
dynamic admission, and sends a `PHYSICS_INITIALIZATION_PAUSED` warning to
Recent status.

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
- fixed-step controller inputs: target `GlobalEntityId`, then local input
  `SessionId`; missing target/session identity or committed scene generation
  holds the input with a structured runtime error, and duplicate target/session
  keys are rejected instead of ordered by Bevy `Entity` bits;
- telemetry delivery: simulation tick, source, name, severity, time, and a
  recursive order over the typed payload;
- USD-connected events: instance namespace, authored event prim path, source,
  and event name;
- USD simulation projection and dynamic-body admission: stable logical stage
  source, optional authored instance-root path, then authored prim path. Bounded
  projection prefixes, topology-task ranks, and body promotion never use Bevy
  entity bits or asset allocation ids; missing or ambiguous identity raises a
  runtime fault and keeps projection queued or bodies kinematic;
- serialized Modelica commands: `GlobalEntityId`, then the world-local entity
  key for local-only models;
- API entity batches use ascending `GlobalEntityId`; scalar first-match
  lookups choose the minimum stable identity; nearest-query ties use distance
  then `GlobalEntityId`, and radius-query ids are returned in ascending
  identity order;
- connection reductions: stable connection identity before floating-point
  accumulation.

The world-local entity key makes execution independent of ECS query layout in
one running world. It does not provide cross-session replay identity. Any actor
or model included in a cross-peer/replay guarantee needs its stable
`GlobalEntityId` or another source-owned, replicated identity.

External held-input changes, discrete edges, live `SetModelInput` changes,
lifecycle input-hold releases, and raw-file runtime spawns share the bounded
session-owned ingress queue and assigned per-tick sequence. `SetModelInput`
identifies live participants by
stable `target_gid`; editor-only documents keep their document selector. When
applying a live Modelica record, the commit reads the target's `ModelicaModel`
identity, as admission does. A USD-projected model need not have an editor
document registry link; that registry does not authorize live input commits.
While capture is active, `SessionInputStream` retains their typed payloads,
correlation ids, producer class and stable caller ID, target, committed
generation, tick, and sequence. Spawn records additionally retain the scene
root, active frame, catalog entry, exact `f64` pose, and reserved spawned-root
`GlobalEntityId`. API and direct typed commands require a nonzero
`producer_id`; actorless Rhai requires one, while Twin Rhai retains its route
and actor identity. Modelica records retain the declared input name, exact
`f64` value, and command correlation. Document-backed spawns remain
`ApplyUsdOps` in the Twin journal. A `ControlInputsReleased` record identifies
its endpoint and `runtime_lifecycle` producer, joins the shared ordered queue
for the next fixed tick, and clears local input holds after previously admitted
inputs. It does not write endpoint values; authored Twin policy owns stop
setpoints through explicit `SetPorts` commands. The fixed-step controller captures physical
`ActionState<UserIntent>` into a
by-value `PhysicalIntentFrame` semantic snapshot. When the controller and
target have stable `GlobalEntityId`s and a committed scene generation, the
fixed-step owner stamps the frame with those identities, the current `SimTick`,
and the shared per-tick input sequence before combining physical and simulated
holds for control translation. Capture also retains that frame as sorted
canonical intent ids with its producer session and admission stamp. The shared
stream has a record bound, validates canonical payload names, stable stamps,
and producer/payload pairing before retention, and stops visibly on invalid
input, overflow, or invalid order. It is observable through
`ReadSessionInputStream`. Physical frames are admitted at their consuming
controller boundary and cannot be deferred by the external queue. A completed
capture can be exported on native hosts with `ExportSessionInputCapture`:
immutable records enter the shared background admission queue for versioned
encoding, then `lunco-storage` writes and reads back the bounded archive on
Bevy's I/O pool. Each capture has a monotonic app-local identity and can be
durably exported only once after a successful write; failed writes may be
retried. The typed export status reports the capture and export identities plus
the read-back result.
Archives are capped at 65,536 records and 16 MiB and stored under the app's
user-config session-captures directory. The archive still has no baseline
manifest or playback consumer, so it does not provide whole-session replay.
Other typed commands remain outside capture.

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

The application-owned `camera.default_presentation` hook now receives an
`Application/Presentation/Preparation` context with elapsed and delta samples
from `Time<Real>`. Both standalone-camera selection and authored-camera
validation use this owner context, and the Rhai policy rejects other cycles.
Their `RuntimeCycleSet::Presentation` membership labels ordering; these systems
still run in `Update` and do not have a separate presentation cadence. UI and
terrain LOD remain shared-cycle work, so independent presentation/visualization
cadences are still open.

The rendering-quality catalog owner invokes its deterministic profile hooks
from `Application/Presentation/Initialization` at startup and
`Application/Presentation/Preparation` on stale policy revisions, sampling
elapsed and delta time from `Time<Real>`. The refresh is labeled with
`RuntimeCycleSet::Presentation` but remains in `Update`. The render recovery
owner invokes the non-authoritative `render.shadow_quality` warning policy as
`Application/Presentation/Preparation` in `PostUpdate`, only when the shadow
configuration changes; a focused owner test validates the supplied context.

The runtime UI recording-contract selector now invokes
`runtime.ui.recording` as `Application/Ui/Preparation`, with the Application
clock sampled from `Time<Real>`. It runs in the UI owner's `PostUpdate` chain
only when the exposure, policy, or recording revision changes; it does not
create a second UI cadence. Its Rhai policy rejects off-cycle calls.

Runtime-surface policies consume immutable snapshots prepared by the shared
headless/windowed exposure owner. Native pure policy evaluation runs through
bounded async admission; the page host uses the same kernel with cooperative
local task execution, without claiming browser worker isolation. Captured hooks
retain the caller's typed execution context. Completion servicing runs every
application update independently of the real-time capture cadence or physics
holds. One in-flight job and one latest pending input per surface bound work;
source epochs and identity checks reject retired results. Publication changes
only non-authoritative presentation state and never advances simulation time.
Live-World scripts remain serial. The complete state and failure contract lives
in [runtime UI architecture](runtime-authored-ui.md); the production
`runtime_ui_preparation` gate covers held-physics failure and recovery.

The whole-simulation guarantee remains open because:

1. Scene start composes owner-specific deterministic boundaries rather than one
   global readiness bit. The root USD loader fetches and composes its available
   dependency closure before publishing the stage asset; incomplete initial
   closures are admitted through the required deterministic
   `usd.scene_composition` Rhai policy after structural projection and before
   the lifecycle progress key releases its hold. An incomplete closure on a
   newly mounted reference reaches the same policy before its plan or layer
   bytes are committed; rejection leaves a primary reference unprojected,
   faulted, and progress-held. Active primary references and causal Modelica
   compilation hold exact `SimulationProgress` keys; the USD
   terrain bridge and progress scan run in `PreUpdate` before `SimulationAdmissionSet`,
   including a pending Twin manifest scan, so authored terrain data and
   collider work hold entity-keyed `SimulationProgress` before the first
   eligible fixed tick and through the web worker's coarse-to-full result; physics
   admission publishes a scene-scoped progress key while body and joint state is
   pending; and scenario startup waits for scene time, readiness, and external
   progress holds. The USD bridge seeds authored body poses in `PreUpdate`.
   A failed root-stage load invalidates mount ownership before clearing the
   projected scene. Production scene tests declare an expected failure through
   Rhai `expect_scene_load_failure(path, detail_contains)`; the runner observes
   the typed terminal event and checks that `SceneLoadInFlight`, the failed-load
   marker, active transition, mount root, progress hold, and projected prims
   are gone before accepting the scenario's verdict.
   Scene tests pump materialization, participant compilation, and physical
   admission with a zero-duration manual clock. Compilation publishes each
   Modelica solver's initialized time-zero state; preparation does not run a
   Modelica exchange or advance any shared simulation clock. The runner checks
   `SimTick=0`, zero fixed elapsed time, and zero fixed overstep before opening
   the scenario lifecycle gate. Rhai `on_start` therefore runs at global
   `SimTick=0`; the first live Modelica exchange, physics integration, and
   behavior sample enter through the shared fixed tick at `SimTick=1`. The
   normal simulation barrier holds later clock progress until worker results
   arrive. The production `sensor` gate checks this zero/one boundary, actor
   ordering, first-sample validity, and the initial altimeter miss.
2. Dynamic references on the mounted primary scene hold admission through
   closure preparation and live projection. Their incomplete closures use the
   same Rhai composition decision as initial scene closures before live-stage
   mutation; rejection or policy failure keeps the affected document hold and
   faults the primary simulation. Preview and additive mounts remain
   independent. Initial USD composition dependencies are fetched and composed
   by the root asset loader before the stage asset reaches structural
   projection.
3. Rhai Modelica port and event access requires the calling scenario to declare
   each participant in its own `simulation_dependencies` plan; aggregate
   barrier membership from USD wiring or another scenario does not grant access.
   A complete typed action path for every script write remains open.
4. Twin `AnalyzeSysml`/`ValidateSysml` queries now read committed async source
   snapshots; read-only analysis uses interactive admission and does not hold
   simulation time. Scenario plans can now wait on generic owner-published
   readiness keys without polling or making editor analysis a global hold. The
   production `sysml_async_analysis` scene verifies that a declared Twin
   analysis dependency admits the script only after its source-set snapshot is
   ready. Standalone SysML document analysis uses shared async
   admission and exact generation/origin fencing. USD file opens and confirmed
   discards parse on their async file tasks and publish immutable
   `PreparedUsdSource`; the registry retains identity, dirty-state, history, and
   owner-thread commit policy. Root Twin composition and live USD edit,
   journaling, and projection costs are separate paths; USD overlay
   serialization remains synchronous. Rhai module body evaluation remains on
   the owner lifecycle path; it can
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
   `PhysicsComputeProfile`; `clock_snapshot()` returns `physics_profile_known`,
   the optional `physics_compute_threads` value, fixed and physics clock deltas
   and elapsed time, pause state, and active `PhysicsHolds` reasons. During
   `FixedUpdate`, `physics_elapsed_s` covers the previous completed Avian step
   and equals `(sim_tick - 1) * fixed_dt_s`; a hold may clear `physics_dt_s`, so
   elapsed time is the cycle-count check. The production Rhai replay gate checks
   clock arithmetic within `1e-9` seconds and compares selected physics and
   Modelica state exactly (`numeric_tolerance=0`). The scene-test
   `--threads 0` profile uses Bevy's default `TaskPoolOptions`, matching GUI
   `DefaultPlugins`; the normal
   headless server entry point currently pins one Compute thread. Bevy's default
   assigns 25% of available threads to IO and AsyncCompute each, clamped to one
   through four, and gives Compute the remaining cores. Avian uses Compute for
   parallel broad-phase, narrow-phase, and constraint work. Broad-phase chunk
   results retain input order before contact-graph insertion; narrow-phase
   status bitsets combine before serial graph/solver updates; the active
   collision filter is read-only. USD projection supplies `PhysicsOrderKey`
   from the stable logical stage source and native composed prim path.
   Dynamic-body admission uses the same source/composed-path order and faults
   before promoting bodies when identities are ambiguous. The physics owner
   validates joint keys after Avian prepares solver data and before its substep loop;
   native joints, motor warm-start, custom prismatic correction, raycast
   suspension and tire forces, jointed tire forces, and raycast wheel
   mass-property folds use that key order. Missing, duplicate, or empty keys
   raise a runtime fault and hold physics. The production
   `multi_rover_stress.rhai` gate compares selected states directly against
   `scripts/tests/fixtures/deterministic-physics-reference.json`. The scene-test
   runner decodes that file into typed records at the
   `--determinism-reference PATH` process boundary and exposes one requested
   value through the existing Rhai query bridge. Rhai selects the profile from
   typed runner parameters, compares six physics checkpoints, sparse Modelica
   and articulated checkpoints, and the explicit final physics, Modelica, and
   articulated state with exact equality (`numeric_tolerance=0`). It retains
   only checkpoint tick numbers and the first mismatch. Physics pose rows use
   `world_pos`/`world_rotation`, backed by Avian `Position`/`Rotation` for rigid
   bodies; render `Transform` is excluded from initial pose checks and replay
   comparisons. The gate also verifies an
   altered physics row is rejected and emits no trace or result bundle.
   Reference t=0 state includes the checked causal samples and authored inputs
   admitted before solver construction. Its provenance identifies the artifact,
   input digests and paired Compute/jitter validation. A change to that admission
   contract requires captured current-state evidence and exact paired replay;
   reference replacement must not weaken tolerance or the negative control.
   `report_verdict` emits the status, check count, and authored failure
   messages in a typed telemetry map. The scene-test runner prints those
   details with its verdict and returns the matching process exit code, so
   failure diagnosis does not depend on Rhai log visibility. The Bash and
   PowerShell matrices in
   `scripts/test-deterministic-physics-profiles.sh` and
   `scripts/test-deterministic-physics-profiles.ps1` invoke production
   `luncosim test` processes and check their exit statuses. They leave
   `--tick-hz` unset, supplying one nominal physics interval per manual update.
   The production composition owns the fixed timestep in every host; scene
   tests change only manual wall-clock input duration through `--tick-hz` and
   `--jitter`. Update count and completed tick count are independent. Reported
   simulation duration comes from completed `Time<Fixed>` elapsed time, so a
   different driver cadence cannot relabel physics or Modelica time. The runner
   advances without wall-time pacing. This remains fixture-specific evidence, not
   whole-simulation replay determinism. The current profile
   records effective pool width but does not select a deterministic Avian
   solver profile; that guarantee still needs a measured production choice or
   deterministic reductions. In the same ordered
   `FixedUpdate` propagation path, a changed joint motor setpoint wakes its
   sleeping dynamic endpoint island with Avian's `WakeBody` command before the
   next solver step. The wake is edge-triggered by an enabled-state or target
   change, so steady repeated setpoints add no wake work.
6. The command journal does not yet provide a whole-simulation authoritative
   input log and replay verdict. Networking rollback's bounded per-vessel input
   frames retain ordered, latched `SetPorts` setpoints for owned-body replay;
   the fixed-step host consumes vessel frames in stable `GlobalEntityId` order.
   The session stream captures API and classified non-Simulation Rhai
   `SetPorts`, `ReleasePort`, and `ReleaseControl` commands, local
   port-inspector actions, explicitly identified direct typed port inputs, and
   live `SetModelInput` changes plus lifecycle `ControlInputsReleased`
   events at their fixed-tick boundaries. Lifecycle release clears local
   input holds; Twin-authored policy owns explicit stop writes. Unclassified
   direct `SetPorts`, `ReleasePort`, and `ReleaseControl` events,
   scene lifecycle, other authored commands, and all Rhai/Modelica runtime
   state still need
   capture or a replay owner. Adaptive Modelica is not a cross-machine bitwise
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
   authored policy is also inspectable in `Application/Repl/Evaluation`. The
   readiness selector receives `Core/Simulation/Behavior` from `SimTick` and
   `Time<Fixed>`. The camera presentation owner receives
   `Application/Presentation/Preparation` with `Time<Real>`. Render quality
   profile loading receives Presentation Initialization or Preparation
   context, and the shadow warning policy receives Presentation Preparation
   context from its PostUpdate owner. The runtime UI recording selector
   receives `Application/Ui/Preparation` with the Application clock.
   Runtime-surface visibility and property policies receive their typed UI or
   camera-status presentation owner context. The `usd.component_refresh`
   policy receives `Twin/Lifecycle/Preparation` with the active or committed
   generation for a mounted-Twin document, and
   `Application/Lifecycle/Preparation` for a preview-only document; an
   installed Twin policy requires its generation. Generic `invoke_hook`
   forwards the active execution context. UI and LOD still run on host-frame
   `Update`/`PostUpdate` schedules without independent cadence drivers, although
   server hosts now omit the visual plugin entirely.
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
    `Update`. The owner drains at most 64 complete cycles per update and retains
    remaining admitted time; a fixed delta preserves numerical step size but does not
    promise 60 wall-clock physics ticks per second or responsive UI during a
    long tick/burst. `SimulationTimingProfile` now reports bounded tick and
    fixed-loop service/budget samples and recoverable pending simulation duration.
    The separate max-delta diagnostic detects unexpected virtual-clock clipping.
    It cannot report wall-paced deadline misses and does not move fixed work off
    the GUI thread.
13. World-bound REPL requests use a bounded 64-entry FIFO, reject excess
    commands visibly, drain one request per `Update`, and cap each live-world
    invocation at 100,000 Rhai operations. This bounds interpreter work per
    application frame while preserving serial command order. A single native
    bridge call can still be expensive, and scenario hooks remain serialized
    in their owning schedules. On native hosts, terrain selection is frame-driven
    during capture while cover preparation and tile bakes use bounded shared
    admission. Offline capture holds virtual time at the advanced frame until
    terrain readiness clears; stable tile-order publication is bounded per
    `Update` by the configured terrain bake budget. Web cover preparation still
    lacks worker transport and remains a separate open boundary.
14. The async admission queue limits its own in-flight requests to four and
    priority selects queued work only. It cannot preempt running jobs. Native
    terrain cover and tile baking now use it, while other visualization and
    preparation producers still submit directly to Bevy pools; there is no
    measured cross-owner CPU reservation for UI and simulation.

These findings and their owner-specific file evidence are maintained in
[`../reviews/open-deterministic-simulation-contract.md`](../reviews/open-deterministic-simulation-contract.md).

## 10. Migration order

1. **Canonical boundary order.** Sort current scenario, event, and serialized
   Modelica work by stable keys, including input assignments inside each worker
   step request; add production Rhai verdicts for observable
   event ordering. Keep source-owned identities and fail visibly when a
   required identity is invalid.
2. **Cycle and clock context.** Current scheduled hook owners supply typed
   owner context, and generic `invoke_hook` forwards the active execution
   context. The initial-camera, runtime UI recording, runtime-surface
   visibility/properties, and component-refresh policies reject off-cycle calls.
   Keep `RuntimeCycleSet` labels distinct from cadence ownership. The
   remaining D14 work is to establish independent UI and
   visualization cadences where owner behavior needs them, while keeping both
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
   work on exact `SimulationProgress` keys, including physical body/joint
   admission, and keep presentation readiness on its own status path. Seed
   authored poses and install each compiled Modelica solver at its initialized
   time-zero state before opening the scenario gate. Keep every shared
   simulation clock at zero throughout preparation; do not prime Modelica while
   holding only Avian physics or reset any clock afterward. The first live
   Modelica exchange, physics integration, and behavior sample share `SimTick=1`
   under the normal fixed-step barrier. Production scene tests assert
   `on_start` at `SimTick=0` and the first `on_tick` at `SimTick=1`. Measure the
   serial and any deterministic parallel solver profile before selecting the
   production default.
8. **Real-time owner isolation.** Route main-world command reads/writes through
   typed tick-stamped requests and immutable snapshots, then move the whole
   authoritative tick to one paced simulation owner. Keep fixed `dt`, report
   deadline misses/backlog, and let realtime, unpaced capture, and test drivers
   choose wall pacing without changing tick order or discarding ticks. Retain
   the current same-world path until every authoritative consumer crosses this
   boundary; do not run only the solver concurrently. The current
   `SimulationTimingProfile` provides a bounded same-world measurement of tick
   service, rate-derived service budgets, fixed steps per app update, and retained
   pending simulation duration. Its separate clipped-time diagnostic is not
   recoverable backlog or a substitute for the command/snapshot ownership boundary.
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

The fixed-step Modelica adapter uses Rumoca's Model Exchange host for checked
initialization, input transactions, algebraic refresh and output publication.
`FixedStepSession` supplies a `MeIntegratorBackend` plugin that takes exactly
four classical RK4 derivative stages per configured step on its integer-indexed
time grid. A cubic continuous extension reads retained stages and never calls
the component during sampling. The host owns derivative-capability activation,
accepted-step validation, and the complete initial-equation solve. A raw lowered
start vector is not an initialized participant.

Native GJK simplex projection uses adaptive tetrahedron orientation predicates
for containment and native triangle queries for boundary witnesses and feature
identity. EPA uses the same orientation owner for seed winding and the existing
stable triangle-normal calculation for its faces. The maintained GeoRust
`robust` dependency supplies exact sidedness for thin support simplices; ordinary
cross products do not define that admission predicate. Inline regressions in
`lunco-physics` cover the actual large-triangle/cylinder and large-cuboid/cylinder
geometry sent by the continuous-contact owner. Query failures remain visible.

## Scene-test evidence transport

The production scene-test runner emits each typed `_EVIDENCE` event as one
`LUNCO_SCENE_TEST_EVIDENCE` JSON line at its external process boundary. Individual
encoded events use the existing 64 KiB evidence bound; an oversized or unencodable
event reports a diagnostic. Internal producers retain `TelemetryValue` maps and
arrays. Large observations stream as separate owner records rather than one
unbounded Rhai string or retained run-wide object. Verification reports continue
to retain their bounded summary and non-pass details through the same observer.

`multi_rover_stress.rhai` captures actual physics, Modelica and articulated rows
at its selected ticks while keeping every assertion enabled. The external
`scripts/capture_deterministic_physics_reference.py` adapter reads the maintained
profile matrix and requires exact agreement between two independent production
processes per profile. It accepts only the existing-reference comparison failure
during capture; behavior, clock, roster and deliberate-mismatch checks must pass.
It writes a separate candidate with binary, input and log hashes. Replace the
maintained reference only after paired agreement, then replay its production
profiles against the candidate. Each launch owns an explicit API port and verifies
process identity and port release.
