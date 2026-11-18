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
in `luncosim-tracy-baseline.zst`, uncompressed SHA256
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

The follow-up `history-rollover/tracy/retry/capture.tracy` contains
30,027,665 zones over 109.5 s for owned PID 597859, using the same local Bevy
cache and Twin source hashes. The executable SHA256 is
`376c8a486ede7cea6102e880de0e401f285e0caadfbfe8d5106e3ba7962c547e`.
Analysis uses the same 50-second window offset. Across 3,092 calls the
Modelica retention p99 is 0.524 ms and maximum is 9.912 ms, compared with
1.969 and 34.286 ms across 1,569 control calls. The candidate history contains
425 ordered samples, including sample 256 at model time 56.383 s. The
verified 1 ms CPU samples in the longest call include allocator work, with
no deque-drain-to-vector copy. Host workloads and fixed debt differ between
captures. The instrumented frame window averages 61.842 FPS, with queued fixed
time below 0.014 s at both endpoints and zero clipping. The app and collector
exit and release their ports after the capture.

## Scoped tool lookup

The landed trace records 902 main-thread CPU samples inside the quiet native
task calls for Griffin's Compact Egress Observation. Native symbols include
allocation and hash-table growth in the shared tool registry. The registry now
groups every registration by tool name. A named lookup visits only that name's
layers, and discovery uses the same scope resolution. The registry retains one
committed active Twin under the registration lock. Tool dispatch uses its
existing captured scope and scene-generation validation. This is a registry
mechanism and requires no authored hook change.

In five optimized batches of 10,000 lookups over 128 tool names and one active
Twin override, the median elapsed time per lookup is 10.510 µs on the control
and 0.054 µs on the candidate. This measures the registry operation.

Both focused registry tests pass. A windowed production Twin-switch probe
passes five authored checks for discovery, binding, invocation and outgoing
registration retirement. The retired lookup also returns HTTP 404. The
broader guided UI test passes the same 36 authored checks on both releases
before the same watched-menu fixture fails to publish.

The locked release build uses the checked-in Bevy pin. Its executable SHA256 is
`9e186d16713e23edacfb618799ac82b9b7e6a8d6873cfd4a314bd08a14755433`.
The `tool-resolution/landed` diagnostic at High 2560×1440 records **57.603 FPS**,
p99 **25.288 ms**, and maximum **67.957 ms** over 60.26 s with advancing physics.
Queued fixed time is below 0.016 s at both endpoints, with zero clipping, zero
runtime errors, and all 181 selected tiles drawn. The same session reports
65.237 FPS with zero fixed ticks during a 20.40 s pause and 56.555 FPS over
20.46 s after resuming. Two other simulator sessions and an additional active
GPU process are recorded in its workload inventory. The run falls short even
with a held simulation clock. It does not isolate the registry's frame impact.
Twin source hashes match, the history API reads 567 ordered samples, and the
owned app releases API port 5410.

## Render receipt with an empty fixed queue

`paused-render-tracy/capture.tracy` uses the earlier instrumented telemetry
candidate with the local directional visibility cache. It contains 18,932,281
zones over 107.9 s for owned PID 629523. The landed timing window holds fixed
tick 1775 with 3.338 s of queued simulation time retained during pause.
The program records 49.849 instrumented FPS over 45 s. In its central
35-second window, the main thread spends a mean 6.644 ms waiting for the render
world before running Update (3.402 ms) and PostUpdate (5.095 ms).
Render-world work averages 9.915 ms in the same window. GPU timestamps record
2.187 ms for opaque, 1.934 ms for early directional shadows and 0.833 ms for
the depth prepass. These covered passes do not constitute a GPU frame total.

The native host now checks the time owner's queued fixed demand. With no
full cycle requested, main-world presentation overlaps the previous render
and receives the render world at extraction. With outstanding demand, the
same host can service completions before sampling presentation. The receiver
never admits fixed cycles during extraction, so a frame uses a single
presentation sample after the last admitted cycle. This is a host scheduling
mechanism with no authored hook change.

