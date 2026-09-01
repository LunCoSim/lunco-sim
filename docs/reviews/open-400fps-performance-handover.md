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
No second propagation implementation, quality reduction, fallback, or local
BigSpace fork is part of the implementation.

## Verification

- `cargo clean` from the optimization checkout removed 16.2 GiB after the
  checkout reached 98% disk usage; all evidence below is post-clean.
- `cargo build -j 4 -p lunco-luncosim --bin luncosim --features ui`: passed.
- `scripts/run_rust_tests.sh -j 4 -p lunco-celestial --lib`: 128 passed.
- Production run used `target/debug/luncosim`, X11, High quality,
  `--no-vsync --no-throttle --log-diag`, API 4366, and the authored
  `traverse_apollo15.usda` scene. `/api/ready` returned
  `ready=true`, `world_hold=false`, `faulted=false`, `pending_count=0`.
- Settled diagnostics reported 4100 entities and approximately 33–79 FPS with
  approximately 12.6–30.3 ms samples in the captured window. The API `Exit`
  command was accepted and port 4366 was verified closed.

## Remaining blocker

The 400 FPS acceptance target is not met. The maintained BigSpace dependency
is pinned to the latest reviewed `bevy-0.19` revision available here
(`5f255228e9b4…`), whose high-precision propagation path still owns its
per-frame worker/channel fan-out. The remaining render/update budget is also
well above the 2.5 ms frame budget required by 400 FPS. Resolving that gap
requires an upstream BigSpace propagation improvement and further measured
render-owner reductions; an application-side duplicate or degraded path is
not an acceptable substitute.

## Acceptance

Close only after a real production High-quality Apollo run sustains at least
400 FPS with p95 frame time at or below 2.5 ms, while the required physics,
visual, headless/windowed, and API shutdown checks remain green. Until then,
keep this handover open and record the exact upstream/render evidence here.

See [`open-200fps-performance-handover.md`](open-200fps-performance-handover.md)
for the shared profiling commands and BigSpace architectural constraints.
