# Landed Griffin performance qualification

> Status: Open · 100 FPS and frame stability are not achieved.

The production Griffin surface-operations scene in
`/home/rod/Documents/models/lunar-base-model/twins/astrobotic-griffin-1`
was measured after its authored `landing_handoff`, at High quality,
2560×1440, Standard rendering, without vsync or unfocused throttling.
Terrain, shaders, shadow settings and physics cadence were not reduced.
Existing worktree quality-profile changes were preserved.

## Retained changes and ownership

`lunco-scripting-bridge-spatial` shares a World-owned cached Bevy system over
the physics owner's `SimulationPoseQuery` for position, rotation, forward and
navigation reads. It caches access metadata, never pose values. Three focused
tests cover active-frame orientation, unavailable frames, live values,
archetype changes and entity retirement. This is an engine query mechanism,
not a policy decision or hook candidate.

The Twin's `tools/griffin_tether_visuals.rhai` indexes each immutable scalar
geometry snapshot once. Guide-vector reads use that index instead of repeatedly
scanning the scalar list and constructing axis strings. The last authored
scalar for a repeated name remains authoritative, and missing/non-finite vector
components still fail. Curve points, widths and publication cadence are unchanged.

`scripts/perf/twin_startup.py` supports `--measurement-seconds`, reports p99 and
frames over budget, and accepts `--max-frame-ms` for a hard worst-frame gate.
The existing readiness, exact camera target, terrain, source, complete-history
and advancing-physics checks remain mandatory. These are measurement controls;
they do not change runtime policy.

The driver explicitly activates and verifies the requested workbench perspective
(`sandbox_view` by default). Its active-camera and physical-target checks still
apply. A persisted Systems perspective must not silently substitute a workload
without the intended 3D view.

## Observed evidence

Receipts and captures are in ignored `.cache/perf/lunar-100fps/`.

The initial existing development executable measured 43.166 FPS over 442 fresh
frames: p95 32.044 ms, maximum 37.497 ms. It was stamped `3fbb85e5-dirty`,
whereas this checkout is `9ba26ca`; it is not a matched source-change comparator.

The earlier completed bounded release window measured
**51.657 FPS**, median **18.650 ms**, p95 **25.213 ms**, p99 **29.050 ms**,
and maximum **33.964 ms** over **541 frames**. All 541 frames exceeded the
10 ms budget. The driver correctly exited nonzero. This does not establish
a source-change speedup or stable 100 FPS.

The attempted 60-second release window failed when the terrain owner reported
`selection_pending=1`, with wanted/resident both 181. Readiness remained clear.
The long stability gate is not qualified; its incomplete samples must not be
presented as a successful sustained benchmark.

The separate diagnostic used `../tracy/capture/build/tracy-capture-0.14.1`
and the matching native CSV exporter, with 1 kHz CPU sampling. Owned PID 2386403
used API 5410 and Tracy 8342. The capture contains 8,444,839 zones across
68.1 seconds and exports successfully. Its exact development ELF is retained
as `luncosim-tracy-baseline`, SHA256
`06ea10e4b50d7e43d2c657e38a7f7a3507e5f8bb03a35fb31e90522f957fdebb`.
The 54–66 second GPU event window reports mean opaque-pass time 2.193 ms,
directional-shadow parent 1.837 ms and early prepass 0.781 ms. Nested events
must not be added to their parents; these recorded passes are not a complete
GPU/frame total. The compact-egress task and application schedule coordination
remain CPU investigation targets. Precise static sample attribution needs
the exact native load map and matching split debug objects; inferred load
biases are not sufficient evidence for another optimization.

All owned simulator and collector processes exited. API ports 5410 and 5411,
and Tracy port 8342, were released.

## Checks

- `cargo test -p lunco-scripting-bridge-spatial --lib script_pose_reads -j 4`:
  two tests passed.
- The same built test binary's
  `tests::pose_query_cache_tracks_live_archetypes_values_and_retirement`: passed.
- `cargo build -p lunco-luncosim --features tracy -j 4`: passed.
- `cargo build -p lunco-luncosim -j 4`: passed.
- `cargo build --release -p lunco-luncosim -j 4`: passed.
- Skill catalogue validation and Python compilation: passed.

## Subsequent attribution and measurement checks