The queued-demand and complete-cycle yield tests pass. The regular locked
release passes all 13 native presentation tests and all eight transport
checks. The observed 2×, 4× and 8× rates are 2.005×, 4.004× and 8.020×.
The executable SHA256 is
`2bef47464c09e4b58ee45a90baa5c9eaaf00ed7ed8cd398f381a95a272a550a4`.

`extract-time-render-receipt/landed` records **56.664 FPS** over 60.28 s
with p99 **25.858 ms** and maximum **36.033 ms**. Pending fixed time remains
below 0.016 s at the endpoints with zero clipping. In the same windowed
session the held simulation records **92.959 FPS** over 20.26 s with zero
fixed ticks, p99 **15.311 ms** and maximum **25.786 ms**. The prior locked
build measured 65.237 FPS and maximum 83.947 ms while paused with the same
three other simulator processes present, under varying GPU activity.
The new session resumes at 55.980 FPS over 20.49 s. Every phase retains all
181 drawn terrain tiles at both endpoints, with no runtime errors. Source
hashes match and owned API ports 5410 and 5412 are released. The running
100 FPS and 10 ms worst-frame gates remain open.

## Fixed service under a delayed render handoff

`locked-main-tracy-93b/capture.tracy` profiles the checked-in Bevy pin and
simulator revision `93bceff2c`. The adjacent Tracy collector records
18,033,665 zones over 176.8 s for owned PID 682444. Griffin uses the tracking
camera at High 2560×1440 with all 181 terrain tiles drawn at both checkpoints,
unchanged Twin sources, zero runtime diagnostics and zero clock clipping.
It records 318 ordered tether-torque history samples. The profiled session
advances from fixed tick 1500 to 4164 during the 60-second sample. Queued
fixed time grows from 45.732 to 63.816 s, with other active GPU processes.
This is a catch-up diagnostic. The settled release run above supplies the
clean FPS result.

In the matched 50-second interval, the three longest frame periods are
152.018, 140.863 and 130.899 ms. Each spends 94–98 ms in complete fixed
cycles, with 9, 5 and 4 additional cycles admitted during the respective
handoffs. Those additional cycles account for 63.765, 71.241 and 61.449 ms
respectively. The long receiver spans include that simulation service.
Many completion wakes in a single handoff can form a burst; the wait after
the final late cycle is at most 1.388 ms in this window.

The time owner grants one budget for both ordinary fixed work and render
overlap per host update. Completions reuse the remaining time and cycle
allowance. Exhausting the budget leaves all unconsumed time in the existing
accumulator and allows the frame to reach presentation. The timing profile
includes both service phases and records the final debt.

The uninstrumented release in `host-update-budget/landed/receipt.json`
measured 5,845 consecutive frames after the landing handoff over 60.34 s.
At High 2560×1440 the mean was 97.683 FPS, p95 was 13.637 ms, p99 was
16.083 ms and the maximum was 51.706 ms. 3,623 frames exceeded 10 ms.
Both endpoints had 181 of 181 terrain tiles drawn, zero runtime warnings or
errors and less than 8 ms of queued simulation time. The fixed clock advanced
3,688 ticks between the qualifying API snapshots and reported zero time
clipping. The same session measured 138.720 FPS paused and 92.943 FPS after
resuming. The two other GPU clients were idle at the sampled endpoints.
The different machine load limits comparison with the preceding run.

Both host-budget unit cases passed, including repeated completion wakes in
one update. The windowed presentation gate passed all 13 checks. The transport
gate passed all eight checks and measured 1.999×, 4.002× and 7.999×.
The authored Griffin surface-operations verification passed all nine checks
at tick 8,756, including four grounded wheels at the end of the descent.

## Command resolution during tether control

