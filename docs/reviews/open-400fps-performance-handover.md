# High-quality 400 FPS performance handover

> Status: Open · Worktree: optimization · Scene: Summer Space School Apollo

## Objective

Measure and reach 400 FPS in the production Apollo scene with High visual
quality enabled. The result must preserve authored visuals, physics cadence,
BigSpace frame semantics, and the same behavior in windowed, offscreen, and
headless hosts.

## Current implementation

`lunco-celestial::globe_lod::update_globe_lod` is now change-driven. Its
existing `GlobeTiles` state records the settled camera identity and position;
the run condition wakes only for unfinished residency, material readiness,
LOD/handoff/grid changes, active-camera hierarchy changes, or camera motion.
The camera reconciler's mutable resource access is not used as a change signal.
The runtime exposure publisher is also change-driven: separate invalidation
domains skip unrelated surfaces, and authored control-root membership is cached
against the existing `UsdStageRevision`. No second propagation implementation,
quality reduction, fallback, duplicate cache, or local BigSpace fork is part of
the implementation.

The shared simulation application admits BigSpace's local-origin and transform
propagation sets from spatial input changes. A changed floating origin receives
one follow-up origin computation to settle BigSpace's per-computation unchanged
flag; that pending settle is tracked between admitted passes, so stable-frame
admission does not scan every grid. Stable frames then leave propagation closed.
BigSpace remains the sole owner of transform propagation.

The shared Modelica engine adapter follows the same boundary. Its document
generation cursor no longer polls the registry and engine queues every `Update`;
document revisions, completion notifications, and tracked edit-debounce
deadlines are the only wake sources. A completion latch is cleared only while
the engine mutex confirms both completion queues are empty, so the bounded
completion budget cannot strand a queued parse or library result.

The asset catalog follows the same UI boundary. The shared discovery owner now
enumerates USD, WGSL, Modelica, and Python extensions in one asynchronous task;
the spawn, shader, and program projections publish only after that task drains.
The startup and Twin lifecycle paths no longer synchronously walk the open Twin
roots from the UI schedule, and listing generations prevent a closed/reopened
Twin from publishing stale results.

This pass also reduces repeated reactive work without changing cadence or
validation: the HUD invalidation sources now share one combined `Or` query; the
Modelica projector checks prim and identity additions together once inside its
system instead of scanning them in both a run condition and the system; and
BigSpace's high-, local-, and low-precision admission predicates each combine
their existing spatial invalidation filters into one query per schedule
boundary. The USD telemetry projector performs one bootstrap pass, then uses
component insert/remove observers plus scalar stage-revision and asset-change
signals instead of repeated `Added`/`Changed` population queries in both its
run condition and invalidation system. BigSpace still owns all propagation
work and the origin-settle pass is unchanged. Physics-validation and telemetry
scratch buffers also retain capacity across calls. Domain member-class
discovery now consumes a typed `UsdSceneChangeBatch` and routes its
resynced/info paths through a canonical stage/root/member reverse index.
Stage-asset changes and missed generation batches requeue only roots on the
affected stage; the all-prim discovery is limited to initial admission.
Endpoint wiring invalidation keeps using the narrower queued-entity path.
Generated Modelica document sync also
uses lifecycle-queued wrappers and a dirty publisher latch rather than per-frame
`Changed` filters. Domain synthesis uses direct authored-property lookup for
member communication periods and enumerates root attributes once for both
actuator input and output ports. USD connection reconciliation also borrows
endpoint paths, generated aliases, and port surfaces in its temporary indexes
instead of cloning them; it builds those indexes in one endpoint sweep instead
of three separate population scans. Earth-direction demand now reconciles its
existing and desired marker sets, avoiding remove/re-add events on unrelated
rewires. Endpoint removals now dirty USD wiring only when the removed
capability belonged to an authored USD prim; unrelated `SimComponent` teardown
no longer triggers a whole wiring pass. These are source-level work reductions
only. Program binding extracts its declared input/output maps once and reuses
them for the admission verdict and published interface; the communication
period is read through a direct authored-property query. Orphan acausal
admission uses a non-allocating prefix probe to skip connectorless programs,
then derives declaration and connection presence from one attribute-name pass
when connectors exist. The authoring-review view model now compares the
selection, controlled target, and displayed camera target/mode it actually
consumes. This avoids reopening the full view projection when camera
reconciliation merely mutably borrows `SceneViewport` or changes camera pose
without changing the followed target; in-place target changes still wake the
view. The regression covers both paths. In the pre-change 60.35 s Tracy
capture, the producer ran 1,756 times (1,713 after the first 10 s); in the
post-change 55.33 s capture it ran 48 times (11 after the first 10 s), reducing
the observed call rate from 29.1/s to 0.87/s. This is profiler attribution,
not clean FPS acceptance; a settled unprofiled Apollo run remains necessary
before claiming an overall FPS improvement. The before/after captures are
`summer-space-school-optimization-ccbfe504-settled-20260924.tracy` and
`summer-space-school-optimization-ba7550-authoring-view-gate-20260924.tracy`
in the ignored `scripts/perf/captures/` directory.
The shader-look binder also reuses loaded WGSL stage verdicts by asset ID and
stage, invalidating them on shader asset events so shared terrain looks do not
revalidate identical source; see the dated note in the [200 FPS
handover](open-200fps-performance-handover.md). No measured FPS delta is
claimed.