`tracy-release-view/capture.tracy` is a fresh optimized Tracy capture with a
retained symbol-bearing ELF, native load maps and JIT maps. The matching native
exporter reads 8,234,831 zones over 62.9 seconds. Owned PID 2708197 used API 5410
and Tracy 8343. Source hashes match before and after. The camera audit confirms
one active 2560×1440 Griffin tracking camera in View. Its attempted acceptance
window stopped on terrain `selection_pending=1`; this is diagnostic evidence.

In its 54–60 second interval, mean main update is 25.672 ms, the native render
handoff is 6.787 ms, the fixed loop is 8.526 ms, and PostUpdate is 4.502 ms.
These inclusive intervals overlap other threads and contain waits. Native
self-time export reports 3.075 ms of PostUpdate schedule time outside its
main-thread child zones; that alone does not distinguish executor overhead
from waiting for worker systems. The exact PostUpdate sample selection contains
348 samples across 233 zones at a verified 1 ms sampling period. Many optimized
stacks contain only a generic query leaf, so they do not establish its caller.
Do not attribute merged query symbols to a particular application system.

Recorded mean GPU opaque time is 2.148 ms, directional-shadow parent 1.716 ms,
and prepass 0.870 ms. These are separate recorded passes, not a complete GPU
frame total. Nested GPU zones must not be summed with their parents.

A same-session observer diagnostic (`probe-comparison/receipt.json`) brackets
frame-only polling with the benchmark's full inspection polling. The three
10-second windows measure 39.755, 45.666 and 43.048 FPS respectively; worst
frames are 48.014, 35.700 and 40.424 ms. This shows observer perturbation, not
a runtime optimization or acceptance. Sources are unchanged, but other simulator
processes run concurrently, so the difference is not an isolated speedup.


Attribute the remaining application/physics and render preparation costs with
caller-complete evidence, then qualify an unchanged source cohort. Preserve
numerical cadence and image quality.
The outstanding acceptance requirement is a complete 60-second landed window
at 100 FPS with every frame at or below 10 ms.

## Immutable read-path optimization

The physics gate initializes World-owned metadata for immutable API pose reads.
`SimulationPoseReadState` borrows that World and its cached
`SystemState<SimulationPoseQuery>` through a non-blocking lease. Every call reads
live components; unavailable metadata, frames, native seeds and prescribed
samples retain their rejection boundaries. Scene poses and identities are not
cached. The immutable borrow prevents mutation during a lease. These are native
engine read mechanisms, not lifecycle policy or hook candidates.

`ApiEntityRegistry::entities_unordered()` borrows the registry for name, path
and proximity reductions. Observable batches still collect and sort by identity;
first-match reductions retain their lowest-id rule. `QueryEntity` reads one
entity's optional metadata directly, without constructing an all-entity query.

The regular release artifact is retained as `luncosim-query-metadata`, SHA256
`6cd88b28457e82c5b3c80dff8132a1f5875b7e2d319ed05c874ae7ad74e1d284`.
The completed build and exact runtime patch are recorded in
`query-metadata-build.json`. It passes all eight focused physics-pose tests,
the five registry lifecycle tests, and the production `transform_entity` scene's
six authored checks (60 simulation ticks, exit zero).

The `query-native-before`, `query-native-after` and
`query-native-before-repeat` receipts retain equal before/after Twin hashes in
each run. Five repetitions of 100 `QueryEntity` reads have median in-command
elapsed times of 21.255, 1.268 and 31.546 ms respectively. This diagnostic uses
Rhai's existing monotonic timestamp only in a command observation and excludes
HTTP/frame-boundary latency. Corresponding `QueryUsdPrim` medians are 37.261,
23.627 and 65.163 ms. The repeated baseline exposes changing machine load;
these query timings do not establish a frame-rate gain. Frame-only diagnostic
windows report 49.753, 29.792 and 17.298 FPS, with other simulator sessions
changing during the sequence. None qualifies the 100 FPS target.

The native sample reader now preserves each sample's timestamp and per-thread
index. Re-exporting the existing View capture retains the same 348 PostUpdate
samples and 233 zones. Joining timestamps to same-thread inclusive CPU intervals
places all 83 samples bearing the ambiguous generic query leaf inside
`bevy_light::check_dir_light_mesh_visibility` command application. This identifies
their instrumented execution context; it does not reconstruct missing caller
frames or make that bounded cost the dominant frame bottleneck. Native captures,
the timestamp join and the diagnostic query drivers remain ignored evidence.