In the settled-camera interval of `locked-main-tracy-93b/capture.tracy`,
1,121 tether-controller ticks with an empty event queue averaged 0.925 ms.
Their p99 was 5.897 ms. Of 1,020 native CPU samples on that task's thread
inside those calls, 12 landed in construction of the matching registration
vector inside `find_api_command`. Ordinary command dispatch now uses the
unique short-name index maintained by Bevy. A registry-reported ambiguity
still distinguishes multiple types from replacement of the same type.
Public markers, reflected events and runtime visibility continue to govern
admission. These samples measure the controller and identify an allocation.
They do not quantify a whole-frame saving.

## Mesh bounds for vertex displacement

In the 50 seconds beginning ten seconds after the View perspective activated,
`indexed-command-tracy/capture.tracy` recorded 2,647 calls to
`update_displaced_mesh_bounds`. They spent a mean 0.322 ms and p99 0.584 ms
reading shader parameters and checking mesh identity and events. The maximum
was 0.794 ms. The render adapter now reads displacement padding when its look
changes and updates bounds on the affected mesh. Its lifecycle test checks a
late mesh load, an edit through the same handle, restored native extents after
look removal, manual bounds on another mesh, and rejection of negative padding.

The uninstrumented `reactive-displacement-bounds/landed/receipt.json` window
recorded 5,509 frames at 92.036 FPS, p99 17.161 ms, maximum 33.624 ms and
3,628 frames over 10 ms. Physics advanced 3,688 ticks between qualification
snapshots, with 181 terrain tiles drawn and 455 ordered torque samples.

With the same recorder debug spans enabled in both Tracy captures, the
original bounds system averaged 0.311 ms across 2,751 landed frames,
p99 0.632 ms and maximum 1.216 ms. The two replacement systems together
averaged 0.014 ms across 2,651 frames, p99 0.031 ms and maximum 0.084 ms.
These are CPU times for bounds maintenance in matching 50-second windows.

## Synchronous telemetry recording after landing

In `indexed-command-tracy/history-growth`, a single landed recording update
expanded 8,099 history tails at the same 128-sample boundary. The allocator
spans totaled 11.232 ms of its 18.378 ms wall time. The same 8,099 channels
also grew together at earlier boundaries. The signal registry now phases its
first chunk length by slot and reserves subsequent chunks on demand. This
changes storage allocation only. Sample times, values, rate admission, and
logical retention continue through the same recording path.

In the corresponding 50-second window of `history-pages-progressive/tracy`,
the largest growth group fell to 33. The longest retention call fell from
18.378 to 3.254 ms. More updates now perform a small allocation. Mean retention
time rose from 0.084 to 0.109 ms and p99 from 0.694 to 1.351 ms. Before the
settled window, one candidate call took 64.269 ms. These results establish a
reduction in synchronized growth after landing.

Two runs per binary recorded 8,192 synchronous channels for 1,800 batches
using the production allocator, and verified all 12,288,000 retained samples.
Worst batch time was 3.529–3.811 ms before the change and 0.645–0.682 ms after
it. Average batch time rose from 0.118–0.121 to 0.175–0.188 ms. Peak resident
memory was 267–268 MiB and 280–281 MiB respectively. Four history tests passed,
including a shared snapshot surviving resize, eviction and clearing. The
windowed Modelica telemetry replacement scenario passed all ten authored checks.

The combined uninstrumented release recorded 5,808 frames at 97.143 FPS,
p95 14.588 ms, p99 16.166 ms, maximum 58.367 ms. 3,605 frames exceeded 10 ms.
The 60.30-second polling window advanced 3,685 fixed ticks between qualification
snapshots, with 181 terrain tiles drawn, 454 ordered torque samples, zero time
clipping and clear runtime diagnostics. A second GPU client showed 8% activity
at the final sample. The 100 FPS and frame-stability requirements remain open.

## The touchdown transition

