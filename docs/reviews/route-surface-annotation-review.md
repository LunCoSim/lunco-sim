# Route surface annotation review

Reviewed 2026-10-01 in `codex/lunar-soil`.

The terrain fragment is the authoritative rendered surface. Route annotations
use its DEM-local coordinates and the original centreline segments, so the
stroke follows elevation, CDLOD morphs and stitched seams without height-fitting
vertices or extra navigation points. The full contract lives in
[USD-driven visuals](../architecture/50-usd-driven-visuals.md).

Ownership and hook review: Rhai selects the route, ordered points and width;
USD supplies material and pointer policy. `lunco-luncosim-edit-ui` validates the
view request and captures its frame. `lunco-terrain-surface` validates coverage,
prepares the sparse index and fences publication by source revision; the shared
terrain shader draws it. Coverage and shader compatibility are mechanism
invariants. Gesture meaning still passes through the existing typed
`scene_interaction` policy. This introduces no Rust route or tutorial policy,
USD persistence path, height oracle, or navigation topology.

Preparation coalesces by target and admits at most two workers at each owner.
Invalid concurrency settings fail visibly. The default index has 32-by-32 bins,
at most 64 candidate segments per bin and 4096 segments overall. Segment budget
rejection precedes growth beyond that bound. Terrain owner discovery responds
to addition/removal; idle frames allocate no owner list. Camera and elevation
changes do not schedule annotation index builds.

Verification:

- The production Apollo 15 gate passed all five checks: current publication,
  hidden independent mesh, sparse segment count, unchanged authored points,
  missing-coverage failure and restoration. The real route had five points and
  four segments.
- The production `route_interaction` editor gate passed 26 checks. Headful close,
  wide and diagnostic LOD views showed continuous strokes; a secondary click on
  the painted route resolved its USD curve identity and `route.context` intent.
- Two pure index tests passed, including a 10 km single diagonal and invalid,
  duplicate and over-budget input. A 1000-build sample averaged 83.826 us and
  produced a 20 KiB image for that one original segment.
- Separate High-quality Tracy capture, 1280-by-720, owned API 48131 / Tracy 48133:
  in the 25–40 s stable interval, curve prepare/poll p95 were 42.890/24.505 us;
  terrain prepare/publish p95 were 45.525/34.825 us. No index worker ran in that
  interval. Worker timings across startup, negative gate and teardown ranged
  from 1.122 to 1174.704 us; they do not block the app or simulation tick.

The capture and CSV summaries are local evidence under
`target/route-surface-audit.tracy`, `target/route-surface-zones.csv`,
`target/route-curve-zones.csv` and `target/route-surface-cpu-summary.json`.
Concurrent simulator and compiler workloads affected the measurements. Tracy
numbers are mechanism diagnostics, not clean FPS acceptance.

A separate unprofiled High-quality run at 1280-by-720 alternated the ribbon on,
off, on and off at a fixed camera. Each window retained 240 full-frame timing
samples (the latest approximately four simulation seconds after settling).
Median frame times were 13.154/13.049/13.071/13.148 ms; p95 were
18.033/18.542/18.038/18.923 ms, p99 19.778/20.766/20.200/21.389 ms and maxima
21.375/20.880/23.623/24.609 ms. Inspection confirmed four segments with terrain
bindings when on and zero segments/bindings when off. The variation exceeds
the on/off difference, so this comparison shows no measurable regression;
concurrent workloads prevent an uncontended FPS claim. Raw samples are in
`target/route-surface-frame-ab.json`. The owned sessions were closed through API
`Exit`; no other simulator was controlled or stopped.

Integration into current main preserves its asset-scoped shader source cache;
annotation capabilities are part of that cached interface. The gate takes an
explicit `view_owner`, pauses that source writer during its disposable negative
fixture and verifies restoration of its original pause state. The merged
production build and surface gate (six checks), updated interaction gate
(36 checks), and skill catalogue pass. API documentation was regenerated from
the merged binary's 247-command schema. Performance measurements above belong
to the pre-integration commit `89e03c037` and were not repeated as merged-tree
FPS acceptance.