## CPU investigation lead

Two owned Apollo startup captures separated domain synthesis from publication.
In `summer-space-school-domain-zones-20260923-4191.tracy`, the ten
`domain_projection_commit` spans accounted for 4.298 s total (430 ms mean,
1.383 s max). The follow-up capture,
`summer-space-school-domain-zones-20260923-4191-after-index.tracy`, recorded
the same ten commits at 32.1 ms total (3.21 ms mean, 16.3 ms max). The measured
commit interval fell by about 99.3% in these startup captures.

The source-level cause was repeated composed-stage work inside signal-layout
publication: each telemetry ownership check enumerated every prim, and each
unit cloned the already-expanded exact-path/provenance maps from earlier units.
The projector now builds one telemetry ownership index per canonical stage
generation or prepared-plan identity and shares it across roots in a projection
batch. Unit expansion reads immutable source-map snapshots, so generated keys
do not recursively feed later units. The new Tracy zones measured two telemetry
index builds at 30.1 ms total, five signal-layout builds at 0.77 ms, and five
unit expansions at 0.60 ms. The 27 domain tests, including external telemetry
ownership and multi-unit mapping regressions, pass.

This is startup/reactive-path evidence, not a steady-frame or FPS result. The
post-change 45.28 s capture had 941 frames and 5.72 million zones, but the
owned API check still reported `ready=false` with USD physics admission pending.
It must not be used as a clean settled acceptance window. The live canonical
USD `Stage` remains `!Send`; if a future profile establishes that synthesis
itself affects a user-visible budget, any worker boundary must pass an owned,
send-safe snapshot and fence publication by root and stage generation.

## Verification

- `cargo clean` from the optimization checkout removed 16.2 GiB after the
  checkout reached 98% disk usage; all evidence below is post-clean.
- `cargo build -j 4 -p lunco-luncosim --bin luncosim --features ui`: passed.
- `scripts/run_rust_tests.sh -j 4 -p lunco-celestial --lib`: 128 passed.
- Production run used `target/debug/luncosim`, X11, High quality,
  `--no-vsync --no-throttle --log-diag`, API 4366, and the authored
  `traverse_apollo15.usda` scene. `/api/ready` returned
  `ready=true`, `world_hold=false`, `faulted=false`, `pending_count=0`.
- Current optimization build: focused `cargo check` and exposure/core tests
  pass. Apollo reached `/api/ready` in 4.1 s from process start; its settled
  diagnostic tail was 92–147 FPS with 7.3–10.9 ms frame samples. The sandbox
  reached readiness in 2.8 s. These runs remain non-acceptance evidence.
- On the post-change production vehicle run (binary rebuilt after syncing
  `d7ad4ad0f`), the shared listing was scheduled at `01:56:35.675886Z`, the
  scene was spawned at `01:56:36.732963Z`, and scene participants were ready at
  `01:56:41.677245Z`. This is approximately 8.88 s from process start and is
  evidence that catalog enumeration no longer blocks the scene schedule, not a
  claim that the 2 s loading target is met.
- The final rebuilt binary also completed a headless vehicle smoke: the shared
  listing was scheduled at `02:43:32.324864Z`, the scene spawned at
  `02:43:32.547703Z`, and participants were ready at `02:43:36.320508Z`.
  The run was terminated by its 10 s verification timeout after readiness; the
  port was free afterward. This confirms the new catalog path on the rebuilt
  binary, but the approximately 5.01 s process-to-readiness interval remains
  above the 2 s target.