The indexed-command Tracy capture shows two consecutive sources of delay.
During fixed tick 1,341 the mission's landed-route task took 329.566 ms,
including 183.098 ms in the nine-operation transient USD transaction.
The enclosing frame took 358.346 ms. In the first host update after four-pad
handoff, authored program projection took another 56.956 ms, telemetry
projection 18.623 ms and exposure publication 13.208 ms. The frame took
125.595 ms and ran zero fixed ticks. A second, schedule-only capture recorded
141.796 ms for that following frame.

The program projector computes Modelica network membership from composed USD.
It now locates network roots and program candidates in a single traversal and
tests the program paths against each native collection query. In the new
Tracy capture the corresponding authored-runtime pass took 20.436 ms.
Four target translations and one visibility edit triggered the projection,
with no structural resync. Telemetry projection still took 25.718 ms.
The stage's prepared and live readers both matched native full expansion
for nested inclusions, exclusions, properties and an expression collection.

The uninstrumented control and candidate used the same working High profile,
2560×1440 camera and Twin source hashes. The five-second handoff windows had
maxima of 365.936 ms and 469.858 ms respectively. Once settled, the control
recorded 83.742 FPS, p99 18.452 ms and maximum 38.952 ms. The candidate
recorded 93.292 FPS over 5,582 frames, p95 14.440 ms, p99 16.349 ms and
maximum 28.048 ms. Of these, 3,639 exceeded 10 ms. The candidate advanced
3,687 simulation ticks between checkpoints, retained 452 ordered telemetry
samples and reported zero runtime diagnostics. The surface-operations
verification passed at tick 8,756. Concurrent host activity varied and the
collection change runs on projection invalidation. These settled frame timings
do not isolate its effect. Receipts and Tracy exports are in
`.cache/perf/lunar-100fps/scoped-collection-members/`.

## Prepared Griffin study parameters

At landing, the route controller read compact-egress parameters and CAD wheel
dimensions by opening their individual SysML files. Native samples in the
mission task included syntax-tree traversal, qualified-name lookup and semantic
model cloning. Griffin model revision `dea9f68` now reads the mounted Twin’s
prepared model. All 74 named egress and controlled-descent values and both CAD
values matched the individually opened files. The surface-operations mission
passed all nine checks at tick 8,756.

With the same production binary, the maximum in the five seconds around
handoff fell from 469.858 ms to 189.323 ms. The subsequent uninstrumented
60-second sample recorded 98.769 FPS across 5,908 frames, p95 13.665 ms,
p99 16.381 ms and maximum 26.101 ms. 3,621 frames exceeded 10 ms. The
simulation advanced 3,685 ticks between checkpoints and retained 454 ordered
torque samples. Terrain endpoints had 193 tiles in this run and 181 in the
preceding run. In the separate Tracy capture the mission task still took
375.258 ms, including 333.132 ms for the nine-operation transient transaction.
A release check was compiling during that capture. The samples identify work
inside the hitch; the uninstrumented run measures frame times. Evidence is in
`.cache/perf/lunar-100fps/griffin-prepared-datums/`.

## Reopening an admitted layer closure

Each private authoring stage opened a new resolver. It parsed every dependency
again to validate its native structure and discover its composition arcs, even
though the loader had already completed that scan. A recipe snapshot now shares
each immutable layer buffer, its successful scan for the exact anchor and its
native OpenUSD decoder buffer. Changed bytes still pass native validation and
a new anchor resolves its own relative arcs.

In the landed Tracy capture the nine edits took 4.624 ms inside an 8.358 ms
mission task. The preceding capture measured 333.132 ms inside a 375.258 ms
task while a release check was compiling. In the separate regular release runs
the wall interval from touchdown confirmation to four-pad handoff was 170.142 ms
and 8.098 ms respectively. The five-second handoff maximum fell from 189.323 ms
to 66.333 ms. The snapshots qualified contact at ticks 1,357 and 1,341
respectively.