## Native render/simulation overlap

The native host receives the previous render frame after ordinary fixed work
and before physical presentation sampling. Its automatic clock is sampled
independently of Bevy's optional render timestamp receiver; manual clock
strategies are unchanged. Ready completions still use the deterministic fixed
runner, never extend a ready render receipt, and never add wall time. The
runtime owner and physics inputs are unchanged; this scheduling mechanism has
no changeable routing policy or hook candidate.

The matched integrated control and candidate are retained as
`luncosim-handoff-control` and `luncosim-handoff-late`. The candidate passes the
native presentation gate (13 authored checks) and the background transport gate
(8 authored checks; measured rates 2.003, 4.008 and 8.008).

The `handoff-late-debt` diagnostic explains its first playing window's 75.592
ticks per wall second: pending simulation duration decreases from 6.593 to
0.995 seconds. Pausing retains about 1.017 seconds, and resuming drains the
remaining debt. No max-delta clipping is reported. These catch-up windows do not
qualify ordinary transport throughput or the 100 FPS target.

The `handoff-control-landing` and `handoff-late-landing` runs use the Twin's
existing production landing observer in the native host, with one compute
thread in each run. Their source snapshots match. All physical metrics match
exactly; only virtual elapsed wall-paced time differs. Both latch touchdown at
tick 1350, complete 3600 post-touchdown ticks, retain all four contacts across
361 samples, and report no missing or non-finite sample. Both **fail** the
existing SysML stability requirement: peak ground speed is 0.069062 m/s against
0.05 m/s. This is unchanged baseline behavior, not a passing landing-stability
gate. No authored limit, timestep, substep, solver or visual setting is changed.


## Typed script-result and contact-query allocation

Tracy's tether-task samples include the body-state query's all-entity walk,
result-tree allocation and mount-authority text projection. The scripting bridge
now retains owned typed containers during selector filtering and authority
sanitization, including reflected compound reads. The asset owner retains its
registry error boundary and skips replacement-set construction when text cannot
contain a hexadecimal mount authority. No public value, selector or authority
rule changes.

`QueryPhysicsState` retains its wheel-contact `SystemState` in the provider
registration instead of scanning every scene entity. A non-blocking lease checks
the exact World and reads live contact ownership and values, including
disabled entities. USD-path ordering and target rejection remain unchanged.
Only access metadata is retained, so scene teardown and new archetypes need no
parallel entity index. This is a query mechanism consumed by the existing Rhai
and HTTP interfaces; there is no new lifecycle or scenario policy hook.

The retained regular release artifact `luncosim-projected-query` has SHA256
`1729e26f5d93bab7887af6405247987cbad8a5a219af39518376c04b8da56591`.
Its exact compiled runtime diff and source hashes are recorded in
`projected-query-build-runtime.patch` and `projected-query-build-source.json`.
The release build passes. Focused bridge tests cover stable/retired/unknown
source identities and both Twin-scoped reflection/exposure boundaries; the
asset owner test covers retired and all-hex source identities. The production
`projected-query-gates` run passes five static-wheel contact checks and seven
Twin identity checks, plus invalid/missing-target and scene-retirement checks.
Its owned process and API port 5412 are released.

The separate `projection-query-before` and `projection-query-after` diagnostics
retain unchanged source snapshots. Five repetitions of 100 body-state reads
have median native elapsed times of 194.134 and 64.210 ms. Entity-pose medians
are 1.644 and 0.775 ms; USD-pose medians are 34.424 and 38.120 ms. Corresponding
20-second frame-only windows are 25.227 and 30.207 FPS, with maxima 103.833
and 107.536 ms. Concurrent simulators remain active, and these diagnostics do
not establish an isolated whole-frame speedup or stable 100 FPS.

The independent `projected-query-acceptance` attempt requests the unchanged
60-second, 100 FPS, maximum-10-ms gate. It stops after three sampled updates
when terrain reports `selection_pending=1`, with wanted/resident both 181 and
no pending presentation. Sources remain unchanged and the owned process/API
port 5410 are released. This is an incomplete qualification, not a successful
sustained performance result. The 100 FPS and spike requirements remain open.

