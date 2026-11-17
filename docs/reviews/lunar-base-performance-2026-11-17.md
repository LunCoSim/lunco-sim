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

The current release executable's completed bounded landed window measured
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

The later retained-render-handoff comparison is unqualified: the Twin manifest
changed externally during its run, and another simulator consumed GPU time.
It cannot justify retaining the scheduling experiment. Runtime scheduling
remains unchanged; the current source retains the native First handoff and
normal schedule executors. All task-owned sessions and collectors exited.
The unrelated simulator on API 4293 was left untouched.

Attribute the remaining application/physics and render preparation costs with
caller-complete evidence, then qualify an unchanged source cohort. Preserve
numerical cadence and image quality.
The outstanding acceptance requirement is a complete 60-second landed window
at 100 FPS with every frame at or below 10 ms.