The candidate's 60-second settled run measured 90.595 FPS over 5,427 frames,
p95 14.433 ms, p99 16.795 ms and maximum 20.193 ms. 3,658 frames exceeded 10 ms.
The simulation advanced 3,691 ticks between checkpoints, retained 453 ordered
torque samples and reported zero runtime warnings or errors. All 181 terrain
tiles were resident at the final checkpoint. The official surface-operations
verification passed all nine checks. The composition suite passed 16 tests,
including replacement and reanchoring of shared bytes. The stage test covered
a reloaded source. The authoring test checked inheritance, references and
layer strength across USDA, USDC and USD root identifiers. Evidence is in
`.cache/perf/lunar-100fps/shared-layer-analysis/`.

The separate strict acceptance run stopped during the measurement when the
terrain selector reported a pending selection with 181 of 181 tiles resident.
Application readiness remained true. The raw 60-second diagnostic above
also exceeded the 10 ms frame budget.

## Projected path lookup during tether operation

Tracy attributed 20 of 320 main-thread samples inside the landed tether task
to global prim-path matching. Stage-scoped lookup also built query metadata
for four ECS queries on each call. Structural reconciliation and Rhai now
share the scene-owned prim index. The caller resolves live registration,
preview ancestry and mount identity from the indexed candidates. Reflected
path replacement updates the same index through the ECS insertion lifecycle.

In the production tracking view at High 2560×1440, each trial resolved one
present and one missing path 100 times. Median script elapsed time across
five trials fell from 3.131 to 0.110 ms globally, from 38.830 to 0.152 ms
within the source mount and from 17.267 to 0.160 ms within the document. The
live Rhai test passed all ten identity, restoration and rejection checks.
The lifecycle test also covered pre-existing projections, a preview, a second
stage and despawn. The nine mission checks passed at tick 8,756.

The separate landed release run measured 90.720 FPS over 5,425 fresh frames.
Median, p95, p99 and maximum frame times were 11.349, 15.144, 17.431 and
25.859 ms. There were 3,664 frames above 10 ms. The run advanced 3,686
fixed ticks between checkpoints and retained 455 ordered torque samples.
A Tracy CSV export overlapped the beginning of this diagnostic. The separate
strict run lost terrain qualification with one selection pending and all
181 tiles resident. Reproducible lookup savings have yet to yield the
requested sustained frame budget. Receipts are in
`.cache/perf/lunar-100fps/projected-path-index/`.

## Griffin tether routing and runtime UI output capture

Model commit `b4de8d5` reads the six routing coordinates needed to draw both
tethers. The physical controller still consumes all 42 required outputs. Five
same-tick comparisons checked all six selected values against the complete
valid snapshot and rejected a missing coordinate. Ten visual reads took a
median 0.287 ms, compared with 2.116 ms for ten full reads. In
`routing-port-selection/qualified`, both curves advanced during the 60.32 s
landed window. It averaged 96.675 FPS with p99 15.542 ms and maximum 70.273 ms.
The separate strict run stopped on a pending terrain selection while all 181
tiles were drawn and resident.

The runtime UI producer captures live participant output values in slot order
and retains their immutable names. Its existing worker constructs the full and
authored public hook maps from that single sample. The mechanism changes
presentation data ownership and preserves the installed Rhai hook, its sampling
clock and its scene and subject checks. A landed Griffin probe inspected all
8,255 samples across 19 participant entries on the two surfaces. All 312 public
values matched their corresponding full-map values.

The separate uninstrumented `ui-output-snapshot/landed/frame-result.json`
contains 5,698 consecutive frames over 60.33 s. It averaged 95.243 FPS with p95
13.661 ms, p99 16.224 ms and a 41.857 ms maximum. Of these, 3,604 exceeded
10 ms. The process incurred no major page faults during that window. The two
other simulator sessions remained active. Both the mean and worst frame exceeded the 10 ms budget.