## Directional visibility query cache

The `tracy-ac46` control profiles application revision `ac46e590f` with
20,109,389 zones over 112.2 seconds. In the 60–110 second interval the main
update averages 22.025 ms. The fixed loop averages 9.168 ms, PostUpdate 5.625 ms,
and receipt of the prior render frame 0.307 ms. The worst two updates include
multiple complete fixed cycles, rendering, and other presentation work. Their
27–31 ms render handoff zones include fixed cycles executed during the handoff.
Main-thread directional visibility command application contains 1,071 of 1,089
sampled leaves attributed to the folded query-construction symbol.

The local Bevy change `400fa336e` moves deferred `ViewVisibility` writes through
Bevy's cached system registry. The system retains access metadata and consumes
only the current visibility queues. The selected caster set, pass ordering,
shadow settings, and application-owned policy stay intact. The cache belongs to
the Bevy visibility mechanism and requires no authored hook. The focused test
covers live archetype admission, retired entities, missing components, and an
empty publication after visible entities were written.

The candidate was built at application revision `ac46e590f` with a local Cargo
path override for `bevy_light` only. It passes 39 authored shadow input checks
and all 12 moving-Sun GPU phases. The offscreen receiver raster test failed to
start on the control with an Egui resource validation panic. The windowed checks
supply the renderer evidence for this change.

`bevy-visibility-query/capture.tracy` contains 13,492,553 zones over 210.4 s
for owned PID 401486, with a verified 1 ms CPU sample period. The exact
executable SHA256 is
`0839330ccde40373d017602789e718d9a28dacc241ccb77e90660acbabc096d0`.
Control and candidate use the same Griffin sources, High 2560×1440 tracking
view, and 50-second comparison windows starting 10 s after View activation.
The deferred visibility command mean falls from 0.518 to 0.043 ms, and its p99
falls from 1.197 to 0.118 ms. The enclosing visibility calculation is separate.

The candidate's instrumented 60-second frame window averages 19.415 FPS with
p95 84.480 ms and worst frame 231.993 ms under concurrent builds and simulator
load. The earlier instrumented control averages 44.549 FPS with a 91.118 ms
worst frame. The different host loads prevent attributing the whole-frame
difference to the cache. Owned API ports and the Tracy port are released.
The dependency commit remains local and the simulator keeps its published pin.

## Shared Griffin tether routing

Model revision `ccd7e22` samples routing vectors once and hands them to the
solver adapter and the curve view. During landing standby the view prepares the
routing snapshot only when the previous publication has completed. It never
constructs solver rates, centre-of-mass inputs, or the named scalar write list
for standby drawing.

The production inline comparison matched all 92 ordered solver inputs and
both curves' vertices and normals on the same landed state. The production
window then confirmed both spatial curves continued to publish after the script
change. Their completed revisions advanced from 567/570 to 1961/1962, with no
runtime diagnostic errors. The registered `Verify_GriffinSurfaceOperations`
scene passed all nine authored checks, exited zero at tick 8756, and released
its API port. Sources matched before and after both runs.

The 60-second uninstrumented frame diagnostic averaged 26.197 FPS with p95
49.090 ms and worst frame 188.041 ms. Pending fixed time decreased from 23.573
to 10.116 seconds with zero delta clipping. That run includes catch-up and does
not qualify steady landed performance.

## Yield to a ready render frame between fixed cycles

The two longest `tracy-ac46` updates contain 27–31 ms handoff intervals with
one or more complete fixed cycles. The host now polls the outstanding frame
before admitting completions and again before starting each eligible fixed
cycle. A ready frame ends service at a complete-cycle boundary and the common
accumulator retains all admitted time. The native host supplies readiness and
the time owner retains deterministic cycle admission. No authored policy changes.

The focused time test proves immediate yield, yield after a complete cycle and
recovery of every queued tick. The regular release executable passes the 13
native presentation checks and all eight transport checks. The measured 2×,
4× and 8× rates are 2.000×, 4.000× and 7.988×.