- Settled diagnostics reported 4100 entities and approximately 33–79 FPS with
  approximately 12.6–30.3 ms samples in the captured window. The API `Exit`
  command was accepted and port 4366 was verified closed.
- `nice -n 19 cargo test -j 4 -p lunco-luncosim-edit-ui --lib
  authoring_review_view_gate_sleeps_until_an_owner_or_identity_changes` passed
  (1 test). This verifies the scheduling gate, not an FPS gain.
- `nice -n 19 cargo test -j 4 -p lunco-usd-sim-domain --lib` passed all 27
  tests after the telemetry-index and signal-map changes.
- `nice -n 19 cargo build -j 4 -p lunco-luncosim --bin luncosim --features
  tracy` passed. The post-change capture's typed API `Exit` was accepted and
  port 4191 was verified closed; the existing 4163, 45552, and 37431 sessions
  were left untouched.

### 2026-09-24 — conservative spotlight shadow relevance

`lunco-render-bevy` now disables only the extracted shadow-map flag for a
spotlight whose conservative finite-frustum bound misses every extracted 3D
camera on compatible render layers. Missing or malformed bounds and boundary
contacts retain the shadow map. Authored lights, direct illumination, shadow
resolution, and the High quality preset are unchanged. This removes irrelevant
shadow-view preparation after extraction; Bevy's main-world per-light caster
visibility work remains in place.

- Six focused `spotlight_shadow_relevance` unit tests passed. The regular and
  Tracy-enabled production binaries both built, and the High-quality Apollo
  scene reached `/api/ready` with no pending work. Both owned sessions shut down
  through typed API `Exit`; their ports were released.
- The unprofiled diagnostic tail varied from 45–132 FPS and 7.6–22.1 ms per
  frame while other simulator sessions were active, so it is contention-affected
  and is not a clean FPS comparison.
- Tracy capture:
  `scripts/perf/captures/apollo-high-spotlight-20260924.tracy` (20.29 s, 518
  profiler frames, 3.82 million zones, 139.12 MB). The full-trace CSV exporter
  was stopped after several minutes at one low-priority core; no zone
  attribution or performance delta is claimed from this capture.
- The Tracy-run Bevy diagnostics still provide pass-level context (not a clean
  acceptance result): its final samples reported 10.4–15.8 ms frame times,
  while `main_opaque_pass_3d` was 1.66–1.69 ms GPU and 0.06–0.08 ms CPU. Its
  pipeline counters were stable at 509,746 clipper invocations and 411,695
  output primitives. Thus the opaque pass alone cannot explain the observed
  frame time; CPU work outside the instrumented render passes, waits, or other
  passes remain to be attributed. These measurements were collected under
  contention and must not be treated as a product FPS baseline.
- Source inspection of Bevy 0.19.1 found that
  `check_point_light_mesh_visibility` scans eligible mesh casters for each
  shadow-enabled point/spot light and rebuilds sorted per-light caster lists
  before extraction. The spotlight relevance adapter above runs after
  extraction, so it cannot currently skip that main-world work. This is a
  profiling lead, not an established hotspot: inspect this system and
  `prepare_lights` in the saved capture before changing lifecycle/order or
  shadow behavior.

This change preserves authored visual quality by construction, but a settled,
uncontended before/after FPS comparison and detailed Tracy zone inspection are
still required; the 400 FPS acceptance target remains open. Other simulator
sessions and an unrelated Cargo build were left untouched.

### 2026-09-24 — Summer Space School settled-frame profile

- Captured the production debug build from this checkout with Tracy enabled,
  High quality, `--no-vsync`, and `--no-throttle`, using the real
  `traverse_apollo15.usda` scene and API port 43442. The 40.33 s capture is
  `scripts/perf/captures/sss-optimization-20260924.tracy` (1,438 Tracy frames,
  11,219,291 zones, 285 MB). API readiness drained to zero pending items; the
  owned run exited through typed API `Exit`, and port 43442 was released.
- Session 43122 remained active throughout and was not inspected or changed.
  Consequently neither this trace nor the following FPS window is an
  uncontended acceptance result.