The 25 s Tracy windows begin 5 s after activating the tracking perspective.
The `shared-layer-analysis` capture recorded 998 surface fact captures at
0.518 ms mean and 1.277 ms p99. The candidate recorded 1,000 at 0.138 ms mean
and 0.357 ms p99. The worker policy span averaged 1.912 ms and 1.968 ms
respectively. The different engine and Twin revisions limit this comparison to the observed
owner timings.
The headless and windowed UI gates each passed 13 preparation and 20 scene
checks. All nine Griffin surface-operations requirements passed at tick 8,756.
The strict UI-candidate run qualified at 44.08 s, then stopped after 31 samples
when the terrain owner reported one pending selection. Its 181 wanted tiles
were resident, presentation had no pending work, and the source hashes matched.

## Current local visibility build

The uninstrumented `current-visibility/luncosim` combines engine `ea1d85b0b`,
model `b4de8d5` and local Bevy lighting `400fa336e`. The local lock differs
only in the source of `bevy_light`. The engine repository retains its published
dependency revision.

At High 2560×1440 after handoff, 5,903 consecutive frames over 60.31 s averaged
98.696 FPS. The median was 10.930 ms, p95 13.420 ms, p99 15.329 ms and the
maximum 19.893 ms. There were 3,597 frames over 10 ms. Authoritative physics
advanced 3,685 ticks between the surrounding checks. The final check found
181 of 181 wanted tiles resident and zero runtime diagnostics.

Holding physics in that same session kept tick 5,373 constant over a separate
20 s window. Its 2,596 frames averaged 130.833 FPS with p99 10.777 ms and a
13.089 ms maximum. Physics resumed and all 312 authored public output values
matched their full maps. The independent strict run stopped after 37 samples
with a terrain selection pending, 181 tiles resident and no pending presentation.
The 100 FPS and 10 ms maximum remain open. The two other simulator sessions
remained running through validation.

## Committed globe contact cover and indexed HUD program lookup

The globe collision owner retains a replacement selection until it has admitted
every selected tile and the native collider bounds are valid. It then releases
the selection and uses the resident map for the continuing near-contact bounds
check. A demand change starts another replacement. The runtime HUD resolves
direct child programs and relationship targets against the existing stage
index and checks the primary mount for each candidate.

Across matched 25-second landed Tracy windows, globe collision preparation
averaged 0.220 ms before and 0.118 ms after (1,430 and 1,492 calls).
Median time fell from 0.150 to 0.045 ms. Its p99 was 1.119 and 1.184 ms.
Across the full captures, the HUD surface-root refresh p99 was 2.212 and
1.462 ms. The settled HUD cache hits averaged under 1 µs in both builds.

The native planetary flight/collar gate passed all 14 checks at tick 8,103
and produced identical numeric flight evidence on both releases. Griffin's
nine checks passed at tick 8,756. The windowed Griffin and FLIP HUDs matched
all 27 control program records including source, action, path and status.
All 312 public output samples agreed with the corresponding participant data.

The separate release run on the checked-in dependency versions measured
96.983 FPS across 5,801 consecutive landed frames at High 2560×1440.
Median, p95, p99 and worst were 11.054, 13.438, 15.805 and 26.118 ms.
3,610 frames exceeded 10 ms. The scene advanced 3,685 fixed ticks between
checks. Runtime diagnostics were clear. The strict driver lost qualification
after 37 samples with one pending terrain selection, all 181 wanted tiles
resident and zero presentation work pending. The 100 FPS frame budget is open.

In the separately filtered control capture, the worst 62.421 ms frame spent
46.821 ms at render extraction. The prior frame's opaque pass finished before
the stall and the next directional shadow pass started 61.284 ms into the
frame. These GPU timestamps locate the gap in submissions. They do not
identify the reason for the renderer's wait. Captures and receipts are under
`.cache/perf/lunar-100fps/committed-globe-cover/` and `landed-critical/`.

## Spatial picking for scene cameras

