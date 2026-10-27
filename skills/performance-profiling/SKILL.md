---
name: performance-profiling
description: Diagnose or improve LunCoSim FPS, physics time, periodic stalls, Builder versus View differences, terrain/render cost, or Tracy captures. Use when performance must improve architecturally without changing BigSpace, substeps, shadows, terrain quality, or image fidelity.
---

# Performance profiling: measure the owner, then remove avoidable work

Read the current handover in
[`docs/reviews/open-400fps-performance-handover.md`](../../docs/reviews/open-400fps-performance-handover.md)
before changing code. A status-bar FPS number is a symptom, not an attribution.

## Required separation

- Run one production session that you own for the product FPS number, launched
  from the task checkout on a verified free API port. Do this even if another
  user's or agent's session is active; never control, stop, restart, or change
  the scene in a pre-existing session. Report concurrent GPU/CPU workloads and
  classify affected numbers as contention-affected, not clean acceptance. Do
  not use Tracy's instrumented number as acceptance evidence.
  Record the admitted Twin source revision and require its expected authored
  verdict before comparing completed-mission windows. A tick threshold or zero
  process exit does not prove acceptance. Exclude source/API errors, and require
  readiness and diagnostics to pass for the measured cohort.
  Native presentation failure appears in `RuntimeDiagnostics` under producer
  `render-recovery`; `presentation-stopped` is an error even when physics and
  the API remain usable. Application-frame FPS can keep advancing without
  rendered scene output. Reject such a cohort; do not interpret its frame rate
  as graphics performance or cure it by lowering quality implicitly.
  Compare the recorded Twin commits explicitly; two individually clean checkouts
  can have different revisions. Such runs are different source cohorts, even if
  their scenario name and application binary match. Initialization-source and
  milestone equality alone do not prove exact native-state replay.