- Tracy self-time aggregates for recurring systems showed approximately
  0.63 ms/call for `drain_world_scripts` (1,437 calls), 0.58 ms for BigSpace
  `propagate_high_precision_channeled` (1,382), and 0.56 ms for Bevy's
  `check_point_light_mesh_visibility` (1,437). `check_dir_light_mesh_visibility`
  plus its command application accounted for another 0.82 ms/call combined;
  GPU preprocessing bind groups were 0.50 ms and GPU clustering preparation
  0.38 ms per call. These distributed CPU costs, not one dominant opaque pass,
  are the main measured render/update leads. The Tracy-run opaque GPU pass
  averaged about 2.02 ms; this is diagnostic only.
- `drain_world_scripts` is an exclusive Update system called every frame; its
  source drains `PendingWorldScripts` and returns immediately when the queue is
  empty. Its measured recurring cost makes avoiding the idle exclusive-system
  pass a high-priority A/B candidate. Bevy `Core3d` ran 9,547 times for 1,437
  root frames; the camera driver also runs auxiliary shadow-map views, so this
  count does not mean there are 6.6 output cameras.
- A separate, non-Tracy production run with the same scene and quality gathered
  16 post-readiness one-second diagnostic samples: mean 104.7 FPS (58.4–151.9),
  frame time 10.46 ms (6.78–17.13), and Avian step time 0.89 ms (0.73–1.08).
  The existing session still running makes these contention-affected, not a
  clean acceptance measurement. The 150+ FPS target remains unverified.

### 2026-09-25 — bounded worker pools and CPU render-path attribution

- Bevy's default task-pool budget was implicitly sized from 32 logical CPUs on
  this 16-core machine. The native API and asset listeners also each created a
  32-worker Tokio runtime despite already owning dedicated OS threads. Bevy is
  now capped at physical-core count; native transports use current-thread
  runtimes. The app process fell from 97 threads to 42. This reduces duplicate
  schedulers without changing render quality or simulation cadence.
- A fresh High-quality Apollo run used the production binary, the authored
  `traverse_apollo15.usda`, and API port 43599. Readiness drained to zero. Over
  a settled 29-second window, `frame_count` advanced by 3,391: **116.9 FPS**;
  the rolling frame-time diagnostic stayed around **8.4–8.9 ms**. This is
  better than the prior 10.46 ms sample, but remains below 150 FPS. A single
  host load-average snapshot was 3.2 and there was no other `luncosim` process.
  GPU SM utilization
  sampled 66–75%, so neither an uncontended GPU ceiling nor a CPU-only limit is
  established from utilization alone. The owned run exited through typed API
  `Exit`, and port 43599 was released.
- The separate Tracy diagnostic capture is
  `scripts/perf/captures/sss-physical-pool-single-api-tracy-20260925.tracy`
  (45.37 s, 2,282 profiler frames, 13.41 million zones, 294.23 MB). Its traced
  frame rate was only about 50/s, so none of its frame-rate numbers are product
  measurements. The exported GPU timestamp categories account for about
  2.5 ms/frame: 1.66 ms opaque, 0.26 ms bloom, 0.26 ms MSAA writeback, 0.17 ms
  clustering, and 0.18 ms tonemapping, upscaling, and other small passes. This
  is not the full GPU frame time: Bevy's shadow render passes have CPU Tracy
  spans but no corresponding GPU timestamp zones in this capture. Do not
  subtract 2.5 ms from the clean frame to infer a CPU-only remainder.
- CPU self-time attribution is distributed: Bevy `Render` averaged 3.67 ms
  (95th percentile 4.52 ms), `PostUpdate` 3.12 ms (p95 4.24 ms), and `Update`
  1.65 ms (p95 3.54 ms). These schedule spans overlap worker activity and must
  not be added as a serial frame-time budget. Recurrent leaf costs include
  `prepare_preprocess_bind_groups` at 0.51 ms/frame; directional shadow-caster
  visibility at 0.13 ms in the system plus 0.44 ms applying its deferred
  visibility commands; point-light visibility at 0.19 ms/frame; and workbench
  rendering at 0.25 ms/frame.
- The strongest single render-thread lead is Bevy's `queue_submit` zone:
  about 1.28 ms/call, with the trace recording 48 pending command buffers on
  each of roughly 2,208 submissions. Source confirms this is the call to
  `wgpu::Queue::submit`; the trace does not yet distinguish driver backpressure
  from CPU submission overhead, so this is a measured target, not a proven
  cause or a fix. Pass markers show six spotlight shadow maps plus four
  directional-light cascades each profiler frame, alongside one main opaque
  camera pass. `Core3d` ran 6.78 times per profiler frame; that count is neither
  the number of active output cameras nor the total number of shadow maps. The
  existing camera-scoped GPU-culling path is already active and intentionally
  leaves mesh/light shadow visibility on CPU.