The workbench egui host paints window chrome and receives pointer hits in
window coordinates. Input processing removes that camera's rays before
spatial backends traverse the scene. Camera identity scopes the removal to
the host, including when the pointer also covers another camera's target.

The landed Tracy windows recorded two ray-culling passes in all 1,492 control
updates and one pass in all 1,460 candidate updates. Mesh picking averaged
0.591 and 0.530 ms respectively, with p99 falling from 1.278 to 1.164 ms.
In an owned release window, injected pointer clicks switched View to Analyze
and back. Both revisions hit the authored route geometry and emitted its
context event. Repeated route tests on both builds failed the same possessed
rover right-click selection assertion. The separate graph HUD gate stopped
before input when its empty HUD did not report a rendered rectangle.

At High 2560×1440 the release frame sequence covered 6,173 landed frames.
It averaged 103.313 FPS. Median, p95, p99 and worst frame times were
10.596, 12.856, 14.881 and 24.706 ms. Of these frames, 3,580 exceeded 10 ms.
The simulation advanced 3,682 fixed ticks and the tracking camera retained
all 181 wanted terrain tiles. The separate strict run lost terrain selection
readiness after three samples with all tiles resident and no presentation
work pending. Stable 100 FPS remains open. Measurements and the native
input receipts are under `.cache/perf/lunar-100fps/workbench-world-rays/`.

## Rigid-body demand identity and infinite far-plane equivalence

In the landed scene, positional comparison across successive ECS query results
reported 25.38 m of motion whenever the result order changed. At the same times
every one of the 34 current or predicted positions was within 0.3 mm of a
previous position. The 16 m selection threshold triggered two unnecessary
globe cover rebuilds roughly every half-second. The physical cover now
compares both projections with the stored projections for the same rigid body.

Across matching 25 s Tracy windows, globe contact preparation p99 fell from
1.150 to 0.116 ms. Of 1,460 control updates, 98 exceeded 0.5 ms. All 1,431
candidate updates took less than 0.3 ms. The planetary flight gate passed 14
checks with identical numerical evidence at tick 8,103, and Griffin's nine
checks passed at tick 8,756.

The tracking view also alternated between two selection signatures while every
finite frustum plane remained inside its retained envelope. The constant
far-plane offset differed by 5.55e-17. Canonicalizing the zero-normal
inequality by its truth value preserves its entire footprint. A projection
test checks positive homogeneous rescaling and rejects an oppositely signed
far inequality.

The strict release run kept terrain ready across all 97 samples, with 181 of
181 tiles drawn at the end, zero runtime diagnostics and zero clock clipping.
Its 6,019 frames averaged 99.332 FPS, with p95 13.225 ms, p99 15.987 ms and
a 64.509 ms maximum. There were 3,692 frames over 10 ms. The independent
frame-history-only run averaged 101.599 FPS across 6,068 frames. Its p99 was
15.357 ms, its maximum was 57.017 ms, and 3,588 frames exceeded 10 ms. Both
used the same High 2560×1440 worktree profile and tracked the landed Griffin.
Receipts are in `.cache/perf/lunar-100fps/identified-globe-foci/`.

## Integrated release, local Bevy and scheduler measurements

Engine `456deba91` and the checked-in dependency pin recorded 94.997 FPS
across 5,707 frames with local main's asset root and High 2560×1440. In the
strict driver's minute, p99 was 16.679 ms, worst was 25.493 ms and 3,672 frames
exceeded 10 ms. All 95 terrain samples were current with 181 wanted, resident
and drawn tiles. The release using the worktree High preset and local Bevy
lighting `400fa336e` recorded 96.771 FPS across 5,847 frames, p99 16.474 ms
and worst 22.010 ms. All 96 terrain samples were current. Both source checks
passed and both sessions reported zero runtime warnings or errors. The Bevy
patch is local. The simulator retains its published dependency pin.