The `host-cycle-yield/landed` diagnostic uses the regular release executable
built at application base `5853ed4b8` and model `ccd7e22`. In 60.36 seconds it
records 4,878 frames at **81.488 FPS**, p95 **14.543 ms**, p99 **18.732 ms** and
maximum **51.657 ms**. 4,778 frames exceed 10 ms. Queued time is 0.016 s at the
first endpoint and 0.010 s at the last, with zero clipping or runtime errors.
The earlier `luncosim-projected-query` executable records 54.832 FPS, p99
32.901 ms and maximum 116.560 ms with the same Twin sources and settled debt.
The application revisions and competing host workloads differ. This comparison
supports the observed frame timings and does not isolate a whole-frame gain
from the yield change. Terrain selection is pending at the sampled endpoints,
so the stricter complete-readiness acceptance also remains open.

## Landed frame tails and scalar-history rollover

`tracy-integrated-f907/capture.tracy` profiles simulator `f907ccab3` with
the local visibility query cache `400fa336e` and model `ccd7e22`. It contains
17,040,555 zones over 122.9 s for owned PID 510210. In the same 50-second
analysis window after View activation, deferred directional visibility
commands average 0.033 ms with p99 0.091 ms. The previous render-frame
receipt averages 0.099 ms. The worst host period is 70.624 ms with 0.201 ms
outside the app update. Its two complete fixed cycles take 30.398 ms in
total and its PostUpdate takes 16.869 ms. The instrumented window also drains
queued fixed time, so it only diagnoses costs.

The corresponding uninstrumented local-cache build has SHA256
`3c3cec899695ce00320df16c73ee111d06375de8144fa9d842047d3973b70d81`.
The 60.33-second landed window records 2,445 frames at 40.904 FPS, p99
51.762 ms, and maximum 65.993 ms. Queued fixed time increases from 0.010 to
0.779 s and delta clipping stays zero. Other render workloads share the
host. This window misses the goal and the workload differs from the earlier
81.488 FPS diagnostic.

A native audit after the timing window identifies four active cascades for
`WorkbenchEguiHost`, each with 1,040 caster candidates, in addition to the
four cascades for the Griffin tracking view. The native PBR light preparation
consumes only 3D views. The audit of 40 consecutive frames in the Tracy run
also finds that every matching 3D cascade clip matrix changes, including the
two maps with empty caster lists.

The second-longest host period is 62.403 ms. It includes a 34.286 ms
`retain_modelica_runtime_state` call inside Update, against p99 1.969 ms
across 1,569 calls. The capture has a 1 ms CPU sample period. Samples on
that worker in the long call resolve against the captured executable's
native address map to the deque-drain-to-vector copy and allocator work.
The scalar history in `lunco-signal` owns chunk sealing. It transfers the
live tail allocation into a shared chunk and recycles one fully expired
buffer when no snapshot holds it. Telemetry producers use the same storage
API, configured rate, and logical retention. This is a shared storage
mechanism with no authored policy hook.

The storage test verifies allocation transfer, snapshot-held expiration,
recycling of an unshared chunk, and exact retained values across resize and
clear. Three existing signal tests also pass for capacity, recording rate,
and time reversal. The release candidate applies the same local Bevy cache as
the control and has SHA256
`45ff266e72400a40f3da0f2f1d54972b2bee8e659286b2c25af3c4fabf10cebc`.
The windowed production `modelica_telemetry_replacement` fixture passes all
10 checks and releases API port 5412. The suite wrapper hit a source-scope
error while discovering another fixture, so the targeted editor runner opened
`modelica_telemetry_replacement.usda` directly.

`history-rollover/landed` records 5,108 frames over 60.31 s at **85.407 FPS**,
p95 **14.372 ms**, p99 **18.122 ms**, and maximum **23.187 ms**. 4,669
frames exceed 10 ms. Queued fixed time at the endpoints is 0.008 and 0.009 s,
with zero clipping and zero runtime diagnostics. The 2560×1440 High tracking
view has all 181 selected terrain tiles drawn and terrain readiness at both
endpoints. The native shadow audit reports the same cascade counts and caster
totals as the control. The production history API reads 451 finite, strictly
ordered samples across a rollover, within the configured 1,500-sample limit.
Twin source hashes match the control. API ports 5410 and 5412 are released.
The control had an additional active render process and accrued fixed debt.
The frame timings establish the candidate's observed performance under its
recorded workload. Stable 100 FPS remains open.