- CPU shadow work is material: directional caster visibility used 0.13 ms in
  the system plus 0.44 ms applying deferred visibility updates, and point/spot
  caster visibility used 0.19 ms per frame. These paths traverse shadow
  casters for light views; their cost is distinct from recording the shadow
  draw passes and from GPU execution.
- The authored Apollo scene has one rover without headlight references, but the
  Twin opts into `usd.runtime_persistence = true`. Startup restored
  `.lunco/runtime/sim/scenes/traverse_apollo15.usda`, whose runtime layer adds
  two Rocker-Bogie rovers, one Ackermann rover, a probe, and two balls. A live
  `QueryUsdPrim` confirmed that all six headlights on those extra rovers are
  shadow-enabled `SphereLight`s (120,000 lumens, 90 m range). These are exactly
  the six spotlight shadow passes in Tracy. The live API reported 788 registered
  entities, and the current non-Tracy diagnostic run showed a rolling
  10.35 ms/frame (~104 FPS); this was inventory evidence, not a separate clean
  acceptance window. The overlay is persisted Twin state, not a loader leak; I
  left the sidecar unchanged.
- `SceneCameraAudit` found five camera candidates: the Avatar window camera is
  the only active camera; the three spawned-rover cameras and the 16×16 USD
  preview camera are inactive. The main opaque pass also ran once per Tracy
  frame. Rendering several full output cameras is therefore not the root cause.
- Preserve the persisted overlay and all six headlight shadows while testing
  camera-only stage admission on auxiliary roots. Measure the 48-buffer submit
  boundary and shadow-pass cost before considering any further render-path
  change; the existing GPU timestamp table does not cover shadow passes.
  Do not infer the full GPU/CPU split from that incomplete table. The 150 FPS
  target remains open.

### 2026-09-25 — auxiliary shadow-view stage admission

Bevy's camera driver runs `Core3d` for point/spot shadow roots as well as output
cameras. `lunco-render-bevy` now gates `Prepass`, `MainPass`, `EarlyPostProcess`,
and `PostProcess` stage sets off for `LightEntity` roots. Source inspection of
Bevy 0.19.1 confirms its early/late shared and per-view shadow passes are
separate systems outside those stage sets, so the depth-map path remains active.
The shared `prepare_preprocess_bind_groups` pass remains active too: it builds
view-specific bindings for each phase's work-item buffers and is needed by GPU
preprocessing/culling for shadow draws. The measured pre-change cost was about
0.514 ms/frame; this app-level gate does not bypass or cache it.

The same pre-change Tracy capture recorded one `wgpu::Queue::submit` call per
frame, with 48 pending command buffers, at 1.277 ms mean (about 1.936 ms p95).
The new gate may prevent camera-only systems on shadow roots from contributing
buffers, but it cannot remove the shared queue call. Post-change capture
`scripts/perf/captures/sss-shadow-schedule-after-20260925.tracy` contains 45.31 s,
861 profiler frames, and 5.32 million zones. The bundled CSV exporter did not
finish its first filter after 15 minutes and was stopped; the capture is
preserved, but no post-change submission count/time or schedule delta is
claimed.

The production UI binary built with the change, and the focused
`camera_stages_are_skipped_only_for_light_shadow_roots` test passed. A headful
High-quality Apollo run reached `/api/ready` with no pending work, and its
2560×1568 API screenshot showed the scene rendering. Under the two pre-existing
simulator sessions on ports 43117 and 43126, its diagnostics averaged about
89.9 FPS / 12.37 ms per frame and 1.05 ms per Avian step. Those sessions were
left untouched; this is contention-affected and not an acceptance comparison.
The separate unprofiled run and Tracy run both exited through typed API `Exit`,
and owned ports 43602, 43603, and 8091 were verified free. Clean post-change FPS,
buffer-count reduction, shadow pass inventory from the new capture, and the 150
FPS target remain unverified.