A 150-second frame-history run with the main asset root recorded 14,967
consecutive frames, 99.809 FPS, p99 15.427 ms and worst 62.760 ms. Linux
scheduler readings put 42.99 ms of CPU time on one Bevy compute-pool worker
during a 49.18 ms portion of that frame. The main thread used 1.94 ms of CPU
and the render thread 2.90 ms across the same scheduler samples. During a
42.12 ms frame another compute-pool worker used 17.84 ms across 21.93 ms
of sampled time. A second 150-second run recorded 40.19 ms on a compute
worker in a 60.91 ms frame. Linux charged that worker four 10 ms ticks of
kernel time, zero user ticks, 64 minor faults and zero major faults across the
sampled interval. A 53.80 ms frame likewise registered three kernel ticks, zero
user ticks and 116 minor faults on its busiest worker. These counters establish
where time accrued. They do not identify the allocating subsystem.

The strict receipts are under `identified-globe-foci/strict-main-assets/`
and `identified-globe-foci/local-visibility/acceptance/` in the performance
cache. The ordered frames and per-thread observations are in
`identified-globe-foci/scheduling/`.

## Native faults during landed camera and telemetry work

With main's High profile and the released dependency pin, a 150-second
diagnostic recorded 15,168 consecutive frames at 101.156 FPS. The 37.858 ms
frame coincided with one failed synchronous compaction and a huge-page fault
falling back to smaller pages. In a 21.37 ms interval the kernel scanned
409,833 migration pages and 198,085 free pages. A 27.188 ms frame showed
another failed compaction. These Linux VM counters cover the whole machine.
A second launch with huge pages disabled only for that process recorded
13,761 frames at 92.031 FPS and a 22.513 ms maximum. Both passed the same
312 public-output checks at High 2560×1440. This experiment changed no product
or system setting.

Linux perf sampled 53,572 minor faults on the eight Bevy compute workers of
the symbol-bearing release. At 13,165 faults the instruction pointer was in
the entity hash-set clone. All 13,165 stacks contained the same return address
56 bytes from the stack pointer. The preceding direct call in the spring-arm
system targets that clone. Other samples included allocator growth during
Modelica scalar recording. The worst frame in this run was 26.447 ms. The
samples establish sources of repeated faults. Attribution of the prior
60 ms frames to a particular call remains open.

The native prop sweep borrows the jointed vessel filter already retained by
the camera. Avian tests its existing mask and vessel membership, then asks
the terrain query about each candidate's exact entity. The terrain query uses
the same DEM root and collider-tile components as the source sweep. This
changes allocation and traversal at the geometry boundary. The follow policy
continues to receive the same obstruction-distance contract. Native receipts
and fault samples are in `identified-globe-foci/scheduling/` under the
performance cache.

The corresponding 25-second Tracy windows contain 1,431 spring-arm calls
before the change and 1,417 after. Mean cost fell from 0.138 to 0.130 ms and
median from 0.132 to 0.123 ms. P99 measured 0.242 and 0.245 ms respectively.
The longest sampled call decreased from 0.711 to 0.420 ms. The candidate
capture contains 25,188,288 zones. Both windows used the worktree High profile.

On main's High profile at 2560×1440 the separate release candidate recorded
5,580 consecutive frames at 92.525 FPS, p99 17.132 ms and worst 67.442 ms.
Of these, 3,687 exceeded 10 ms. Repeating the control on the same desktop
recorded 5,136 frames at 84.957 FPS, p99 39.941 ms and worst 81.587 ms.
Both windows ran with two other simulator processes. The variation in whole
frames exceeds the isolated camera cost. Every terrain sample passed (95 and 91
respectively), the fixed clock reported zero delta clipping, the admitted
Twin sources matched and each run ended with zero runtime diagnostics. The
camera-navigation suite passed all 58 authored checks across follow, rapid
motion, free flight, capacity, rebasing, policy and zoom. Receipts and the
Tracy capture are under `borrowed-camera-filter/` in the performance cache.
