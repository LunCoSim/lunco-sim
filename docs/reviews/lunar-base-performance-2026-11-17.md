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