A later owned non-Tracy Apollo diagnostic run on API port 43604 reached
`/api/ready` with zero pending work and exited through typed `Exit`. It ran
while the existing sessions on 43117 and 43126 were still active; immediately
before launch, host load averages were 10.36/11.87/8.84 and NVIDIA GPU
utilization was 78%. Its rolling diagnostics reported 63.2 FPS / 18.67 ms,
with individual samples as low as 32.9 FPS / 30.40 ms; Avian averaged 1.13 ms
per step. This is severe contention evidence only, not a post-change acceptance
comparison or a clean estimate of the gate's effect. Port 43604 was released;
the other sessions were untouched.

### 2026-09-25 — Bevy render-queue submission boundary

Source inspection of Bevy 0.19.1 separates two queue submissions that are easy
to conflate. The CorePipeline render-graph `Submit` stage drains all
`PendingCommandBuffers` in one `wgpu::Queue::submit`; the pre-change Tracy zone
measured that call with 48 buffers. Bevy's outer `render_system` then always
creates a second encoder, asks the screenshot and GPU-readback subsystems to
append any pending copies, finishes it, and calls `Queue::submit` again. When
neither subsystem has pending work, those helpers append no commands, but the
empty encoder is still submitted. That second call is outside the measured
`queue_submit` span, so its cost is currently unknown.

The local shadow-root stage gate reduces camera-only Core3d work before the
batched graph submit. Safely removing the unconditional screenshot/readback
submission is a Bevy render-system ownership change: replacing Bevy's wrapper
from the application would also take ownership of frame presentation,
screenshot capture, and GPU readback ordering. Do not fork or duplicate that
wrapper in LunCoSim. A focused upstream change should submit the secondary
encoder only when screenshot/readback work was actually recorded, with
production coverage for idle frames and both request paths. First measure the
second call separately and keep the current post-change FPS and buffer-count
comparison open; source inspection alone does not establish a frame-time win.

### 2026-09-25 — directional caster-visibility scratch lifetime

The pre-change trace attributed 0.13 ms/frame to Bevy's
`check_dir_light_mesh_visibility` and another 0.44 ms/frame to applying its
deferred `ViewVisibility` updates. In Bevy 0.19.1, the visibility system
collects entity IDs in a `Local<Parallel<Vec<Entity>>>`, then `mem::take`s that
scratch into a `Commands` closure. The closure marks those entities visible
after the parallel system completes; the moved per-thread vectors are dropped
after application, so their capacity is not available to the next frame. This
matches the owner source's TODO to avoid the per-frame allocation.

The safe optimization belongs in Bevy's light-visibility owner: retain the
per-thread entity buffers in reusable storage through deferred application,
while preserving the current parallel caster tests and the ordering before
`MarkNewlyHiddenEntitiesInvisible`. Do not skip shadow visibility or cache
results in LunCoSim without a complete invalidation contract. The measured
0.44 ms is a target, not a promised saving; compare exact visible-caster sets
and frame time after an upstream implementation.

### 2026-09-25 — GPU-cluster preparation allocation lifetime

The September 25 production logs confirm that this Vulkan device selects
Bevy's GPU-clustering path. In Bevy 0.19.1,
`prepare_clusters_for_gpu_clustering` constructs fresh per-view
`ViewClusterBindings` and `ViewGpuClusteringBuffers` during render preparation,
reserves their working storage, and queues both components onto the view.
`upload_view_gpu_clustering_buffers` then writes those buffers, and
`prepare_clustering_bind_groups` creates four bind groups for each clustered
view on every render frame. The cluster contents do change with lights and
view state, but buffer capacity and bindings need not be discarded merely to
refresh that content; the owner can retain storage and rebuild bindings only
when their underlying buffer handles or layouts change.

The older Tracy table's `prepare_clusters` zone averaged 0.975 ms with one
approximately 39 ms outlier, but the zone has not yet been mapped conclusively
to this exact Bevy system. Treat this as a high-value source-level lead, not an
attributed saving. No `Resizing the view clustering ...` warning appears in
the checked September 25 Apollo logs, so increasing initial capacities alone
does not address the observed steady-state allocation path. The correct fix is
in Bevy's GPU-clustering owner; do not replace clustering with the CPU path or
alter cluster quality from LunCoSim. A new focused trace must identify the
exact system, and validation must compare lighting/cluster output as well as
allocation count and frame-time percentiles.