- Run a separate Tracy build/capture using the adjacent `../tracy` checkout;
  start the matching `tracy-capture` before the production binary and inspect
  the settled window, not only startup. Bevy 0.20 uses Tracy 0.14.1; the matching
  official tools are `../tracy/capture/build/tracy-capture-0.14.1` and
  `../tracy/csvexport/build/tracy-csvexport-0.14.1`. Older capture tools cannot
  read the new client protocol.
  On Linux, the adjacent Tracy client supports `TRACY_SAMPLING_HZ`; record the
  requested rate and verify the saved capture's sampling period. Its default
  10 kHz CPU sampling can add substantial overhead. A lower sampling rate is
  useful for a separate zone-attribution run; it is still not FPS acceptance.
  Select native physics zones for broad phase, narrow phase, solver preparation,
  substeps, integration and CCD. Keep `lunco_physics::order_audit` disabled
  unless deterministic ordering itself is the measurement subject.
  Check the profiler listener and connection against the owned PID with
  `ss -ltnp`/`ss -tnp`. A concurrent Tracy client can occupy 8086 and move the
  owned app to the next port; pass that port with `tracy-capture -p`. A capture
  from another process is not evidence for the task scene.
  A zero collector exit does not prove capture integrity. Compare the saved
  file size with the collector's reported compressed payload plus native file
  framing, and require a successful native CSV export before using its zones.
  `FileWrite::GetCompressionStatistics` excludes the six-byte file header and
  four-byte length per compressed block; these bytes are present on disk. A size
  mismatch,
  parser failure or stalled export leaves that capture unusable. A reported
  instrumentation failure also makes the requested window incomplete. Keep hot
  zone names bounded: step ids, timestamps, and measured durations must not be
  span fields when the Tracy layer uses those values as source-location identity. Inspect the
  exact owned exporter process before terminating an unsuccessful read; do not
  restart a still-running export solely because an observation timed out.
  Check disk space while the owned app is live as well as after it exits.
  Native capture writing can overlap runtime allocations, and the collector
  can report success despite a truncated file.
  Native unwrapped CSV rows are grouped by source location; sort by
  `ns_since_start` before reconstructing chronological phases or transitions.
  Create the ignored capture directory and check that the collector is still
  alive before launching the app. A failed collector must not leave an
  unconnected instrumented client accumulating events. If fine executor
  bookkeeping dominates the diagnostic, a separately labelled cohort may use
  `RUST_LOG=info,bevy_ecs::schedule::executor=warn`; retain system, schedule and
  native physics spans and never compare that cohort as an isolated speedup
  against a different filter. Exact-zone CPU selection matches the zone's own
  thread: a FixedMain main-thread selection excludes concurrent Bevy workers.
  For sampled CPU stacks within a slow zone, use the read-only
  [`scripts/perf/tracy_samples.cpp`](../../scripts/perf/tracy_samples.cpp) tool
  and its [build/interpretation instructions](../../scripts/perf/README.md#saved-cpu-samples).
  Match the capture's Tracy release when building its server library, verify
  the owned PID, and select the zone's exact thread and time interval. Samples
  can be narrowed to complete instances of one exact zone by appending its name
  to the sample export command; inspect `matched_zones` and the terminal result.
  Frame-wide samples include unrelated work outside the selected child zone.
  The native exporter supports `-e -u` for per-event self times. Match those
  rows to the inclusive export by source, thread and start before selecting a
  bounded window: self duration does not provide the event's original end.
  For one complete schedule subtree, require the selected self-time sum to
  equal its inclusive duration. Main-thread schedule self time includes waits
  for worker systems; it does not by itself prove executor overhead.
  Profiling data stays in ignored `scripts/perf/captures/`, `.cache/` or
  `target/perf/`; commit
  reusable tool sources and documentation only.
- Compare Builder and View with the same scene, camera, rendering-quality
  settings, physics substeps, shadow settings, and terrain assets. A Builder-
  only cost usually means an editor observer, rebuild, projection, or UI path,
  not that physics needs a different global timestep.
- For a settled DEM quality window, require `TerrainLodStatus.stream.wanted ==
  resident`, `stream.pending == 0`, `derived.active == false`,
  `derived.pending == 0`, and `derived.ready == derived.total`. Record those
  owner facts before and throughout the window alongside camera and quality
  inputs. Simulation readiness alone does not certify optional visual-map
  preparation. Keep this measurement gate out of simulation admission.
  Audit a surface camera with `world_pos` and `world_rotation` in the active
  simulation frame; `visual_foci` reports composed root coordinates, which move
  with celestial ancestors even when the camera is stationary over the terrain.
- Assert the intended fixture before timing an owner-resolution benchmark.
  `PortRegistry::default()` installs the generic input owner first; registering
  other backends afterward does not make them precede it. A synthetic
  precedence fixture must verify its resolved owner index.
  The ignored `lunco-port-core` `multi_port_transaction_diagnostic` measures
  2,048 complete atomic transactions over 32 entities with 1, 8 or 32 inputs
  each, behind five registered but absent component owners. It asserts the
  winning backend and exact final scalar bits. Report every width, including
  regressions, and qualify the actual scene separately: a multi-input fixture
  gain does not establish a faster production exchange.
- Attribute CPU, GPU, physics, terrain, and UI separately. Never disable
  shadows, lower authored terrain quality, change BigSpace, or change the
  standard substep count to make a graph look better.
  For typed hook conversion, `lunco-hooks-rhai`'s ignored
  `map_conversion_diagnostic` measures complete nested-map conversion at 8,
  128 and 2,048 entries in sorted, reverse and permuted order. Compare every
  case with independent executable snapshots and alternating runs, without a
  concurrent build. This fixture uses the test allocator; pair it with the
  production UI-preparation gate and an owned scene before claiming runtime
  effects. A fixture improvement is not a whole-frame or loading improvement.
  Snapshot compiler-managed executables with independent copies, for example
  `cp --reflink=auto`, never hard links. Rebuilding can overwrite every hard link
  to a test artifact. Verify separate device/inode identities and record hashes
  before and after the comparison; reject results if a snapshot changed or a
  supposed code variant resolved to the same executable.
  Before grouping static shapes, inspect native child-pose invalidation and
  material inheritance. Preserve mutable global material defaults; a lower
  body count alone does not prove a speedup or equivalent physical behavior.
  For pose-bridge selection, the ignored `pose_invalidation_diagnostic` in
  `lunco-usd-avian-core` measures 512 bodies over 256 persistent-system runs.
  Preserve its separate leaf, render-branch, above-frame and physical-parent
  cases; require exact native poses, velocities and leaf representation history.
  Pair it with a normal production window before claiming scene-level savings.
  `collider_composition_diagnostic` in the same owner runs 512 colliders over
  256 iterations for ancestor translation, rotation, scale, physical-parent
  motion and an irrelevant leaf. Require exact transform fingerprints across
  independent variants and retain the cached-prefix hierarchy-depth check.
  In a diagnostics-enabled render host, `lunco-render-bevy` adds
  `lunco_shadow_shared_early`/`late` and `lunco_shadow_camera_early`/`late`
  timestamps around the native shadow passes. Shared events correspond to
  individual point/spot views; camera events include that camera's directional
  cascades. Check all four names and their event counts before attributing total
  shadow cost. Late spans can contain only timestamp overhead. Use the
  existing Tracy build for native GPU export: Bevy 0.20's `trace_tracy` enables
  the render diagnostics recorder through `bevy_render/tracing-tracy`.
  Inspect timestamp support and recorded pass coverage before claiming a
  complete GPU total; a sum of recorded passes excludes uninstrumented work
  and inter-pass gaps. Use the
  early shared child spans `lunco_light_depth/<pass_name>` when attributing
  individual maps; native exports prefix them with `lunco_shadow_shared_early/`.
  Do not sum child and parent times. `lunco_shadow_view_work` supplies admitted
  viewport dimensions, optional phase emptiness and native bin/set counts.
  This brief CPU snapshot is not pass execution time; bins are not draw counts
  and stable counts do not prove static caster inputs. Native labels identify
  current render allocations, not authored lights. Close each child before its
  parent. Retain native dependency ordering, native-parameter admission before
  begin and render-context buffer order when changing this adapter. Reapply
  PBR's system-local early/late
  ordering rules after removing the original system; retaining its implicit
  type set alone preserves only dependencies targeting that set. Check the
  actual shadow image before admitting a capture as performance evidence.
  Qualify point/spot roots with the `shared_shadow_allocation` graphics fixture
  before comparing native builders. Its positive and negative pixel probes
  cover the separate shared-light schedule; camera shadows alone do not.
  `SharedShadowView` uses the native single-threaded executor for its serial
  GPU chain or two CPU-mode shadow passes. Native render preparation clears
  indirect parameter buffers once per frame before view work. Attribute its inclusive
  and self costs separately; frame-level Render and camera Core3d still use
  their existing executors. Verify native pass inventory and final buffer order
  before comparing executor costs. This render adapter does not change solver
  cadence or authoritative simulation ordering.
  Unsupported timestamp features or
  exhausted per-frame queries leave incomplete GPU evidence, not zero cost.
- For intermittent long frames, export individual CPU zones and correlate them
  with perspective/window markers. Exclude screenshot delivery, authored test
  verdicts and perspective transitions from settled statistics. Inclusive
  render/extraction schedules can contain a native `queue_submit` wait; report
  the submission boundary separately from owner computation. A long submission
  span alone does not establish its driver, GPU or scheduling cause. Retain
  periodic GPU clocks/utilization, CPU pressure and concurrent process evidence,
  then run the same exact uninstrumented artifact separately. Failure to reproduce
  an earlier tail in a later run does not prove a fix. Record visible popup/UI
  state alongside camera and graphics settings.

For JSON-scope stalls, distinguish authored source catalogs from completed
experiment histories. The asset/Twin owner publishes
`JsonAssetContent::PersistedResult` as identity-only metadata, projecting the
`persisted_result` Rhai fact without loading or copying result bodies. These
entries own no Bevy text handle and never delay JSON-scope readiness; loading
counts cover authored reads only. The existing
application asset policy skips those records before JSON decoding. Confirm the
`application_json_catalogs` production gate and measure the same admitted source
cohort. Do not hide malformed authored catalogs, change
Rhai limits or classify sources by guessed catalog filenames.

For coupled startup, distinguish the one-shot solve from participant
certification. Pending constructors supply no certifiable observation; the owner
waits on their outcomes and revalidates current source, wiring and physical
samples at final admission. Include coordinator counts and main-thread time
when measuring startup; worker timing alone misses repeated certification work.
Bootstrap API readiness does not certify the requested Twin. Record the exact
active Twin and its physics admission. An interactive replacement retires the
startup failure guard on `TwinClosed`; a late canceled asset result must not
terminate the replacement. The focused production gate is
`scripts/api/test_startup_scene_replacement.py --port <free-port> --log-dir <ignored-evidence-directory>`.
Record prepared-solver hits and native attempts/hits/compilations alongside
startup time; tests and other Twins can legitimately evict bounded artifacts.
An immediate repeat with unchanged inputs distinguishes cold-cache work from
recurring constructor cost without changing retention limits.

For Modelica native-construction costs, distinguish selected application work
from whole-source preparation. Projection reuse storage is needed only for
selected source programs and is shared across that application's issued colors.
Preserve checked identity, input validation and evaluation order. Compare the
same prepared component's complete f64 states alongside construction timing;
include its first complete observation. Zero native compilations do not prove
faster admission: reference evaluation can dominate a large coupled initializer
even when it avoids JIT work. Keep an evaluator change only when the actual
scene improves with its numerical contract intact.
Before changing an artifact-cache codec, sample its complete lookup zone.
Checked deserialization may spend most of its time reconstructing native
structural contracts; parsing and decompression must be attributed separately.
Do not bypass those checks to improve a disk-hit number.
Shared-value construction also pays for source discovery and symbolic proofs.
Its read collector shares the exhaustive Solve operand checks in one traversal;
preserve range/context checks and whole-program equivalence when optimizing it.
The symbolic term table interns complete operation keys once; each scalar output
uses the table-local ID and output ordinal. Retain full structural equality and
the shared table lifetime across segments and both sides of a check.
Scalar terms carry the original slot or operation/output identity directly;
there is no separate scalar-term interning table. Preserve disjoint identity
domains and the independent checker's complete walks when changing storage.
Operation shapes use the complete typed IR encoding, including floating-point
bits. A changed constant's bits must fail the checker; hash equality alone
cannot establish operation identity.
Private proof lookup tables use Rumoca's existing fast hasher. Preserve full
key equality and program-order ID assignment. Qualify constructor costs with
recorded prepared/native-cache hits and the fixed MSL canary; isolated proof
benchmarks do not establish faster scene admission.
Exclude overlapping builds/apps and record local dependency patches separately
from the portable lockfile. Dependency changes require the repository's explicit
authorization before edits.

For fixed RK4, verify that its ME component receives the opaque execution
backend issued by `rumoca_sim::fmi_execution_backend`. Selecting a fixed
integrator does not itself install compiled evaluation. Keep the same lattice,
input transaction and complete checked observation when comparing native and
reference execution. The nonlinear-array differential test qualifies numerical
agreement; its small diagnostic timing does not certify whole coupled ticks.
Use actual compiler-cache accounting on nontrivial production models to verify
native work. A local dependency path patch can disable prepared-model disk
caching; record that state and reject it as final loading acceptance for a
portable Git pin.

For terrain certificate lookups, whole-cell identity is the fenced surface and
the two raster indices. Refined rectangles also retain exact coordinate bits.
Keep one FIFO capacity and the same cache-independent interval and refinement
admission. Use the certified occlusion mechanism checks and production solar
visibility, night and invalid-admission gates. Compare the actual solar zones
in matched settled captures; a smaller synthetic lookup time is not whole-tick
or graphics acceptance.

Measure first scene readiness against the requested active Twin and spawned
physical root. Bootstrap readiness precedes Twin admission; do not use a minimum
elapsed-time filter to distinguish them. Record landing as a separate mission
milestone. Preserve source hashes and native construction/cache accounting.

For USD startup snapshots, attribute native path/field resolution separately from
file loading and ECS publication. Share the library's `BindingCache` only within
one immutable stage snapshot; preserve purpose resolution, native collection
membership and invalid material-target errors. Qualify an SDK update against the
same Twin revision and a regular production readiness/landing run before claiming
a loading gain. Dependency source edits still require explicit permission.

## Architecture checks

Frame-conversion and collider-admission kernels are owned by
`lunco-usd-avian-core`; its regular development build uses optimization level
three with development checks retained. Follow the actual owning package when
profiling moved kernels. Collider propagation must independently drain parent,
transform and body removals before comparing performance; a stale relative
transform invalidates a faster result.

Shader-port writes are fixed-step consumers even though their downstream GPU
binding runs at presentation cadence. Measure the actual backend with the ignored
`shader_input_transaction_benchmark` owner diagnostic, including both unchanged
and changing values. Its declaration and material-intent packages use regular
development optimization level three. Preserve live validation and no-op change
detection; a large visual binding count alone does not authorize changing clocks
or skipping writes.
The shader metadata callback uses the already-selected live slot. Preserve
authored-type precedence over a differently typed live override, rejection of
untyped drives, and current drive membership before accepting a faster result.

When comparing native CPU/GPU instance preparation, retain the same PBR,
shadows, meshes and quality. The desktop composition selects native CPU
preparation; qualify changes with both `shared_shadow_allocation` and
`vertex_shadow_alignment` pixel gates and source-matched normal Twin windows.
Keep the per-scene tradeoffs visible: the GPU path can have a lower median in
some scenes. Device capability is not active-path evidence:
camera `NoCpuCulling` requires native `RenderMeshInstances::GpuBuilding` and
indirect drawing. `NoIndirectDrawing` views need CPU frustum tests. Preserve
foreign-owned markers when reconciling these structural changes.
Native camera and shadow batch modes must also match the active builder. CPU
preparation admits effective `GpuPreprocessingMode::None` after native device
support initialization on every `RenderStartup`, including recovery. A faster
cohort with missing geometry is a failed visual comparison, regardless of FPS
or readiness diagnostics.

For GPU-only buffer layout, preserve deterministic scalar math and validate
shader array stride at the native allocator. `UninitBufferVec` uses encase's
`<[T; 1]>::SHADER_SIZE`: a shader element's minimum size alone does not include
all array alignment. Owner regressions cover both structure padding and `vec3`
array stride. Verify the actual terrain image and retained render diagnostics
when admitting native GPU preparation.

For co-simulation input writes, preserve the port owner's live metadata and
precedence checks. Initial handoff and live exchange use `input_transaction`:
immutable staging and commit share one exclusive world borrow, so each eligible
target is validated once. Returning an error rejects the staged batch. A
retained command preparation has no such borrow guarantee and must still be
revalidated at commit. Do not replace either proof with a revision-only cache.
The scoped transaction's Bevy hash table stores indexes into its staged writes;
duplicate checks compare full identities without cloning the port names again.
Its compact records retain only the admitted callback, entity, name, slot and
value. Preserve full metadata snapshots for independently retained preparations;
only the uninterrupted transaction borrow permits releasing admission metadata.
Unit-reference clones share immutable definitions, coordinate-frame clones share
immutable text, and Avian reuses its fixed built-in unit references. Preserve
exact text/hash/serialization behavior and reject blank frame identities. This
storage sharing never caches the live port
owner, bounds, frame or permission. The bounded debug spans
`port_input_transaction_stage` and `port_input_transaction_commit` separate
validation from application when `lunco_port_core::ports=debug` is enabled.
When reducing shader commit lookups, retain Bevy change detection: borrow the
component once, inspect through its shared dereference and mutate only a changed
slot. The owner diagnostic measures both constant and changing transactions;
`lander_plume_activity` verifies connected shader delivery and rejected scalar
range writes through the production scene-test host.

Before a wheel-shape comparison, verify the coordinate contract in
`docs/architecture/21-domain-usd.md`. Compare the native collider's axial and
radial bounds with its authored geometry and mechanical axle. Preserve the
admitted geometry through a restart: disposable view edits are discarded when
the clean source document refreshes. For an isolated unsaved comparison, use
the normal document journal, do not save it, and verify source hashes after exit.

For synchronous USD edit stalls, inspect compound operation count and native
composition lifetime. The document owner's `UsdEditSession` shares one current
authoring stage across an ordered sequence; direct layer changes invalidate it.
Preserve the private candidate and atomic rejection boundary described in
[architecture 21](../../docs/architecture/21-domain-usd.md). Measure the actual
command in a production run; a faster edit still occupies its calling thread.
Also inspect `usd_native_reference_discovery`: native property resyncs prepare
only their asset values, while prim resyncs scan the affected subtree. Asset
types gate provenance and time-sample resolution. Domain live-edit owners consume
one complete notice batch in place; label changes must preserve live child state.

Modelica coupling holds `FixedMain`, while the pacing clock retains running
demand in `Time<Fixed>::overstep`. Compare completed simulation time against
monotonic wall time and inspect pending demand through `SimulationTimingProfile`;
a short native physics service time does not certify realtime co-simulation.
When a coupled sensor changes with host pacing, compare its full input values
before attributing the difference to solver math. Contact impulses belong to
the completed master interval; `lunco-physics::contact_of` owns load normalization
for ports and telemetry. Native admission delta may be zero while that sample
remains valid. Exercise `scripts/api/test_contact_force_hold.py` through the
production scene-test host when changing this boundary; its authored observer
checks loaded contact, held time, retained force and an unavailable surface.
Physical render easing consumes the time owner's bounded
`SimulationPresentationTime.interpolation` snapshot; full queued cycles are
never a render extrapolation factor. Compare successive sample ticks across coupling waits and
pause/resume: they must not decrease within one mission. Celestial and
interaction clocks retain their separate owners. The common application plugin
uses native endpoint capture/restore with public custom-easing markers. Do not remove queued time or
loosen quaternion admission to conceal an invalid render pose.
For scene-test diagnostics, `--tick-hz` varies manual update pacing; use the
runtime's `Time<Fixed>` quantum and elapsed time when interpreting physics cost.
Completed Modelica receipts have one reader in `SimulationCompletions`, serviced
before clock projection, before admission of a held pending fixed cycle, and by
the native driver on response readiness between application updates. A ready
receipt can release another cycle in the same update; unfinished work yields
without a wait. Full presentation isolation remains a separate requirement.
Native headless readiness is shared through `SimulationCompletionSources` for
participant workers. Its worker leases control registration lifetime;
readiness does not consume a receipt. Windowed hosts never block on this wait.
Complete-cycle continuation checks both coupling and generic progress holds.
For runtime UI policy work, compare actual immutable input snapshots as well as
producer change ticks. Equal facts must not schedule another projection merely
because a presentation component received an equal-value write. Preserve hook
generation, source lifetime, caller identity and derived vessel invalidation.
Attribute `exposure_runtime_surface_facts` separately from its publisher and
worker policy spans. Public output selection uses the port owner's name index
and live slot order; preserve complete output facts and unsampled-port semantics.
The ignored `runtime_output_facts_diagnostic` in `lunco-luncosim-exposures`
measures complete output projection, including all-value allocation. Compare
independent binaries and qualify the authored public-output consumer in a
production session; fixture timing alone does not establish a scene speedup.
The default communication period is one physics tick; longer periods must be
authored explicitly.
Physical celestial sampling uses `CelestialClock::epoch_at_tick`, with
`celestial_physical_inputs` at interval start before environment computation and
`celestial_physical_endpoint` after native writeback. The component's tick is
the admitted cycle, and the sampler owns its phase. Presentation's angular
error gate never chooses physical inputs. Directions, gravity and solar
visibility share `SimulationPoseQuery`; do not restore raw hierarchy readers
for moving probes. Reuse the ephemeris provider's exact-epoch cache. Attribute
sampling separately from native physics and from the rendered celestial chain;
its cost must not be hidden inside a physics-only timing number.

For deliberately frozen captures use `StepSimulation` and presentation-timed
sequencing. Manual demand uses the same fixed runner as paced execution and
still respects preparation and solver barriers. A native hold gates the entire
simulation; compare `clock_snapshot().physics_elapsed_s` and `fixed_elapsed_s`
at completed-cycle boundaries before interpreting throughput measurements.
Attribute numerical service separately from transaction dispatch and result
admission. Live workers pass the exact communication point to the existing
`LiveStepper::advance_to` boundary; adaptive Rumoca sessions and fixed
prediction own their internal steps. Preserve the fixed prediction lattice,
event handling, input sampling and finite/completion validation when optimizing
that path. Do not add another worker-side integration grid or catch-up planner.
For replay divergence, enable
`RUST_LOG=info,lunco_modelica_worker::step_audit=debug` in a separate diagnostic
run. Compare session-relative step ids, communication times and ordered input/
output fingerprints for each exact participant identity. These bounded log
records locate differences; hash equality cannot prove state equality. Use
`trace` for complete accepted samples, or `ReadModelicaStepSamples` for selected
values in the last accepted transaction. Full traces can produce large logs
and perturb scheduling; check storage and retain an uninstrumented acceptance
run. Never infer a numerical repair solely because tracing hides a divergence.
Keep communication-clock checks at floating point roundoff scale, with finite
timestamps required at both request and completion admission. A solver's state
tolerance must not loosen its communication endpoint validation. Validate the
endpoint using `start + dt`; subtracting large absolute timestamps can reject
a valid short interval through cancellation.

UI exposure publication is paced by `Time<Real>`, independent of simulation
pause and rate. Its dirty-state gate still coalesces unchanged facts. Attribute
main-thread snapshot capture separately from `runtime_ui_policy_preparation`
and owner publication. Native pure policy evaluation uses shared bounded worker
admission, retaining captured hooks and caller context; one job plus one latest
pending snapshot per surface bounds work. Source and identity changes fence
completion. The wasm kernel is currently cooperative, not an isolated worker.
Wait for ready surfaces and inspect preparation failures before taking a UI
performance cohort. This boundary does not isolate the entire fixed runner
from the presentation thread. Keep
mission/timeline waits and telemetry deadlines on their declared simulation
clocks; changing their timers to wall time changes simulation semantics.

For script cost, attribute `rhai_native_task_tick` by stable actor gid and
compare the same task-invocation range or simulation phase. Equal wall-time
windows need not contain the same mission work when realtime pacing differs.
Task callbacks borrow the persistent map through Rhai's bound receiver; see
[the callback contract](../../docs/architecture/rhai-task-tree.md#leaf-callback-contract).
Preserve captures, imports, state/error semantics and fixed cadence while
removing copies at that boundary.

For cost or memory that grows with runtime, check interpreter scope lifetime:
initialized globals persist, while per-call constants and function locals
must be released on both success and error. Use Rhai's scope-rewinding option
at every scenario call boundary; do not periodically clear live scenario state.

For Rhai presentation cost, inspect helper arguments and array iteration for
copies of nested facts. Project the fields a helper consumes before calling
it; indexed access can count participant statuses without copying their model
input/output maps. Preserve the public hook facts and compare complete property
outputs, including empty and unavailable states. Use paired real-owner spans
for attribution; synthetic large-payload timings alone do not establish FPS.

For typed Rhai boundary cost, use the shared `lunco-hooks-rhai` converter.
It borrows input containers; embedding owners lower native leaves through
`dynamic_to_hook_with_native`. Do not build a normalized Dynamic tree before
calling it, clone containers to inspect their type, or add another recursive
converter. Preserve native f64 math, unsigned integers, shared values and
visible rejection of unsupported types. Check the interpreter resource seam
with `typed_conversion_rejects_nested_native_values_without_cloning_them`, then
exercise production typed command/query consumers, including a negative case.

For port-exchange cost, inspect `BindingStatus.connections` and join targets to
`ListPorts` owner metadata. The causal-state sink count is not the number of
compiled wire targets: scene/shader inputs also participate in the exchange.
Separate `cosim_exchange_write` from accumulation before changing routing.
Preserve live metadata, bounds, ownership and stale-slot checks. For shader-name
cost, use the shared borrowed converter described in
[shader parameters](../../docs/architecture/shader-layers-and-params.md);
canonical-name lookup must not allocate, and entities without `ShaderLook` must
reject before conversion. Allocation probes and kernel timings do not certify
whole-scene FPS.

For native physics ordering investigations, enable
`lunco_physics::order_audit=debug` in an owned diagnostic session. The continuous
owner records native body and collider identities, poses, mass/inertia and
velocities, plus contact-graph IDs, colours, manifold indices and prepared
point geometry at the physics clock. Compare the first divergence before
attributing a difference to pool size. This logging is diagnostic and must be
disabled for performance acceptance. Partial pose checkpoints do not prove
full-state replay.
Compound audit rows include native scale and local part mass properties.
If the first divergence is already in COM, compare admitted geometry and
mass ownership before attributing later motion to the query dispatcher.
For time-zero mass/inertia, inspect `PhysicsGeometryAdmission` before changing
the authored checkpoint or reference. The shared native geometry schedule must
finish before USD publishes physics-ready; advancing a solver tick to prepare
geometry would change the initial state. Qualify preparation changes with the
rootless scale/mass mechanism check and the unchanged four-rover production
gate, including its deliberately altered-row negative.
`PhysicsGeometryPreparation` uses the native single-thread executor for its
short ordered chain. Compare the same source cohort in normal A/B/A windows
and require exact replay against the maintained reference before changing its
executor. Main-thread schedule self time includes worker waits; do not label
the whole self span dispatch overhead. Attribute that child schedule separately
from the complete fixed cycle and the native solver.
The shared application also configures `FixedPostUpdate` with the native
single-thread executor for the surrounding physical commit chain. Attribute
that outer schedule separately from `PhysicsSchedule` and geometry preparation.
Keep native parallel queries, Modelica worker pools and authored replay checks
when comparing executor configurations.

When attributing native GPU preprocessing, trace allocation identity rather
than only buffer capacity. Bevy's `collect_buffers_for_phase` exchanges
per-phase and frame-level buffer cohorts each frame; equal sizes do not mean
consecutive bindings agree. Native view groups must be present on current
extracted views. Compare actual buffer identities, binding offsets/ranges,
layout descriptors, occlusion textures, work modes/counts and bin-job
parameters, and measure admission overhead and native rebuild frequency.
Pipeline-cache change ticks alone are unsuitable: native queue processing
mutably accesses that resource every frame. Preserve native deferred publication
and every GPU preprocessing/draw pass. See the current handover for the owned
binding-lifetime captures; they establish no product speedup.

For authored image preparation, trace `lunco-materials` typed
`PreparedShaderImage` loading: native decoding and owned RGBA8 filtering precede
source/child publication. CPU filtering awaits the async compute pool with an
owned image, without blocking an I/O worker or borrowing ECS. The renderer resolves `ShaderTexture` and reacts to
source asset events; it must not snapshot resident pixels. The prepared loaders
register no extensions, preserving native image/glTF settings and decoding.
Prepared children use the native render-only extraction contract: pixels move
into GPU preparation while the main asset retains metadata. Audit CPU readers
before choosing that usage; sampler/descriptor edits must come through source
reload with fresh data. Verify initial load and reload publish complete chains, independent color/scalar/
normal identities, native-image forwarding and invalid-pixel rejection at the
asset seam. `load_raster` accepts physical paths; verify labeled requests reject
before issuing a prepared-root load and importer-owned native image handles
still load, bind and reload. Preserve exact RGBA8 transfer, sampler quality and role identity.
Worker-time gains need separate loading/UI/FPS evidence before an app-level claim.

For repeated analytic USD primitives, inspect reflected `Mesh3d` identities
before inferring duplicate preparation or upload cost. `PrimitiveMeshAssets` in
`lunco-usd-bevy-mesh` owns shared preparation and weak native mesh lifetime;
source admission remains in `lunco-usd-bevy`. Preserve the
[immutable mesh contract](../../docs/architecture/render-decoupling.md#immutable-primitive-mesh-assets),
quality replacement and entity-specific edit isolation. Run the authored
`primitive_mesh_sharing.rhai` sandbox gate for equal and unlike dimensions.
Fewer asset IDs alone do not prove fewer GPU draws or faster frames. The retained
`engine.frame_time` series samples raw app-frame values at the fixed 60 Hz
ceiling and may repeat values; collect it before its 240-sample ring truncates
the measurement window. Report it separately from once-per-rendered-frame data.

Search the owning systems for unconditional writes, full-set topology scans,
repeated observer registration, per-frame allocations, polling, and work that
should be gated by a revision/change event. Structural edits should invalidate
structural caches; transform propagation and telemetry output are not by
themselves topology changes. Check both the Builder and View registration paths
before fixing only one.
For loading throughput, compare worker CPU duration with the complete job window
and idle gaps while preparation is queued. `AsyncWorkAdmission` starts work in
PostUpdate and refills released slots on completion; its global bound, priority
service, cancellation and weak queue lifetime remain independent of frame rate.
Enable `lunco_core_runtime::async_work=debug` for `async_preparation_job` zones
grouped by finite work kind and priority. Dispatch logs report each operation's
queue wait separately; identities and elapsed values stay out of zone fields.
Correlate `preparation dispatched` with `preparation started` by operation key
to separate shared admission delay from task-pool dispatch delay. Wall-clock
log differences are diagnostic correlation, not simulation clock inputs.
When capacity exceeds one, required preparation retains one admission slot;
optional queued work and external permits share the remaining capacity. Inspect
pool dispatch separately. Queued native preparation uses the process-owned
`LunCo preparation` Bevy pool, with the host's configured async worker width.
Native asset/shader compilation keeps its own pool; test preparation while
those native workers are occupied and preserve non-blocking queue retirement.
Native one-shot validation uses `submit_transport` to wait in that shared
queue. Its request owns queued cancellation and the result retains the permit;
temporary worker occupancy must not become a `QueueFull` query rejection.
Check both full waiting-queue rejection and stale request retirement, including
reuse of the same operation key. Keep source/policy publication on the owner.
Verify refill without another host update and retirement with captured permits,
then compare settled terrain readiness and replay the authoritative state.
For physical solar visibility, attribute `publish_sun_visibility` separately
from native physics. Canonical ray-enclosure cuts reuse spatial certificates
only while the complete current segment is covered by them. When a ray crosses
both midplanes, the same slab clipper must produce a complete rectangle cover;
prefer a common containing half when one exists. Bound the total cut count,
retain scratch storage, and never quantize or offset the fresh ray. Preserve exact
rectangle keys, complete surface invalidation, FIFO capacity and the configured
`max_ray_cache_depth`; cache residency must not change the attempted cut order.
These exact-key caches use Bevy's maintained hash map. Keep lookup storage
independent of decision order: eviction uses the FIFO queue, and neither the
certificate map nor the ray-local sample map supplies traversal order.
The immutable ray query fences surface identity and capacity once before its
first bounds read. Preserve the first fault kind and remaining quota under
cold/warm caches, tiny capacity, surface replacement, grazing rays and both
travel directions when changing this boundary.
Enable `lunco_terrain_surface::occlusion=debug` for the bounded diagnostic spans
`terrain_occlusion_bounds_miss`, `terrain_occlusion_exact_footprint` and
`terrain_occlusion_point_sample`. They separate reusable-certificate preparation,
fresh exact-footprint bounds and actual uncached point evaluation. Compare a
settled native-export window; instrumented times are not FPS acceptance.
The moving-footprint regression verifies nearby-ray reuse and a real blocker
outside the earlier clear certificates. Distinguish terrain enclosure cache misses from actual
height evaluations: a warm bounds cache can still repeat procedural samples
at adjacent-cell and refinement endpoints. Ray-local sample reuse must use
exact f64 coordinate bits and preserve traversal, interval budgets, unavailable
data and fault behavior. Run the focused certified-occlusion mechanism checks
and the production terrain solar visibility and invalid-admission gates; compare
the unchanged solar-power observations and run a separate production Twin window.
The ignored `occlusion::tests::certified_cache_moving_ray_diagnostic` in
`lunco-terrain-surface` exercises one- and two-metre raster cells with 32 ticks
at a thrashing capacity and 512 ticks at a resident capacity (36 rays per tick).
Compare alternating invocations of independently copied baseline/candidate test
binaries with unchanged hashes. Require identical ray inputs and verdicts;
cache-only changes must also retain interval totals and occupancy. A stronger
geometric certificate can reduce admitted refinement work; verify its bounds,
budget failures and cold/warm admission independently. These are consumer
timings, not complete fixed-tick or application FPS measurements. Resolve sampled instruction addresses against
the matching executable and saved process mappings before attributing a derive
line to hashing: that line can belong to inlined equality inside table lookup.
For procedural synthesis, separate noise, population generation and profile
evaluation. Reuse input-constant preparation at the existing mathematical owner
before adding a population cache. Preserve cell hashes, f64 operation order and
draw independence; compare exact point bits as well as production terrain and
simulation outputs. Keep math diagnostics separate from whole-app acceptance.
For dataset consent, check `publish_dataset_provisioning_surface` separately
from its Rhai policy. Diagnostic-outbox drains must not mark registry catalog
facts changed. Bevy excludes a system's own writes from its next change check;
trace other writers before attributing repeated work to a publication cursor.
Preserve real registry/workspace/command edits, policy replacement and interactive-window
transitions. Run the windowed `dataset_provisioning_surface.rhai` gate with a
declared missing scope; confirm unrelated dismissal preserves the view and
active dismissal clears retired properties without requesting a download.
For the derived scene entity tree, immutable parent insertions use native
lifecycle messages coalesced by entity at the Update gate. Compare current
edge values with the cached ancestry; identical replacements must not rebuild.
Mount identity, mount diagnostics and the system-entity filter compare with the
build owner's last observed inputs. That scene-owned scope resets on teardown.
A pending worker or a derived hierarchy diagnostic must not repeatedly
invalidate unchanged mount/filter inputs; genuine source changes still advance
the worker revision and reject stale results.
Separate change admission from rich candidate reads: the native changed query
returns only entity identities; indexed candidate reads fetch label/visibility
data for those identities. Preserve the candidate predicate and mutable metadata
coverage. Direct mutable component edits must still invalidate the cached tree;
insertion observers alone cannot detect them.
Retain mutable-label and scene-boundary checks, removal handling, unnamed
ancestor invalidation and active-Twin queue retirement. Native renderable
additions cover marker admission; the metadata query also admits new names.
Retain the native mutable metadata change query, including edits to unpublished
worker members. Cover a new name on an already tracked unnamed ancestor.
Compare indexed membership scans with native
change filtering in the live owner before changing the scan strategy. Measure
the complete gate and source observers separately from tree derivation and
panel paint.
The gate exposes `entity_tree_gate_named_changes`,
`entity_tree_gate_removals` and `entity_tree_gate_parent_changes`; compare the
complete gate as well as these phases, with equal instrumentation and cameras.
For frame-contract gate costs, distinguish the full connectivity validator from
its admission/fixed run conditions. The bridge observes native physical and
hierarchy lifecycle events through one change-ticked resource; each consumer
retains its own observation boundary. Preserve insertion, reparenting, removal,
despawn and active-frame switch/removal detection without waking on ordinary
pose values. Validate a disconnected body through the public owner diagnostic
and stopped solver ticks, then verify a replacement scene resumes physics.
For link-sweep costs, attribute `celestial_ephemeris_position` separately from
pair geometry and verdict hooks. Use `link_collect_endpoints`,
`link_snapshot_occlusion_inputs`, `link_evaluate_pair`,
`link_terrain_line_of_sight` and `link_verdict_policy` to distinguish snapshot
preparation, terrain marching and typed-policy marshalling/execution. Pair
spans contain terrain and policy spans; do not add inclusive durations.
The analytic provider's exact-epoch cache shares native VSOP2013 EMB and
ELP/MPP02 Moon operands across dependent bodies. Earth's native
`heliocentric_position_from_samples` consumes those checked samples. Keep epoch
validation at their native evaluators and epoch coherence at the provider cache;
an EMB-only request retains its independent range. Preserve native
f64 values and formula order; a new epoch bit pattern invalidates final and
intermediate samples together. Compare public `BodyPosition` results against the
prior exact artifact at adjacent and revisited epochs before claiming a gain.
Before increasing epoch retention, record exact body/epoch keys and cache hits.
Distinguish first evaluations from repeated misses after eviction; extra slots
cannot save unique sample work. Do not quantize epochs or duplicate a maintained
analytical formula to create apparent reuse.
For an exclusive system, check repeated `World::query` construction separately
from iteration. Retain Bevy's native `QueryState` in the owning system when
the query shape is stable; read current membership each pass and collect
identities before structural mutation. Preserve that system's serial schedule
position and lifecycle ordering. A settled-frame saving also needs live
add/remove or replacement evidence, since queries must admit newly matched
archetypes after startup.
For USD Modelica source dispatch, inspect pending membership before attributing
its exclusive span to initialization. Empty calls still flush the World. The
native component/scene-transaction run conditions skip that boundary only when
there is no admissible source work. Verify deferred source insertion, scene
holds and replacement, while retaining the wiring/defaults chain, prim-path
compile order and initialization sampling. A removed idle span proves avoided
work; it does not by itself prove loading or whole-app FPS gains.
The co-simulation endpoint lifecycle publisher uses this native exclusive query
parameter. Validate its same-frame ready transition and run sandbox binding
acceptance again after scene replacement; keep immediate endpoint insertion and
the `PostUpdate` binding boundary intact.
The neutral scenario driver shares its native model query across preparation,
startup, fixed and visualization passes; preserve fresh model/scope/authority
reads and stable actor sorting when changing that owner.
For telemetry retention, separate due recording batches from cheap between-batch
calls. `SignalRegistry::record_scalar_at_rate` admits and appends through one live
history lookup; preserve backwards-time segments, retention, archived-source
reactivation and catalog notifications. A registry benchmark isolates that owner,
but production retention spans and raw physics/frame tails need separate evidence.
Modelica and physics channel-limit checks read the live scalar count after
append and only hash history identity for admission when the catalog is full. Preserve
existing-channel recording above a lowered limit and rejection after history
removal; a cached producer identity alone does not prove that history exists.
For dynamic-body admission, compare `activate_dynamic_bodies` with its native
eligibility gate. Paused or invalid assemblies retain `ShouldBeDynamic` for
diagnostics but must not wake admission when its query has no eligible bodies.
Share the exact archetype filter; retain preview-ancestry checks, stable identity
faults, promotion order and joint holds inside admission. Verify paused poses
and warnings through `sandbox_ramp_placement.rhai`, plus exact-state replay
against the unchanged binary. A stale tracked Modelica reference is not passing
replay evidence; capture a separate baseline and preserve the tracked reference.
For articulated mass facts, separate `refresh_dynamic_joint_island_masses`
from its `dynamic_joint_island_mass_rebuild` span. The physics owner retains
lifecycle invalidation through pauses; native change detection handles mutable
effective masses and joint endpoints. Normal and rollback exchange read the
same derived resource at their existing boundaries. Verify live mass edits,
rejected negative writes and island splitting through `joint_island_mass.rhai`,
plus exact replay against the unchanged artifact. Quiet steps must not rebuild
islands; marker removal, body removal and paused lifecycle edits must still
invalidate them. Preserve native sums and the full-island invalid-mass result.
For a system that queues many compatible ECS component changes, inspect its
`system_commands` flush separately from the system body. Collect changed values
and use the owning crate's existing batch command path where it preserves the
same commit boundary; retain change filtering so a quiet fixed cycle queues
nothing.
In panel paint code, use `PanelCtx::resource` for reads. Its `resource_scope`
temporarily removes and reinserts the resource, which marks it changed even
when the closure only reads it. Scope a mutable borrow only around a real
state transition; otherwise a steady Builder repaint can wake change-detection
systems and full-scene reconciliation on the next Update.
USD authored/composed text painting borrows `UsdViewportState` and its cached
source strings. Check `usd_preview_render_layer_propagation` when diagnosing
preview invalidation; report traversal cost separately from frame time. Opening
several previews must not add bodies, colliders, or joints to the mounted scene.
For SysML requirements rebuilds, inspect `sysml_requirements_rebuild` input
flags. Domain notification publishers check `has_pending_events` before
mutably draining their registry; an empty queue must not invalidate readers.
Workspace document focus and USD editor open/close are not SysML inputs. The
requirements producer compares the active Twin identity, root, manifest, and
file index before reacting to workspace change detection. Verify both an
unrelated editor change (no rebuild) and a Twin/source/evidence change (fresh
projection); do not suppress domain registry invalidation.
Source links use `DocumentRegistry::doc_for_file`, which checks all exact paths
before resolving file aliases. Preserve symlink identity and dirty-document
semantics; do not build a panel-local path registry.
Inspect scenario preparation separately from lifecycle execution. Unchanged
actors borrow their canonical source; only admitted compile/retry work copies
worker inputs. Do not defer initialization or change actor commit order to
reduce this preparation cost.
For Modelica response retention, reuse observable-variable keys and move the
worker's input/output buffers into the accepted record after transaction
validation. Preserve detected-symbol/output precedence and verify reset
isolation, signed-zero values and stale-result rejection at the owner seam.
The observable snapshot uses native Bevy-reflected `IndexMap` storage; inputs
and parameters use Bevy's `HashMap`. Indexed publication checks actual names,
aligns each admitted batch, and preserves reordered, omitted and duplicate rows.
Keep public reflected reads and filtered `SnapshotVariables` results consistent
with the accepted step. Retain live input/parameter classification, shared
name-sorted compile and step sampling, and command-surface precedence in the
borrowed-entry input bridge.
The ignored `named_observation_admission_benchmark` exercises 512 accepted
steps with 2304 named observables and excludes input/parameter fields. Compare
the same fixture and retain its exact-value and clock checks. Qualify the
`modelica_variable_projection` and `modelica_variables_missing` production gates
and the compiled-metadata editor scenario after changing this boundary.
For worker-side collection, the ignored solver-owner
`observation_diagnostics::named_and_batched_observation_collection` compares
native named-map collection with the public ME batch reader and shared labels.
Run it with `cargo test --locked -p lunco-modelica-solver --lib
observation_diagnostics::named_and_batched_observation_collection -j 4 --
--ignored --nocapture`. It checks all names, f64 bits and current inputs at five
communication points, then times 512 reads per mode in alternating order.
Construction, advancement and label preparation are excluded. Do not turn that
collection result into a complete-worker or coupled-tick claim, duplicate the
adaptive host constructor, or remove input visibility to bypass an SDK boundary.
The [owner proposal](../../docs/reviews/modelica-batched-observation-2026-10-24.md)
records the supported batch APIs and dependency-approval boundary.
Immutable live solver facts share one `Arc` across the cache, receipt and model;
do not clone their parameter/input strings on every step. The bridge ownership
regression checks shared storage as well as terminal metadata rejection.
Modelica workers return current observations only. Retained samples belong to
`lunco-modelica-telemetry` and the shared chunked `SignalRegistry`, installed
through `TelemetryRetentionPlugin`. UI consumers read that same history and
receive only a checked `ModelicaSessionStarted` lifecycle message when a new
session requires binding cleanup. Verify retained history through
`ListTelemetryChannels` and `QueryTelemetryHistory` in both hosts, and exercise
the public accepted-step reader through production. Measure response latency
separately from solver integration and fixed-step output publication.
For loading, separate prepared-FMI lookup/lowering, live construction and ordered
publication.
Rumoca's `SharedAssignmentSchedule` carries the already-checked immutable
programs to interpreter and native construction. Preserve that shared storage,
source provenance and target order when attributing proof/IR costs. Treat
immutable preparation sharing separately from mutable runtime workspace changes;
qualify each against the unchanged exact reference and fixed MSL canary.
Native torn-sweep scratch belongs to each Rumoca runtime. Preserve complete
buffer initialization, clone isolation, backend-decline recovery and independent
per-row f64 checks. Use changed-input/reentry tests at the native owner, then
production coupled admission and an owned Twin run. Distinguish `FixedFirst`
through `FixedLast` service time from complete coupled-step latency, including
Modelica worker completion and response; never add overlapping worker intervals.
Read `GetReadiness` alongside compact `CosimStatus` during loading. A terminal
participant error, including a rejected generated network with no solver,
must expose its named `program_failed` readiness item. Preserve the full error
detail and classify that run as failed admission; do not count it as a slow
successful load or a physics benchmark. Removing the program closes its exact
ticket even when its physical host survives. The production
`program_attach_command` gate covers rejection and removal recovery.
Read `CosimStatus.modelica_construction_profile` for queue wait, session,
metadata and first-observation nanoseconds. Compact status retains the same
facts; a non-compiled participant reports no construction profile. Model-attributed
structured logs and Recent status also retain completed initialization timings.
Native profiles include `native_code_cache` attempts, hits, compilations and
rejected entries. Attempts equal hits plus compilations; rejected entries are
included in compilations. Record these per constructor when comparing cold,
warm and new-process loading. The worker's application-lived cache retains only
relocatable compiler artifacts, under `ModelicaCacheLimits` entry/byte budgets;
it does not retain live FMI state. Cache I/O is asynchronous, so report observed
hits rather than assuming that a disk snapshot was ready before construction.
Pure-call helpers use full-width symbolic references to fresh module-owned result
cells. The native two-live-table regression requires artifact hits with separate
mutable primal/directional state. Compare complete constructor attempt/hit/compile
counts across unchanged warm sources; code reuse alone does not establish total
loading or tick improvements. Preserve the non-Sync/lifetime contract and every
input-bit/numerical check when profiling this boundary.
Inspect the separate `modelica_native_cache_load` and
`modelica_native_cache_publish` zones when cache I/O consumes startup CPU.
Also inspect `modelica_compiled_artifact_lookup` and
`modelica_compiled_artifact_publish`: native-code hits do not imply equation
compilation was reused. Check strict compiled-artifact hit logs across fresh
processes, and distinguish source/admission changes from a warm complete input.
Keep native checked DAE reconstruction, source/URI/structural-bit identity and
external-resource eligibility intact; this cache retains no numerical state.
The shared transport buffers serializer fragments before Zstd. Native artifacts
use Bincode's direct byte-slice encoding; checked prepared-FMI records retain
their native structured decoder. Preserve both byte budgets, the binary
allocation ceiling, full consumption, explicit flush failure handling and the
owned versioned namespace. Separate a new namespace's first fill from warm
restart, without seeding it from an unrelated format. Codec diagnostics do not
establish a complete-loading or coupled-tick speedup.
For key-level attribution, enable
`RUST_LOG=info,lunco_modelica_worker::worker::native_code_cache=trace`.
The cache admission event identifies the actual storage path and entry/byte
limits. `native-code-cache-operation` events record gets and inserts under the
same cache lock, with compiler keys, hit flags, code sizes and post-insert
eviction/retention counters. Replay that order to distinguish keys absent from
the loaded snapshot from keys evicted before reuse. These events are disabled
by the normal info filter; use a separate run for timing acceptance.
Fingerprint the actual file before and after each process. Concurrent LunCoSim
sessions merge accessed/compiled deltas in the same versioned cache under the
storage transaction. Confirm that each writer uses that protocol; record its
initial fill and warm reuse separately. Matching sources alone do not establish
matching cache starting state or prove that another process caused a miss.
The `native_code_cache_preserves_other_writers_and_recent_reads` regression
rejects stale-owner replacement while preserving both retention budgets.
Check `evictions`, `retained_entries` and `retained_bytes` alongside the admitted
byte/entry limits. A repeated load whose working set exceeds retention can evict
the next functions before they are reused; a cache file alone proves no benefit.
`CosimStatus.last_coupled_initialization` retains the successful one-shot actor's
worker receipt after retirement. Compare its solver identity and
construction timings with ordinary participants' construction profiles.
Run `test_modelica_initialization_receipt.py` for retained construction facts and
scene replacement; both full and fleet-only status must clear the old receipt.
The `modelica_source_target` production gate checks the profile through rejection,
replacement and recovery. Record window focus and the effective host pacing
policy: idle unfocused windows can use a one-second low-power wait. Use the
explicit `--no-throttle` production run for throughput attribution and report
ordinary background loading separately. `[pacing] host frame policy changed`
logs primary-window focus and both effective update modes on transitions.
The command owner reserves domain-worker lanes and publication slots before
preparation. Attribute prepared-IR dispatch separately from live publication;
later ready IR must not wait for an earlier constructor. Cache hits, errors,
shared lowering and cancellation retain the reserved publication order. The
low-level `independent_preparations_advance_while_live_publication_stays_ordered`
regression covers those stage-ordering boundaries.
`modelica_live_construction_job` records queue wait and measures the native session,
metadata and initial-observation children. `modelica_live_construction_commit`
measures publication. Preparation and live sessions use separate captured bounded
worker budgets. Scene and document actors have separate live-owner pools;
each session retains its construction thread through stepping
and retirement. Compare concurrent construction intervals and end-to-end
readiness, not only summed worker time. A round transfers complete input buffers
once, runs independent participants concurrently, and admits checked results in
command order. Require exact replay, terminal observation-failure and scene
replacement evidence before a scheduling gain claim. Preserve exact
communication points, backend numerical profiles, metadata and generation
retirement. The native
compiler already shares its DAE and source map; additional metadata caches do
not address dominant native construction cost.
Inspect native artifact attempts, hits, compilations and evictions together
with session construction time. Shared immutable caches refresh recency on
successful lookup and replacement; both native artifact bounds remain active.
Compare recorded source/solver identities and cache starting conditions before
attributing a loading gain. Retain complete initialization and checked outputs;
only relocatable compiler artifacts belong in the native code cache.
For source parsing, compare the logged queue `wait` against parse/install time.
The engine-sync driver submits documents linked to non-editor scene participants
at `SimulationRequired`, the focused editor document at `Interactive`, and
other documents at `Background`, in stable priority/document/generation order.
Verify the exact registry link and participant document before attributing a
wait to scheduling. A priority repair does not establish a construction or
end-to-end loading gain; running work is not preempted.
For mixed loading and live execution, inspect `modelica_live_step_job` queue
wait and execution separately. Match its construction identity and thread to
preparation/construction spans before attributing a response-latency rise to
mailbox blocking; response latency also includes transport and main-thread
admission. Preserve the fixed-tick backlog and barrier state alongside timings.
Modelica and scripted snapshot publication use `ScalarPortMap::upsert_samples`.
The destination retains slot hints, but validates each live layout and exact
name on every copy. Name validation and the bit-exact update share one mutable
slot borrow; measure that copy separately from solver response admission.
`modelica_outputs_copy_variables` isolates the variable-map traversal and port
update; `modelica_outputs_seed_inputs` isolates omitted-input seeding. Both are
children of fixed-step output publication, so compare their total time with
that owner over the same settled window. Their call counts are per participant,
not per simulation tick. The required `ModelicaVariables.values` component owns
the full native f64 reflected snapshot; `ModelicaModel` owns clocks, inputs,
parameters and status. Serial worker admission marks the snapshot changed only
when value bits change. Reflection writes also participate in native detection.
Added models, USD participant markers and destination components force initial
publication. Run the production `modelica_variable_projection` and
`modelica_variables_missing` gates when changing this boundary, plus exact
state replay and lifecycle resource checks for late results and retirement.
Preserve reordering, same-size source replacement, target
edits, clone/clear/compaction retirement and native-bit no-op detection when
changing that mechanism. Do not infer unchanged source topology from map length
or cached iteration order. Preserve the full reflected snapshot and fixed-step
publication boundary, and measure both copying and total owner time; moving work
to another phase is not sufficient evidence of a throughput gain.
For port-write metadata, preserve the live validation boundary. `PortMetadata`
borrows static source/authority text through native `Cow` and owns dynamic text;
check both representations and content equality, then verify public metadata
and collision diagnostics. Borrowing labels must not bypass owner precedence,
stale-handle rejection, writability, bounds or atomic batch admission.
For structural publication, use the existing `PortTopologyState` owner to
observe expensive auxiliary fingerprints by their own component change ticks.
`observe_if_changed` computes a cold key and recomputes changed owners; sample
updates reuse the recorded key. A participant's new admission must refresh
auxiliary facts that could have changed while it was absent. Verify metadata
edits, same-size declaration replacement, removal and re-admission. Keep direct
backend inspection and write validation on the live contract.
For fixed-step port costs, the exchange exposes `cosim_exchange_topology`,
`cosim_exchange_accumulate`,
`cosim_exchange_holds`, `cosim_exchange_write` and `cosim_exchange_diagnostics`
inside the single propagation owner. Attribute these phases before changing
lookup or validation; do not add their nested duration to the parent.
Backend `declare_ports` publishes direction and optional slot in one `PortDeclarationQuery`
for both full inspection and exact-name, direction-constrained discovery.
Map-backed owners query the requested name directly; named queries retain only
the first matching direction and slot without constructing owned rows. Preserve the
same declaration order and live metadata checks; topology publication alone
cannot guard an owner edit before its scheduled publication pass.
For `ReadPorts` and `ReadPortsBatch`, carry the name selection into declaration
collection. Selections through `declare_map` query the existing index, retain
native slot order and validate the selected stored values through their owner.
Keep declared but unsampled scalars distinct from missing runtime endpoints;
verify every direction/owner row, endpoint removal and missing-name rejection. Measure
selected reads on a large surface and the consuming production behavior tree.
Avian resolution and named declarations use immutable name candidates derived
from `AVIAN`; metadata consumes the selected live slot. Preserve candidate order and live group
membership on removal, re-admission and mobility edits; full inspection retains
table order. The index does not authorize a write or cache world membership.
`write_resolved` shares its locator's `Arc<str>` with canonical input-write
preparation, compares the live owner with its locator, then commits within
the same exclusive World boundary without a name or single-element batch allocation.
Registry clones share the immutable backend table; registration detaches it
without changing existing snapshots or their precedence. Verify snapshot
isolation and shared name identity when changing those allocation boundaries.
Preserve live precedence, metadata, topology revision, slot, value checks, and
rejection ordering. Separately prepared batches still revalidate every write
before committing any of them.
Ownership selection retains the winning backend's optional slot for the same
World borrow. Handle creation, preparation and batch preflight consume that
result without repeating the winner's resolver. Verify one resolver call per
admission, declared owners without a slot, readable input-side sources, stale
layouts and metadata edits; do not substitute a persistent ownership cache.
The metadata callback receives the declaration's slot in that same immutable
World borrow. Coalesced inspection keeps the first slot and merged direction;
preserve each owner's shared input/output contract when consuming that slot.
The shell's steady `WorkbenchSnapshot` check compares borrowed dock and
perspective iterators before materializing owned vectors; preserve that
allocation-free stable path when changing layout publication.
The top-level menu's unique label projection is built only when the menu
registry changes. Width measurement borrows that projection and streams labels
without collecting another vector; compute perspective-tab rows once per UI
pass and reuse them for both width measurement and painting. Do not rebuild the
same responsive menu inputs during steady repaint.
For contributed top-level menus, defer collecting and sorting scripted
contributions until the popup callback runs; a closed menu should only paint its
row and publish its anchor. Use `workbench_custom_menus_render` to inspect that
row path separately from the workbench aggregate.

When a measured UI snapshot builds several indexes from the same entity
population, combine compatible marker reads into the existing query and avoid
another full-population traversal for each marker set. Keep queries over
different populations separate unless measurements show that a broader scan
costs less.
For large Builder hierarchies, derive a lightweight row index from the cached
tree and current expansion state, retain that index across repaints, then use
`ScrollArea::show_rows` to create widgets only for rows in the viewport. Rebuild
the index only after a source revision, active filter/scope change, or a branch
disclosure change reported by the shared `tree::branch_header` helper. Its
header-only allocation preserves the fixed row stride; ordinary recursive
trees use `tree::branch` to allocate their indented body. Keep foldout state keyed
by stable entity/group identity and preserve selection, drag, and tooltip
behavior on painted rows. Immediate-mode widgets are expected to be repainted;
the domain tree and entry records should be borrowed or shared, not cloned into
panel-owned snapshots. Borrow selected-entity state during paint and retain an
owned selection snapshot only when it changes; compare egui temporary values
through borrowed type-map access instead of cloning a cached vector every frame.
Gate panel-owned view models with `WorkbenchSnapshot::is_panel_visible`, and
order their systems after `WorkbenchSnapshotPublishSet`. Hidden dock tabs have
no reader and should not rebuild view data each frame; keep separate cleanup
work transition-driven when a panel closes.
In the Builder Spawn palette, use the catalog owner's revisioned
category-to-entry index. Borrow category labels and spawn entries, and retain
formatted display labels only until that revision changes. Do not rescan all
entries for each open category or build cloned entry groups before egui
determines which categories have a reader.
For Builder Ports, retain matching and expanded/collapsed entity indexes by
topology revision, filter, and expansion state; update the sampling request
only when the expanded entity set changes.
In the Inspector's material part selector, keep the entity index separate from
its display text. Format the active label only for the selected part and format
the other labels only while the dropdown is open. Retain the material-bearing
entity index for projected USD roots by selected root and `UsdStageRevision`;
rebuild it only when either changes. Recompute for non-USD roots or when the
revision resource is unavailable.
When deriving a chosen part's material controls, filter the selected root's
existing material-bearing entity list instead of walking that part's child tree
again.
For live line plots, do not copy and decimate a full history in every UI frame.
Keep one bounded build per binding, snapshot changed histories at a limited
presentation cadence, transform immutable sample snapshots on the async-compute
pool, and keep painting the last completed point buffer while the next build
runs. `ScalarHistory::snapshot()` shares completed chunks and copies only its
bounded open tail on the caller; flatten and decimate that snapshot on the
worker. In Tracy, compare `line_plot_history_snapshot` (app-thread copy time)
with `line_plot_series_build_worker` (worker time and input/output point
counts); several plots can rebuild concurrently even when each panel's paint
span is short. When live curves share an experiment plot, also inspect
`line_plot_scalar_history_snapshot`, `line_plot_scalar_history_build_worker`,
and `multi_series_plot_points_build_worker` to account for both history
flattening and plot-point preparation. Auxiliary Graphs overlays should use
the same snapshot cache and keep painting their last completed buffer while a
new one is built. Key transformed point buffers by source identity, transform
mode, and pixel width; apply line style while painting.
For experiment and live-overlay plots, retain transformed `PlotPoint` buffers
by immutable source identity, log-Y mode, and display width; build them on the
async-compute pool and borrow them while drawing. Min-max decimate time-sorted
series to about one sample per logical display point on that worker so line
geometry does not traverse every retained history sample each repaint,
preserving narrow peaks. Cache each buffer's full-data bounds so egui auto-fit
does not scan all samples on every repaint. Keep experiment variable groups
and positivity summaries in the change-gated view model instead of regrouping
names or scanning all sample values during graph painting. Time-series hover
may use monotone-X interval pruning; keep the dependency's general segment
search for phase-space curves where X can reverse.
For the entity tree, derive parent and grid facts through indexed lookups along
named candidates' deduplicated ancestor closure instead of copying every scene
entity's `ChildOf` and `Grid` membership into the snapshot.
Before optimizing a panel snapshot's change query, trace its registered reader
and confirm that reader in the live application. Remove unconsumed producers,
public exports and dependencies together. Shared widget plugins initialize
shared infrastructure; active domain panels own their view models.

For fixed-step one-shot mechanisms, measure preparation after admission has
settled. Raycast mass folding checks the existing eligible-carrier query before
building additions; its shared ordering validator still checks every source.
Use the mass mechanism's private math checks and production parity facts, and
keep an already-failing drive verdict separate from a mass-preparation gain.

Keep topology invalidation separate from the query-heavy snapshot producer:
mark the revision dirty when scene facts change, then gate snapshot work until
no prior worker is active. This lets a revision change reject an in-flight tree
result without re-entering the full ECS query system just to discard it.
For Builder telemetry, inspect `telemetry_catalog_patch_capture`,
`telemetry_catalog_patch_worker`, and `telemetry_catalog_patch_commit` separately.
Each patch handles at most 64 changed descriptors and relevant ancestor facts. There is one
initial identity enumeration; later notifications update the persistent index, affected alias
groups, and tree paths. Selection, samples, and unchanged hierarchy writes must cause zero descriptor preparation. Use
`InspectTelemetryCatalog` counters and owner timings to verify this in an owned production
session, and `scripts/api/test_telemetry_catalog.py` for repeated metadata edits with authored
Rhai verdicts. Separate initialization from settled frame costs and worker time from app-thread
capture/commit time. Use `workbench_panel_render` child zones inside `render_workbench` to
attribute painting and visible-row index costs independently of descriptor work.

For startup asset graphs, separate asynchronous source reads, discovery,
composition, and UI/physics admission. Include the file-backed scenario source
wait before its document exists.
The scripting owner retains one preparation key from asset admission through
compilation and `on_start`; a faster solver must not admit ticks during that
asset wait. Use the unchanged `modelica_observation_failure` production scene
to verify its checked initial observation and `expect_runtime_fault` registration
at global tick zero. Loader cancellation must release only its own pending key;
document handoff must preserve it until activation.
Read all known dependencies in each
breadth-first frontier through bounded batches, then merge results in stable
authored order; this avoids serializing independent branches behind one parent
at a time. Preserve the source-change reload graph when using child load
contexts. The USD layer-closure owner and its reload receipts are documented in
[`21-domain-usd`](../../docs/architecture/21-domain-usd.md#composition-closure-and-partial-scene-loading).
Keep CPU-heavy source compilation off asset I/O workers too: register literal
dependencies first, then prepare source ASTs on the existing async-compute pool.
Startup discovery must keep filesystem enumeration, manifest reads and parsing,
and installed-artifact checks off the app thread. Merge the resulting typed
registry snapshot and publish readiness on the owning app schedule. Open Twin
manifest scans can overlap, but commit in discovery order and discard results
when their owning Twin closes. Reuse authored Rhai source classifications for
the same manifest and policy revision; asset-content changes do not require a
second classification pass.
Application session metadata such as recents follows the same boundary:
load, normalize, and persist it on workers, merge typed results on the app
schedule, serialize writes, and finish the final write during shutdown.

For native source-library loading, separate scan, declaration indexing, bundle
writing and editor-index publication.
For cold indexing, compare `modelica_diagram_capture` with `modelica_icon` and
its `modelica_icon_members`/`modelica_icon_merge` children. Do not sum nested
durations. The shared declaration owner distinguishes base-class lookup before
current-class inheritance from member-type lookup after inheritance; verify
scope-read growth with the inline multiple-base fixture, inherited enclosing
types and missing/cyclic base diagnostics. The indexer consumes one admitted AST
per file and retains qualified documentation identity. Use an owned library root
without generated artifacts for cold measurements, then run the exact normal
production binary separately. Concurrent builds and simulators limit wall-time
claims.
Check interface ownership too: each scope lookup should share the cached immutable
class interface, rather than deep-cloning its complete member/header inventory.
Retain per-file fingerprint invalidation and snapshot isolation.

Selected file-backed Modelica participants queue parsed source-root requirements
at declaration publication through the existing registry, in authored prim-path
order. Trace root preparation alongside scene projection and physical-input
admission. Compile dispatch still checks current dependencies and waits for
certified initial samples; earlier library I/O must not become earlier numerical
initialization. Qualify changes with the `modelica_initialization_network` and
`modelica_initialization_failure` production gates.

For Modelica startup, separate Rumoca compile time, prepared-solve cache lookup,
`prepare_fmi_component`, and ordered result commit. Prepared-cache lookup includes
Rumoca's checked FMI reconstruction, artifact derivation and model validation;
separate native CPU samples from JSON/Zstd work before attributing it to I/O or
changing codecs. A disk hit does not bypass construction checks.
For immutable assignment ownership, measure native `producer_index_diagnostic`
construction together with prefix queries and preserve overlap, sparse-register
and full-range checks. For USD parent sharing, pair native
`parent_derivation_timing` with `native_projection_snapshot_benchmark`; include
first-read construction, retained memory and scene-edit invalidation. Fixture
gains alone do not qualify Twin loading.
When equivalent requests share the
same structural solve key, coalesce in-flight lowering instead of occupying a
second worker; preserve one result position per participant in the existing
stable commit queue. Keep the combined compiler/root/lowering admission bounded
to the largest pool count plus two staged operations, so the serialized
Rumoca owner can compile later programs while solve workers are busy without
building an unbounded DAE backlog.
Measure preparation queue waits separately from lowering. The native host
captures the typed `ModelicaWorkerLimits` resource before worker launch;
its `preparation` and `live_execution` budgets each cap workers at six after
reserving two reported CPU threads; small hosts retain one worker per pool.
Its `document_execution` budget defaults to one owner; the default total can
reach thirteen threads. Confirm all three pool counts in the
production log and distinguish preparation from construction blocking. Preserve ordered result commits
when changing that budget. Compare fresh isolated cold caches and the same
exact scene/solver/override identities; retain sampled UI/API latency and peak
process memory alongside readiness. A cache-hit replacement does not measure
cold lowering, and concurrent Cargo or simulator work limits timing claims.

For wrench-allocation cost, inspect the generated actuator count and sweep
count separately from retained algebraic variables. The maintained allocator
factors fixed geometry through `WᵀW`; preserve bounded cyclic sweep semantics,
iteration count and command limits. Validate the numeric allocation and physical
mission after reassociation, then compare preparation and settled-step profiles.

Confirm generated-network source-set stability across launches: identical
generated text must not acquire unrelated open documents as compile siblings.
Preserve exact generated target and source bytes in cache identity, and retain
authored multi-document compilation. Inspect typed
persistent-cache rejection warnings and the native preparation owner logs.
`cache=memory-hit`, `cache=disk-hit`, and `cache=miss` identify the actual lookup;
compare lookup, lowering, and total preparation durations for the same admitted
solve key. The native `modelica_solve_preparation_job` span records source key,
solver ID, exact override bit patterns, cache version and disk eligibility from
the captured plan. `admitted_library_revision` records compiler context; reuse
follows the successfully compiled participating closure, resolved solver and
exact overrides. Source equality alone does not prove key
equality or eviction. Compare the distinct admitted working set with the actual
retention limit before blaming cache capacity. Stepper construction timing alone
does not establish persistent reuse.

Count domain discovery and publication separately. Content prim GIDs do not
enter ordinary scene-network namespaces; instance identities and stage/source
events still invalidate their owners. Initial prepared results publish a
bounded four-root prefix only under their own fixed-clock admission holds,
with no prior installed projection. Live replacements remain one per Update,
and repeated roots wait for the ECS publication boundary. Preserve the oldest
request and every generation fence; do not admit physics early to shorten
loading measurements.

Application policy startup has separate Tracy spans for
`application_policy_source_prepare_offthread`,
`application_policy_compile_offthread`,
`application_policy_prestartup_wait`, and
`application_policy_activation`. The activation span separates
`application_policy_clear_registry`,
`application_policy_validate_order`,
`application_policy_validate_prepared_hooks`,
`application_policy_install_prepared_hooks`, and
`application_policy_publish_registry`. Preparation starts during runtime
plugin construction; the compile span includes authored installer-order
selection. Owner filtering and per-hook contract/arity validation run in
`application_policy_filter_manifest_offthread` and
`application_policy_validate_prepared_hook_offthread`. `PreStartup`
validates the selected order and its manifest records, then registers the
prepared callables before Startup consumers. Keep source text and callables in
the Rust preparation bundle; the application selector receives hook
identities only. Compare the activation children before moving additional
registry work across the lifecycle boundary; preserve authored install order
and visible failure reporting.

Schema declarations are compiled by `openusd-build` and installed through
`schema::native_registry`. Attribute native registry construction and the
one-time `SchemaMetadata` projection when profiling startup. Schema availability
has no asset-registration cadence. Project UI hints use the shared metadata
decoder at build time; native USD owns declaration and fallback resolution.
Compare loading and settled runtime separately before making performance claims.

Read-only USD projectors use `CanonicalStages::reader_for`.
Generation-zero reads consume the worker-prepared plan, and
later authored generations consume the live canonical stage. Do not call
`get_or_build` just to read startup facts. Use the prepared schema/path indexes to
find initial candidates. Change batches should advance unrelated edits and
invalidate only owners whose consumed paths changed. If a live generation has
no exact worker-prepared plan, snapshot the canonical stage recipe on its owner
thread and submit replacement facts through shared bounded `AsyncWorkAdmission`;
cap this owner's pending stages, then commit only when asset-plan identity and
canonical generation still match. Hold startup progress only until the first
topology index commits. For later generations retain the last committed facts
so current simulation continues, while new or reprojected prim admission stays
queued until replacement facts commit. A required preparation failure,
including a host without worker transport, must be visible through the scene
fault owner. Do not run a whole-stage topology traversal synchronously in
`Update`.

For USD simulation admission, check whether visual-only prims consume the
participant prefix. The vehicle owner excludes ordinary prims using only its
current-generation topology index, alongside existing preview exclusion;
missing or stale facts remain eligible. Keep the same simulation-work limit,
stable stage/composed-path order, and readiness markers.
Inspect physical-wheel fixtures through the shared reflected `PhysicalWheel`
contract, and run simulation queries under their real Twin-owned scenario route.

For route-edit latency, record four separate spans: the bounded Rhai input
hook, the durable route `ApplyUsdOps` and its one incremental projection,
reference-marker admission, and `UpdateUsdCurveView`'s stroke preparation and
terrain index-patch publication. Inspect `surface_annotation_incremental_worker`
separately from app-thread admission and render uploads. Fragment lookup uses
bounded spatial bins;
verify idle camera/terrain changes do not rebuild the stroke index.
Visited-marker recoloring uses the existing route-view key and must not resubmit
unchanged ribbon geometry. A ribbon seed authors only its reference root and
anchor; wait for the reference owner's projection before `UpdateUsdCurveView`.
The marker asset owns its geometry and pointer-interaction defaults. A rejected
view command is terminal; it must not keep the route's admission flag pending
and submit the same rejected edit every tick.
Ribbon presentation preparation is presentation-only and must not issue another
`ApplyUsdTransientOps`, change document generation, or run inside the fixed
scenario event that observes a projected route. Compare the UI hook and fixed
tick against frame/physics budgets; a fast worker result does not excuse a
slow synchronous query or document edit in the input path.
For surface-annotation flicker, inspect `InspectUsdCurveView`'s
`displayed_surface_binding_count` and `surface_texture` across pending revisions.
Route edits and arriving wheel sources retain the displayed texture until the
patch commits; current-revision readiness remains a separate result.
The `route_surface_annotation` gate checks retained bindings and texture identity
alongside long-path publication and missing-coverage rejection. It edits one
vertex after admitting 2002 legs and verifies `index_updated_segments`,
`index_touched_nodes`, `index_patch_bytes`, `index_full_uploads` and actual render
`uploaded_patch_bytes`/`uploaded_patch_batches`, with `uploaded_patch_sequence`
at or beyond the source's `index_upload_sequence`. Ordinary movement must not copy
history, resize the texture or upload its full contents. `InspectVehicleTrail`
reports producer `annotation_edits`; compare that count with active wheels, not
retained sample count. Initial admission, capacity growth and explicit grid reset
are separate full-upload events.

For general `SpawnEntity`/`DeleteEntity`, measure the command, USD add/remove,
live structural reconciliation, and referenced asset admission separately.
Keep edit-to-projection wall time separate from frame time. A new referenced
entity may still be preparing its source closure across frames; the scene
admission owner retains the source handle and settled reference facts. Measure the
first spawn, a warm spawn, and deletion separately, while checking render-frame
and fixed-step cadence during each. Use the production spans
`scene_spawn_entity_command`, `usd_apply_ops_document`,
`usd_document_apply_change_set`, `usd_reference_layer_closure_merge`,
`usd_live_structural_reconcile`, `usd_visual_projection_batch`,
`scene_runtime_spawn_commit`, `usd_live_subtree_despawn`,
`scene_delete_entity_command`,
`scene_delete_entity_persist`, and `usd_apply_one_op_document` to separate
command, document, reference, projection, and removal costs. Warm instances of
one immutable asset recipe should bypass layer-byte cloning and comparison;
asset reloads use a new recipe identity and still merge changed bytes. Path
reconciliation uses the lifecycle-maintained stage/path index, not a full
population scan per changed prim. Raw-file spawn identity collision checks use
the lifecycle-maintained API identity registry and a hash set of queued roots,
not a world scan or a walk of all pending inputs. Document projection waits only for
references named by that edit's typed AddPrim operations; an unrelated
reference must not extend its completion time or document-projection hold. An
authoritative reference batch retains its document-projection key until its
active roots are projected. A completion wait is not itself a frame stall.
Measure native composition and bounded ECS materialization separately; render,
physics and query reads retain one scene-stage owner. Deletion removes the
native root and its scoped ECS subtree.
`scene_runtime_spawn_commit` measures each changed prim's live ECS structure
insertion, including deferred reference descendants. `usd_visual_projection_batch`
measures the bounded per-update component and visual projection pass; keep its
frame cost separate from total time until the last descendant is admitted.

Use `usd_projection_plan`, `usd_projection_facts`, and
`usd_projection_native_paths` to separate native composition, immutable fact
extraction, and dependency-path preparation. Preserve native graph errors and
live-edit resolution. `test_native_material_terminals.py` drives eleven authored
checks for native terminals, live PBR edits, cyclic-graph rejection and unchanged
intent after rejected edits.

For snapshot attribution, enable
`RUST_LOG=info,lunco_usd_bevy_stage::projection_plan=debug`. The bounded
`usd_projection_transforms`, `usd_projection_attribute_values`,
`usd_projection_attribute_metadata`, `usd_projection_time_samples`, and
`usd_projection_asset_sources` spans separate the extraction groups.
Additional bounded spans attribute traversal, property enumeration, connections,
relationships, material bindings, prim identity/visibility and collections.
Compare **inclusive** `usd_projection_facts` durations when adding child spans;
self time moves into those children and is not a loading improvement by itself.
The ignored `native_projection_snapshot_benchmark` includes both omitted stacks
and authored double-precision translation/rotation/scale stacks. Compare both
snapshot-only and complete recipe preparation with exact geometry checks; the
paired transform reader shares native composition without changing rendering's
decomposition/convention order. Alternate independently copied test artifacts
when host workloads change, and keep these timings separate from Twin readiness.
Attribute preparation retains native handles for one prim across its field
reads. Check live/prepared equality for values, types, authored opinions,
connections and samples, including blocked and unset values. Measure complete
recipe preparation; handle construction alone excludes the real consumer.
Native typed relationship enumeration avoids target-resolution probes on every
attribute. The prepared reader retains empty relationships; verify the
`native_relationships` gate as well as the complete snapshot timing.
For attribute metadata, inspect `usd_projection_attribute_metadata` and the
shared `SchemaMetadata` lookup. Its immutable table groups declarations by name;
only those declarations participate in ambiguity checks. Schema-qualified reads
remain exact, without allocating lookup keys. Verify both ambiguity rejection
and native unit/inspector hints before comparing complete snapshot time.
For collection costs, inspect `usd_projection_collection_expansion` and the
`prepared native memberships and surface terminals` counts. Immutable preparation
reuses successful expansions only when OpenUSD's membership queries compare equal;
evaluator queries remain distinct. Compare inclusive collection/snapshot spans
on matching Twin sources, and qualify native membership errors through the
`usd_collection_lint` production gate. The focused stage-read test covers
exclusions, property expansion, expressions and fresh membership after edits.
For material costs, compare combined binding/identity and complete snapshot
durations: surface extraction is inside `usd_projection_material_bindings`.
`usd_projection_surface_terminal` runs once per distinct bound render material
within that snapshot. The same debug receipt records terminal requests and
resolutions. Preserve native binding and terminal errors, shared and empty
terminals, and fresh resolution after source changes. Live reads remain current;
the map does not survive a snapshot or own native stage handles.
`usd_twin_source_decode` and `usd_twin_stage_input` measure the two worker jobs
before scene mounting. Time-sample extraction resolves each native table once;
value capture and sampled asset-source capture reuse that existing table.
Keep native clips, sample order and default-time behavior intact; the focused
snapshot test compares sampled values and asset-source layers with live USD.

For Twin-open stalls, profile the active Twin policy loader separately from
policy activation. Native manifest and Rhai source reads should run through
bounded `AsyncWorkAdmission`; `twin_policy_source_prepare_offthread` measures
that preparation and `twin_policy_activate` marks the lifecycle-bound commit.
Verify that stale completions are discarded after a Twin switch and that the
active `assets_mounted` plan and first authoritative tick wait for activation,
with the app/UI schedule continuing. Browser builds still use synchronous
WebStorage reads because they do not have a worker transport.

One-shot `RunRhai` and tool callbacks share the prepared `ScenarioDriver`
engine. When profiling a callback stall, separate one-time startup/prelude or
tool-generation refresh from request execution; do not rebuild the engine for
each callback.

For Bevy visibility costs, distinguish the active scene camera from auxiliary
shadow-map subviews. If adapter capability supports GPU culling, put
`NoCpuCulling` on the scene camera so camera frustum work can move to GPU
preprocessing while per-mesh light and shadow visibility remains CPU-owned. Do
not put it on shadow-casting `Mesh3d` entities: Bevy excludes those from its
CPU-built per-light visibility lists. Unsupported adapters retain CPU camera
culling. Measure GPU headroom and compare a clean FPS run plus a separate Tracy
capture; do not trade away shadows or authored quality to reduce CPU time.

Local-light shadow views are filtered at the render boundary: compare a point
light's finite range sphere or a conservative bound of each extracted spotlight
frustum with every active extracted `Camera3d` frustum and compatible
`RenderLayers` before Bevy prepares shadow views. Do not mutate authored lights,
omit offscreen cameras, or infer relevance from the light origin alone.
Disjoint layers prove irrelevance even when spotlight bounds are unavailable.
Uncertain bounds and boundary cases keep the map for compatible layers. Restore
the extracted main-world shadow intent before every frame's relevance check;
native light extraction is incremental, so focusing a parked preview must
restore its shadows without requiring a light edit. This saves only maps
provably irrelevant to all outputs. Bevy's main-world point/spot caster pass
is admitted only while either native light component exists. Verify that its
zones disappear in a world with no local lights and that dynamic point/spot
addition still wakes the original native system. Shadow-LOD extraction remains
active for its native consumers, including directional shadow views.

Before adding shadow-content reuse, measure per-map native caster poses and
light projections across consecutive frames; stable counts are insufficient.
Reuse admission must also cover mesh/material/pipeline contents, skin/morph
inputs, backing texture identity and atlas-layer assignment. Spot maps share
the native directional atlas, so light or cascade changes can shift a layer
without changing the light entity. A pose digest is diagnostic candidacy only.

Count shadow roots independently of active cameras. Invisible extracted local
lights retire their native `PointAndSpotLightViewEntities` through the renderer
adapter; Bevy's cleanup observer removes the associated views. A deferred
retirement must retain a light already re-extracted in that boundary. Verify
one/many parked-preview counts and text-to-Visual reactivation separately from
authored shadow intent or a camera's `is_active` flag.

The render adapter routes native point/spot roots selecting Core3d to
SharedShadowView after native light preparation. Check the compact schedule's
native GPU-plugin/device admission, global indirect-prepass filters and final
buffer flush order. Keep custom roots, cameras and directional cascades on
their selected schedules. Verify the same early/late shared and camera shadow
pass inventory before and after, plus visible shadow maps and normal frame
timings. Fewer schedule systems alone does not establish faster frames.

Treat Bevy `Changed<T>`/`Added<T>` filters as population filters, not free
events: a no-match query can still inspect candidate entities, and separate
`is_empty()` queries can repeat that work. Combine compatible invalidation
sources into one `Or` query when they drive the same decision. If this remains a
hot path, audit every writer before adding a source-owned event/revision/dirty
set; once all writers are accounted for, prefer that signal over an
always-evaluated `Added<T>` population query in a run condition.

Keep readiness checks separate from reconciliation requests. An unresolved
async participant may require a cheap readiness check on later frames, but a
shared revision that wakes full topology or causal-graph work should advance
only when topology or endpoint lifecycle facts actually change—not merely
because readiness is still pending. Guard `ResMut` revisions by comparing the
owner's state before calling a mutating method: Bevy marks the resource changed
on mutable dereference even when an idempotent method leaves its value alone.

`SimComponent` input/output shape is tracked by `lunco-port-core::ScalarPortMap`.
Its identity key changes at insert/remove/clear boundaries, while numeric sample
writes leave it stable; use borrowed-name `set` in hot publishers and strict
`set_existing` for already-declared inputs so stable keys do not allocate. A
strict write separates whether the name exists from whether its value changed.
Callers that bypass Bevy change detection should mark the component changed
only when the sample differs; generic `InputPorts` follows the same rule.
`PortMap<T>` keeps authored names at its boundary and resolves them to dense,
process-local slots for the fixed propagation data plane. Map-backed ports,
static scene-property inputs, shader inputs, Avian ports, and catalogued
link-class outputs use owner slots. Shader live values are predeclared for
driven parameters, and authored shape fingerprints update only at structural
setters. Every owner that can change a declared surface publishes the shared
`PortTopologyRevision`; compiled wire handles are rebuilt on that revision or
on connection changes, and a stale slot is never
retried through a name lookup. Numeric samples do not invalidate the compiled
fabric. Do not re-hash every map when `SimComponent` changes for ordinary
physics values. `PortHolds` advances its own revision only for effective intent
changes; rebuild its target-index projection on that revision or wiring changes
and reuse the aligned value buffer on steady ticks. Do not probe every target
against the hold table or clone its presentation snapshot per tick.
Readable `inputs:*` connection sources use a distinct input-side slot reader;
write-only inputs can be targets but never become fabricated readable sources.
Success diagnostics publish once per compiled target and retain a dense
acknowledgement until fabric rebuild. Scene teardown clears the shared
propagation cache with the scene's diagnostic ledger. Failed writes still check
the live ledger; an acknowledgement must never admit or bypass a write.
Compiled targets cache surface presence until topology invalidation;
pending/broken snapshots share compiled names as `Arc<str>`, and fault warning
keys are formatted only on first failure rather than every fixed step.

Exclusive systems that query pending components must retain a system-local
`QueryState` rather than create a fresh world query on every update. Its normal
`iter(world)` refreshes new archetypes while reusing the established matches.
Keep pending-source sampling, source ordering and reload admission unchanged;
verify scene replacement and the unchanged exact replay reference.

For deferred USD projectors, keep one entity-work set per owner and feed it
from the complete lifecycle boundary: identity arrival, projection readiness,
invalidation, removal, and scene teardown. The same applies to deferred adapter
steps such as wrapping a Modelica model into its shared port surface. Keep a
single bootstrap discovery for entities predating plugin installation, and
retry only work whose authoritative stage/readiness input is still pending.
When a projector caps per-update work, select the bounded prefix in stable owner
order and retain the remainder; avoid sorting the entire pending batch on the UI
thread before applying that cap.
For render-free USD discovery, wake candidates from changed prim identity,
canonical stage generation, or stage-asset events, and keep only transient
runtime prerequisites in the per-frame retry set. Dormant `BasisCurves` without
the owner's relationship must be revisited when their stage changes, not
queried from USD every update.
The celestial admission projector uses the shared `PendingEntityWork` queue for
one bootstrap and later lifecycle arrivals; its idle run condition reads that
owner instead of scanning all USD prims. Keep `CelestialProjected` on the scene
root after its source classification because static-light resolution consumes
that boundary. Ordinary non-root prims do not need a completion command.
Preserve authored-field candidates with a `lunco:` property, the scene root,
`DistantLight` parent-body fills, and non-root `LunCoEpochAPI` diagnostics.
When initial candidate processing spans many owners, queue the bootstrap IDs
once and drain a fixed-size batch in stable entity order per app update; keep
the remainder queued so a large scene cannot monopolize one UI frame.
For USD visual projection, read both the system body and its Tracy
`system_commands` flush: `frame_budget` bounds prim binding but cannot bound
the deferred command batch applied after the system returns. Cap prim work
items per update as well as elapsed time. Direct-child admission has its own
`child_spawn_budget` and `max_child_spawns_per_update`; the parent keeps its
awaiting/projecting markers until its direct children enter the ECS queue, so
scene readiness must remain held while batches drain.
Within each prim's deferred work, use native component bundles for facts that
commit together. Primitive shape identity accompanies its pending mesh request;
grid cell and spatial projection publish together. Clear projection markers and
retired geometry definitions in one removal bundle, retaining the owning
failure, generation and replacement fences. Measure both prim and child
flushes and CPU mesh publication. Verify complete event counts: a fixed startup
time slice can omit work under contention. Separate initial and replacement
admission using a verified inactive gap or an owner marker, and record background
builds throughout each comparison. Preserve the exact physical replay reference.
For large procedural terrain fields, profile both scatter execution and its
deferred entity commands. Admit generated body bundles and visual components in
stable bounded batches while physics continues. Keep the terrain's applied
marker pending until bodies and required visuals are committed. Cancel queued
entries on refresh and teardown. Publish the terrain's `TerrainScatterContent`
fingerprint at queue admission, separately from completion. An unchanged
recomposition must preserve a partially displayed field; check that startup
issues one scatter request and that an actual recipe change still refreshes it.
Gate sparse lifecycle work on an existing pending marker or owner queue; an
idle Update should not scan lifecycle state for a request that did not arrive.
For joint admission, use one combined query over the existing
`PendingUsdJoint` and `PendingJointAdmission` markers to gate the readiness
scan. Reconcile candidates before the admission commit so the stable batch
boundary is preserved.
When a worker prepares facts for a composed plan, result commit must use the
same prepared-plan identity for any derived cache. If the canonical owner
records that exact plan as the source of the current live-stage generation,
prepared facts remain valid at that nonzero generation until a live edit
advances it. Later generations require complete change history or topology
extraction from that exact generation's recipe snapshot on a worker. Otherwise a
cache-key mismatch can repeat a full-stage scan during result publication.
For dependent-stage refresh, pair `usd_canonical_stage_asset_sync` with
`usd_sim_prepared_topology_cache` and `usd_sim_joint_topology_scan` to verify
that publishing the new asset plan neither reopens the canonical stage nor
repeats its prepared topology extraction.
When initial preparation already builds composed type or API-schema indexes,
run one-time topology and vehicle-output extraction against the prepared reader
on a worker and use its indexed candidate query. Combine overlapping schema
queries and property-prefix checks into one candidate traversal, so live
canonical readers do not walk the stage once per consumer. Return each
candidate's type, API-schema, and property-prefix facts with its path so later
classification does not repeat native schema reads. Build per-prim candidate
sets and authored vehicle-port lists from the prepared snapshot; skip extraction
for unrelated entities while preserving their readiness marker. Do
not materialize every prim path on the UI thread to find a small set of joints,
attachments, vehicle roots, or policy prims. A canonical topology refresh encodes
its captured recipe, composes a worker-local stage and reads topology directly
through `StageView`; only topology facts return to the World. It does not build
the visual projection snapshot or extract material and animation facts.
The live canonical stage stays on its owning thread. Invalidate cached topology
from `UsdSceneChangeBatch`: resynced paths that match an indexed source or currently
carry a relevant schema, and info changes on indexed source prims, require
refresh; unrelated paths only advance the cached generation. Transform-only
info changes on topology source paths also advance the generation without a
topology rebuild; structural resyncs or mixed edits still refresh. Check
intervening changes before accepting prepared worker output. If another live
read or the change observer has already made the cache current at that exact
generation, discard the late prepared result instead of scanning the stage
again. `usd_sim_prepared_topology_cache` records whether this check found a
current-generation cache; pair it with `usd_sim_joint_topology_scan` to confirm
that a repeated reconciliation did not scan the stage.

For whole-index projectors such as USD telemetry, use one initial bootstrap,
then coalesce relevant insert/remove observers into an invalidation flag. Keep
stage-generation and asset-store invalidation as scalar checks. Do not repeat
the same `Added`/`Changed` population filters in both the run condition and the
projector, and do not reproject until the index is invalidated. When structural
projection arrives in bounded batches, keep the index dirty and coalesce further
invalidations until the owning queue settles; clear derived outputs once for
that dirty interval. Keep processed entity identities in the owner's index so
prim progress does not enqueue a deferred ECS marker command for every prim;
clear the set with the stale outputs on invalidation. This still skips ordinary
non-declaration prims on later projector passes without imposing marker insert
and removal flushes on the app schedule.
Reuse composed network-membership facts when admitting a member's own telemetry
declaration. Its generated wrapper owns the sampled alias; a metadata Scope still
needs a measured target. Use `ListTelemetryChannels` with an exact `name` when
checking one retained channel, so its metadata read stays bounded.

When a USD reader already exposes `has_authored_attribute`, use it to test one
known property instead of enumerating every attribute name. If one enumeration
feeds multiple derived port sets, derive them together from that single result.
Temporary lookup indexes over immutable ECS queries should borrow path and port
surface data instead of cloning those maps for a one-pass reconciliation.
Build compatible per-entity indexes in one query traversal rather than running
separate full-population passes for each index.
Modelica live metadata comes from the worker's checked FMI scalar inventory.
Construction publishes it once; ordinary steps move only observations. Admit the
inventory under the same session/source fences as the stepper and retain its
immutable identity. Draft document edits must not invalidate live telemetry
metadata or trigger component-name scans. Move first or changed facts into the
shared signal registry, which owns comparison and storage; do not retain a
second `SignalMeta` in the producer. `modelica_telemetry_channel_metadata`
attributes that publication with the Modelica telemetry debug filter enabled.
Cache each runtime producer's stable `SignalRef` identity for the session and
borrow it when recording an existing channel. Avoid reconstructing and cloning
signal paths for every due sample.
For burst channel publication, keep retention depth as a logical limit and let
the history buffer grow with recorded samples. Do not reserve every channel's
full retention window when most new channels contain only their initial sample.
When multiple runtime consumers inspect the same composed owner, acquire its
canonical reader and child list once, derive owned typed facts for each
consumer, then release the stage borrow before mutating ECS. Preserve the same
typed resolution and commit helpers across initial and live projection paths;
do not introduce a second fact cache.
The presentation `project_env_settings` owner retains authored exposure and
bloom facts with the exact stage-plan `Arc` and the matching environment prim
paths. Reuse them across transform-only batches and info edits outside those
paths; environment-path edits, resyncs, generation gaps, or plan replacement
require a fresh composed read. Compare the stage signal without allocating a
replacement vector on steady updates.
When multiple USD consumers need the same composed-stage fact, put its
generation/instance-keyed cache at the shared fact owner and reuse that cache;
clear it at the scene teardown boundary instead of keeping consumer-local copies.
For derived marker sets, compare current membership with the desired set and
apply only additions/removals; unrelated rebuilds must not emit lifecycle churn.
Removal invalidation should be qualified by the entity's authored USD identity
and relevant endpoint capability, not by a generic component removal alone.
Extract a USD program's declared interface once at admission and reuse it for
validation, diagnostics, and publication instead of re-enumerating attributes.
Derive a domain root's synthesizer once alongside member-role discovery and
reuse the root-scoped selection during projection; do not traverse a large
component collection again to repeat its role-schema queries.
In Tracy, compare `domain_member_role_discovery` and
`domain_synthesizer_selection_cache` against the enclosing
`project_domain_islands` interval to attribute any remaining app-thread tail.

Keep invalidation domains distinct: a wiring/topology latch may be raised by
endpoint arrivals and must not automatically trigger domain discovery. Live
canonical edits publish `UsdSceneChangeBatch` with stage generation and
resynced/info paths; route those paths through the owning stage/root/member
index. Transform-only info paths advance the generation without re-reading a
Modelica network; structural and other member/root changes still invalidate
affected roots. Stage-asset changes and missed generation batches requeue only
roots on the affected stage. Reserve the all-prim discovery for initial
admission, and keep entity arrivals on their queued-entity path.
Generated-source document sync should share the `PendingEntityWork` contract,
and its metadata publisher should consume the owner-published dirty flag rather
than adding a parallel change query.

When gating a dependency's multi-system transform schedule, distinguish its
per-update output flags from authoritative input changes. Preserve any required
settle pass after a floating-origin change, and test the change → settle → idle
sequence so a gate neither stays open forever nor closes before propagation.
Keep the settle marker between admitted passes so the idle admission check does
not replace expensive propagation with a full-grid scan.

For physics, distinguish persistent environmental state from transient
commands: use the engine's persistent acceleration or passive non-waking force
contract for gravity and contact support, and reserve waking force/torque writes
for authored drive, braking, or actuator commands. Do not emulate this with a
timer, sleep threshold, or a second cache.

Use the existing cache/revision owner and preserve the USD projection boundary.
Do not add a second cache, a timer, a compatibility path, or a quality fallback.
If the hot path is not established, stop after source inspection and capture a
bounded profile rather than guessing.

For analytic ephemeris cost, count exact body/epoch evaluations in the native
capture before changing cache scope. Reuse maintained position-only evaluators
and their declared units/frame through the existing typed adapter. Qualify a
dependency update with frame/time math and production celestial/landing gates;
an isolated evaluator benchmark does not establish a complete-tick gain. See
[the native celestial review](../../docs/reviews/celestial-native-evaluation-2026-10-24.md).

Profile the installed Avian 0.8 systems for joint preparation, native XPBD,
contact generation, contact recycling, constraint solving and swept CCD.
`PhysicsPerformance` exposes native solver and collision durations alongside
complete step samples. Validate graph endpoints after scene replacement before
attributing contact cost to active geometry. Record source hashes, ordered
inputs, collider counts, CCD filters, substeps and iteration settings for each
comparison. Rerun the production joint, drive, gear and continuous-contact gates
when changing those owners. Preserve native scheduling and enhanced determinism;
thread or batch count is not a correctness verdict.

For first-step joint replay differences, compare actual native pass order and
prepared constraint/body inputs before changing tolerances or blaming coupled
solver workspace reuse. XPBD corrections share body state: ECS table packing is
not a valid execution order. The native owner sorts each joint type by live
entity index in position and velocity passes. Preserve deterministic entity
admission. Velocity relaxation borrows and orders query items once per call,
then reuses them across its iterations without a persistent cache. Preserve
the exact production replay reference; exercise unrelated
archetype moves and joint disable/re-enable in the native regression. Include
the unchanged serial/default-thread and jitter profiles in qualification, and
measure ordering cost separately from numerical correctness.

## Evidence

For frame distributions, query `EnginePerformance` after a quiet measurement
window. It reads the core health owner's chronological raw frame history, including
while simulation is paused; `frame_sample_count` identifies the last observation
and `history_capacity` bounds retained rows. Keep adjacent collection windows within
that capacity when collecting every frame, and deduplicate by process identity and
sample number with wrapping arithmetic. `available: false` means frame diagnostics
are not installed or have not published; do not fabricate zero FPS. The
`engine.frame_time` telemetry channel is sampled on the simulation clock, so catch-up
ticks may repeat one render diagnostic. It cannot qualify a render-frame percentile.
The production `engine_performance.rhai` gate covers rolling paused-frame publication
and unavailable headless diagnostics.

For whole fixed-step timing, use the `PhysicsPerformance` query's
`step_time_samples_ms` history in every host that installs the shared USD
physics runtime. It returns retained `PhysicsTotalDiagnostics.step_time`
samples, one per completed physics step; compute p50/p95/p99/max from that
array after the measurement window. The query also returns the current
`step_time_ms` and `step_number`, and the editor publishes the same current
sample through `engine-health.physics_step_ms`. Query once per window because
`PhysicsPerformance` also counts live topology and is not a per-step sampler.
Compare its native `contact_graph_pairs` with
`contact_graph_pairs_missing_entities` and
`contact_graph_pairs_without_colliders` after replacement or live projection.
The latter two count invalid endpoint pairs, including sleeping contacts;
an excess graph-node count alone does not identify a retired contact. Keep
the audit on demand and trace a confirmed invalid endpoint to its lifecycle
owner before changing cleanup or contact admission.
The editor-count diagnostic additionally checks it at one-second checkpoints
to reject a stall after an early advance, along with full readiness and stable
physics topology. It also rejects retained errors from `RuntimeDiagnostics`,
including scene-scoped telemetry delivery holds that do not fault physics.
That polling overhead is part of the diagnostic window.
`scripts/perf/usd_editor_tabs.py` compares yaw, pitch, target, distance,
projection, orthographic scale, measured viewport rectangle and window scale
factor both within a window and between equal-camera windows. Its negative
guard tests cover rotation, resizing, faults, retained runtime errors, late
waits and stopped physics.

Record the clean FPS window, physics and render timings, Tracy capture path,
scene/settings, and whether the result is startup or settled. Rebuild the
production binary after a code change and repeat one clean A/B plus one Tracy
diagnostic capture. Link the changed owner and state any platform/GPU evidence
that was not available.

For fixed-step versus UI diagnosis, query `SimulationTimingProfile` during the
settled owned run. It summarizes the latest 240 fixed ticks and 240 app-update
fixed-loop bursts: p50/p95/p99/max service time, per-tick realtime service-budget
exceedances, fixed steps per app update, and retained pending simulation duration
(`fixed_loop.latest_pending_simulation_secs`, whole ticks plus fractional
overstep). The owner drains at most 64 complete cycles per update and yields
between complete ticks after one fixed quantum of monotonic wall-clock service,
while admitting the full running delta. An expensive first tick still completes;
the remaining demand stays queued instead of becoming a synchronous 64-tick
batch. This fairness boundary does not make an individual slow tick fast. Its separate `Time<Virtual>::max_delta` diagnostic
detects unexpected clipping and should remain zero. Pair this with
`QueryTelemetryHistory` for `engine.frame_time`, which covers the full app frame.
Use `CosimStatus` step diagnostics to separate solver service from end-to-end
response latency. Fixed-tick service brackets `FixedFirst` through `FixedLast`;
it excludes asynchronous solver work and completion admission outside that
interval. Do not label that number complete coupled-step time or use it alone
to certify a coupled-step budget. Trace the causal worker critical path and
response/commit boundary without adding overlapping worker spans.
`SimulationCompletions` runs in `First`, after message
rotation and before `ClockProjectionSet`, and in the native driver's bounded
completion wait. The shared fixed runner also services it once before checking
admission for a held pending cycle; compile requests remain in
the `Update` lifecycle cycle. Preserve session/source/communication-point
validation and the all-causal-participant hold when changing this boundary.
The profile is read on demand and is not a replay or UI-isolation verdict; a
nonzero clipped-time total means wall-clock demand was already excluded by the
clock admission, distinct from the recoverable pending duration.
When changing this boundary, exercise the rendered production
`fixed_budget_recording.rhai` gate with a fresh writable `output_dir`; it runs
25 FPS recording at 64x to require several budgeted updates per logical frame.
Require three saved frames, completed cycles yielding with pending duration,
zero clipped demand, and no readback while whole cycles remain queued.

For CPU outliers, report p50/p95/p99/max for the app-thread frame and the
authoritative fixed-tick transaction, plus fixed steps per app update and
simulation deadline/backlog. Do not infer these from mean Avian time alone or
add overlapping Bevy schedule spans. Bevy drains accumulated fixed schedules
synchronously before `Update`: a fixed `Time<Fixed>` delta is not proof of a
constant wall-clock physics rate, and a catch-up burst delays UI/input. Never
improve UI timing by silently discarding authoritative overstep. If both
wall-clock physics cadence and UI isolation are required, measure the whole
simulation-owner boundary and consume immutable snapshots from the UI/render
side; separate cycle labels alone do not provide thread isolation.

For physical terrain Sun visibility, enable only
`RUST_LOG=info,lunco_terrain_surface::occlusion=debug` for attribution.
`terrain_occlusion_coarse_traversal` and `terrain_occlusion_refinement` separate
raster traversal from unresolved interval refinement. Their self times exclude
the nested bounds-miss, exact-footprint and point-sample spans; do not add
inclusive parents to their children. These spans are disabled by the normal
info filter. Compare the complete surface/source cohort and preserve exact ray
coordinates, conservative bounds and terminal budget errors when optimizing it.

Inspect effective compiler profiles as well as runtime work. The root's explicit
development overrides optimize `lunco-physics`, the workspace glam math owner
and `lunco-luncosim-simulation`, which instantiates native systems with USD hooks,
at level 3, retaining development assertions and overflow checks. Cargo's
dependency wildcard excludes workspace members, and generic native kernels can
be instantiated under their caller's optimization level. Preserve these native
math profiles when changing workspace membership; verify the physical gates and
the exact normal artifact after a profile change. See
[Cargo profiles and generics](https://doc.rust-lang.org/cargo/reference/profiles.html#overrides-and-generics).

When `tick_rhai_scenarios` or `tick_rhai_scenario_visualization` has an outlier,
compare the nested `rhai_user_scenario_hook`, `rhai_user_event_hook`,
`rhai_native_task_tick`, `rhai_mission_event_projection`,
`rhai_prelude_scenario_driver`, and `rhai_user_visualization_hook` spans. Their
fields identify the scripted entity, hook/driver, and queued-event count. If
the outer scenario system is slow while these spans stay short, the time is in
driver traversal or elsewhere in the exclusive fixed pass; do not attribute it
to Rhai evaluation or move authoritative World access to a worker on that basis.

For exclusive systems that borrow `&mut World`, inspect individual Tracy event
timestamps and durations, not just aggregate averages. Align long calls with
Twin-open and readiness milestones: one startup outlier can stall UI even when
the same system is nearly free on settled frames. If the outer system is hot,
attribute time to its internal owner operations before choosing an async
boundary or cache.

For startup `drive_engine_sync` outliers, compare
`modelica_engine_completion_snapshot` (engine lock and typed completion move)
with `modelica_engine_completion_install` (generation validation and document
syntax/index installation). The latter identifies the document and generation.
`modelica_engine_syntax_prepare` measures the separate session AST copy. Native
parse workers execute it before taking the session lock; WASM transport still
performs that ownership copy on the page. Document results retain the worker's
shared AST and exact diagnostics through the FIFO. Its per-update budget covers
installation, but one large index can still exceed the budget. Attribute that
remaining tail before changing the index preparation boundary.
For physics telemetry, `physics_telemetry_source_retention` separates registry
retention from kinematics/contact collection and marks metadata discovery with
`metadata_dirty`. Compare first discovery with settled sampling before changing
the owner or its cadence.
`physics_telemetry_channel_metadata` covers first or changed descriptions;
`signal_scalar_channel_create` covers initial history/map publication, and
`signal_scalar_history_tail_growth` covers tail allocations. The signal registry
owns metadata values; the physics producer retains only a publication flag.
The tail span selects a full tail before bounded eviction and append, so a
capped non-allocating append can also appear. It is not an allocation counter.
Enable these diagnostic spans with
`RUST_LOG=info,lunco_modelica_core=debug,lunco_usd_sim_telemetry=debug,lunco_signal=debug`; they are
disabled by the ordinary info-level filter.

When `system_commands` dominates after a producer, inspect the number and shape
of its deferred writes. For a large set of entities receiving the same bundle,
use Bevy's existing batch command when its fallible/overwrite semantics match
the owner, and preserve the owner's iteration order when it determines stable
IDs. Relationship bundles such as `ChildOf` can use the same fallible batch:
Bevy runs their relationship hooks per entity, in batch order, while continuing
past stale entities.

For `compute_local_gravity`, distinguish flat fields from position-dependent
surface fields. Steady flat fields use the archetypal missing-field query;
motion alone does not require recomputation. Retain global/frame/provider and
link-removal invalidation, new-entity admission and unresolved surface cleanup.
Steady surface work includes linked consumers and existing fields, excluding
unlinked entities that have neither a field to produce nor one to remove.
Use `SimulationPoseQuery` for body and mount poses. A parent's native motion
must invalidate a mounted field without a presentation write; readiness
withdrawal must remove the field. Retain bounded ancestor change detection
and per-pass provider-pose reuse when optimizing this path.
Profile flat and surface owners separately and require unchanged exact replay.

For `sync_local_gravity_to_avian`, compare against the existing
`ConstantLinearAcceleration` vector. Update an existing value in place and batch
new components in query order. Preserve the sample's change provenance:
consumer-motion updates bypass the external-force change notification, including
the final motion sample on a newly sleeping body; changed source fields retain
native wake notifications. External pose edits use native pose wake detection.
Every path retains the exact vector consumed by live integration and rollback.
Provider-pose reuse compares actual active-frame poses,
not prescribed-sample timestamps. Its retained samples belong to the registered
scene cache and clear at teardown. Measure the insertion `system_commands` flush
separately; removals remain driven by the existing `RemovedComponents` streams.

For `reconcile_frozen_subtrees`, profile readiness freeze and release flushes
separately from the system body. Batch repeated joint, rigid-body, collider, and
ownership-record inserts in held-root traversal order, with joint disables
queued before endpoint body disables; retain the chained joint-release boundary
after body restoration.

For `pose_to_position`, treat `PhysicsPoseSeeded` as one-time readiness
metadata. Insert it only on the first valid physics-pose write; later external
pose updates still write position/rotation and update the bridge shadow, while
sleeping bodies wake through Avian's `Sleeping` removal hook.

For `apply_pending_forces`, leave already-zero accumulators untouched so idle
bodies do not publish a `PendingForces` change every fixed tick. Clear every
nonzero command after application or while physics is held, and retain the
terminal non-finite fault path.

For `process_queued_usd_visuals`, include queue preparation in the frame-budget
review. Reuse its system-local child-key scratch set across updates, and keep
duplicate-child identity checks scoped to the parent being admitted after the
budget check; preparing keys for every queued parent can turn a small projection
slice into a full-queue UI stall.

For live composed-stage discovery of a known USD type, use the typed
`UsdReadObject::prim_paths_matching` query. It preserves live traversal
semantics while avoiding a full materialized path list followed by a separate
type lookup for every prim.

For fixed-step telemetry, keep static channel-presentation facts borrowed in
the per-sample path and allocate their owned signal metadata only when that
channel's metadata is first created or changes. Reuse one `HashMap::entry` for
each sample's cached identity lookup and mutation, and update its global-owner
association only when publication is needed. Modelica's session cursor checks
both live owner identity and the registry's catalog revision once per batch;
new variables and removed/re-admitted channels still publish through the
registry. Reuse starts only after a scalar sample has been retained; owner or
metadata preparation alone cannot prove admitted wire ownership.
Invalidate per-channel publication on a changed batch even for skipped values;
only a channel's actual association acknowledges its new owner/catalog state.
Add the shared `SignalSource`
owner marker only when the entity first retains a sample; its removal observer
owns history cleanup, so later batches do not need to queue marker writes.

Dependent USD-stage refresh is owner work behind `sync_twin_overlays`. Snapshot
the base/runtime revisions, serialize each persistent source snapshot once on
bounded workers, share its bytes across dependent stages, and coalesce changed
layers per target stage. Compare recipe bytes on workers and skip unchanged
rebuilds. The main-thread owner validates source revisions, operation/revision,
and target-plan identity before opening the live stage and committing the plan.
Measure serialization and plan preparation separately from the live-stage
build, reset owners, and visual projection; do not move the thread-affine stage
across the worker boundary or let completion order select a commit.
For a first-mounted document with a non-empty view layer, profile
`view_ops_since_source_baseline` separately from the global operation journal.
The initial recipe already contains base and runtime layers; only the bounded
view suffix needs replay, while an expired view suffix still requires a full
composed-source rebuild.

For `project_usd_policies`, a cold cache can use the worker-prepared plan as its
baseline only when every `UsdSceneChangeBatch` from generation zero to the live
generation is present and proves that no policy prim changed. Existing cached
facts use the same complete-generation and affected-path checks when promoted.
A missing batch, generation gap, plan replacement, or policy-affecting change
must use full live extraction. Compare startup and settled edit captures after
changing this path; compile evidence alone does not establish a timing gain.

For movement across terrain LOD bands, inspect
`rebind_changed_shader_look` and source reflection before blaming picking or
physics. `shader_source_validate` and `shader_source_schema` execute once per
loaded shader revision; streamed tiles and material replacements reuse their
facts. Mark the exact idle/movement windows in Tracy messages, keep the initial
avatar pose fixed, and seed a pointer in SceneView when reproducing native
picking. Distinguish shader reload events from material/look changes. Verify
invalid-stage diagnostics and hot-reload invalidation as well as frame time.
For a hidden DEM continuation with complete terrain counts, compare source
reflection and material readiness separately. Only source reflection owns
`ShaderLookSourceInterface`; material retirement must preserve it. Check queue
admission before source reflection and readiness invalidation before promotion.
Use `shader_source_lifecycle.rhai` on an admitted celestial DEM scene to verify
valid-source reload and inactive-path rejection through the production API.

Resolved port writes validate the live owner and metadata before committing.
When profiling co-simulation exchange, preserve those checks and the all-or-none
preflight for retained batches; single exclusive writes need one validation pass.
The ignored `lunco-cosim` `resolved_surface_write_benchmark` measures 6000 atomic
batches of 128 resolved surface writes with live unit, frame and range metadata.
Use the same fixture for comparisons, retain its exact final-value checks, and
qualify the production port-ownership scene afterward. Private name/duplicate
tables use Bevy's `FixedHasher`; slot order and full key equality retain their
contracts. Do not change persisted topology fingerprints to tune lookup cost.
The ignored `settled_diagnostic_publication_benchmark` measures the complete
exchange with 128 targets and fan-outs 128, four and one. Compare every case
with identical exact-value checks; counting shared sources alone does not prove
a useful speedup. Preserve wire summation order and fresh transaction samples.
For initialization stalls, inspect each wire's selected source backend and its
actual sample. A compiling Modelica participant can share an entity with ready
native pose/control ports. Initialization reads the registered owner; only the
Modelica backend withholds model outputs until its session has compiled. Missing
samples remain pending and producer errors remain terminal.
Attribute deferred compile dispatch separately from worker construction.
Pending compile batches sample physical providers once in their stable request
order and return before source copying and dependency traversal when inputs are
unavailable. Compare `system_commands` for `dispatch_modelica_compile_requests`
and `InitializationSampling` counts when diagnosing startup frame costs; retain
the production `modelica_initialization_network` and inconsistent-initial-equation
negative verdicts when changing this boundary.
The coupled coordinator waits on its transient actor's compiled/error state;
worker-wait frames do not sample providers. Result acceptance samples current
providers and revalidates the full plan before admitting participants.
Check that participant surfaces publish before connection rebuilding and that
the wiring transaction commits before initialization. Compare admitted inputs
with authored connections when a missing-input failure varies between runs.
`PendingInputConnections` records sources awaiting port-surface projection;
initialization consumes that owner fact rather than treating omitted edges as
unconnected inputs.