The current Bevy `main` source was checked on September 25 and still registers
these preparation systems every render frame and creates fresh per-view buffers
in `prepare_clusters_for_gpu_clustering`. Merely moving to a newer Bevy release
is therefore not yet an evidence-backed fix; the upstream owner needs a focused
reuse change and before/after measurements.

### 2026-09-25 — UI/physics CPU-tail architecture audit

Source review identifies real main-thread blockers, but does not yet attribute
the measured steady-state frame tail to one of them. The production 60 Hz
`Time<Fixed>` keeps integration delta constant; Bevy drains accumulated fixed
steps synchronously before `Update`. At the highest transport rate the current
raw-delta budget permits a burst of up to 64 ticks before UI/input runs. The
raw-delta cap bounds catch-up work by excluding excess wall time, so this is
neither a wall-clock 60-tick/s guarantee nor tick loss inside an admitted burst.
A one-tick-per-Update cap would protect the main loop only by retaining lag or
discarding causal time; it cannot by itself satisfy both constant-rate physics
and UI responsiveness.

Other verified tail-risk paths are:

- `drain_world_scripts` is an exclusive `Update` system that takes and evaluates
  the entire queued REPL batch against the live `World`. Rhai allows up to one
  million operations per invocation. The earlier Tracy capture attributed
  about 0.63 ms/call to this system, but did not publish p99/max time or separate
  the empty-queue call from actual evaluations.
- Scenario hooks run serially in the fixed simulation path and may invoke
  substantial live-world Rhai work. Their per-tick maximum and p99 costs are
  not currently available beside the Avian solver timing.
- Terrain visualization's lockstep capture mode waits on every pending bake in
  `Update`; normal mode polls without waiting but applies every completed mesh
  in the same pass. Launch count is bounded, completion application is not.
- Shared async admission allows four running jobs and prioritizes only queued
  jobs. It cannot preempt a running CPU task, and visualization owners still
  have direct submissions to Bevy's pools. Pool contention remains a source
  hypothesis, not a measured explanation of the clean 116.9 FPS result.

This rules out schedule labels or a lower catch-up cap as complete solutions.
The required architecture is one paced owner for the full causal simulation
world (Modelica, Rhai, event/coupling barriers, Avian, and authoritative state),
with the window/UI process exchanging typed tick-stamped commands and
nonblocking immutable snapshots. A monotonic real-time driver keeps fixed `dt`
and reports missed deadlines/backlog; unpaced tests and offline recording drive
those same ticks on demand. Renderer interpolation and visualization work
consume snapshots without waiting for that owner. Splitting Avian alone would
leave live-world causal reads/writes racing across threads.

This is a design finding, not an implemented worker split. Before claiming the
tail is fixed, add per-cycle p50/p95/p99/max and tick deadline/backlog evidence,
then separately remove unbounded one-shot evaluation batches, make capture
readiness asynchronous, and budget result application. A same-world
per-application-frame step limit is an interim overload policy only; it must
retain ticks and expose lag, and cannot guarantee UI progress through one
overlong synchronous tick. No FPS acceptance run was started for this audit;
the existing simulator sessions were left untouched.

The idle one-shot path now has a `PendingWorldScripts` run condition, so an
empty queue skips the exclusive drain system's execution. Its focused
`empty_repl_queue_does_not_schedule_its_exclusive_drain` test passed. This does
not bound active Rhai evaluation or queue depth and has no measured FPS delta;
it is a narrow idle-path reduction, not the UI-isolation fix. No new simulator
or FPS profile was started for this change.

## Remaining blocker

The 400 FPS acceptance target is not met. The maintained BigSpace dependency
is pinned to the reviewed `bevy-0.19` revision available here
(`5f255228e9b4…`). Application admission skips stable propagation frames, but
an active high-precision pass still uses BigSpace's worker/channel fan-out; the
available Tracy attribution predates the application gate, so its current cost
is not established. The remaining render/update budget also needs a clean,
settled measurement against the 2.5 ms frame budget. Further per-entity
fan-out reduction belongs in the maintained BigSpace owner, not an application
duplicate or degraded path.

## Acceptance

Close only after a real production High-quality Apollo run sustains at least
400 FPS with p95 frame time at or below 2.5 ms, while the required physics,
visual, headless/windowed, and API shutdown checks remain green. Until then,
keep this handover open and record the exact upstream/render evidence here.

See [`open-200fps-performance-handover.md`](open-200fps-performance-handover.md)
for the shared profiling commands and BigSpace architectural constraints.
