# Wheel trail contact and terrain review

Reviewed 2026-10-01 in `codex/lunar-soil`.

## Ownership and contract

Avian/mobility supply solved contact, suspension geometry and tire dimensions.
`VehicleTrailPlugin` records disconnected physics-frame history after wheel
ray results in `FixedPostUpdate`. The sampler uses physics heading without
visual wheel spin and requires compressed, load-bearing static support.
`ColliderTileOf` maps streamed colliders to their DEM owner. Airborne, inverted
and unsupported contacts end the stroke; landing starts a separate stroke.
Ordinary authored static supports retain solved contact-plane ribbons.

`VehicleTrailSettings.max_points_per_wheel` defaults to 32768: approximately
16 km at the unchanged half-metre sampling spacing. A moving endpoint renders
sub-spacing motion without recording points every tick. Only the oldest points
retire at that configured bound. Invalid bounds stop recording with an owner
warning. Width is the realized tire width; paused width edits invalidate its
presentation. Frame changes and `SceneTeardown` retire histories and visuals.

`lunco-terrain-surface::annotations` owns ground footprints. It reduces each
continuous stroke in blocks of at most 64 legs, with centreline error bounded
by one percent of the half-width, preserving turns and contact breaks. Source
segment arrays are immutable and shared with queued/worker snapshots.

The index preserves every retained segment. A 32-by-32 root grid subdivides
crowded cells into four children, to depth 12. A fragment walks one child per
level and evaluates at most 64 references at its leaf. Generic spatial math
extends the existing annotation owner; terrain LOD coordinates do not own this
annotation lookup. Default allocation bounds are 262144 segments, 65536 nodes
and 1048576 references. Root references are bounded before allocation grows.
Density/depth, segment, node, reference and precision overflow fail explicitly.
There is no alternate mesh path or silent segment retirement for DEM tracks.

At most two workers prepare indexes. Immutable CPU state stays f64; checked
narrowing happens once at the RGBA32Float GPU image boundary. The 256-wide image
stays within 8192 rows, including geometric capacity growth. Image capacity
grows in powers of two and stays until its publication generation retires.
Continuous revisions coalesce and retain the current image. Source topology,
snapshot revisions, settings and shader changes fence stale results.

`lunco-render-bevy::shader_look` tracks image descriptors and refreshes dependent
material bind groups when a resize replaces the GPU texture. Equal-descriptor
content uploads retain the binding and readiness. All custom-material texture
roles share one dependency reader. Authoring hit resolution excludes wheel
histories before distance scans because they have no USD prim identity.

Hook review: these are generic continuous contact, geometry, spatial lookup and
render-resource lifecycle mechanisms. Inputs are existing typed engine facts;
outputs are bounded history, annotations and GPU bindings. The shared visual
plugin serves the viewport and recorder. No tutorial policy, authored USD field,
new terrain query model or interpreted contact loop is introduced. Authored
Rhai scenarios own behavioral verdicts. The canonical lifecycle is in
[USD-driven visuals](../architecture/50-usd-driven-visuals.md#motion-trails-are-bounded-physics-history).

## Verification

- Production `cargo build -j 4 -p lunco-luncosim --bin luncosim` passes.
- Three filtered history tests pass with production feature unification:
  moving endpoints/contact breaks, bounded/frame-reset history, and retaining
  both ends and all 10001 half-metre samples of a 5 km lane.
- Five annotation tests pass: sparse long lines, invalid/overcrowded rejection,
  dense-cell subdivision retaining all segments, bounded reduction preserving
  turns/gaps, and eight curved 5 km lanes. The last fixture verifies sampled
  oldest/middle/current coverage through the actual GPU texture ABI.
- Eight curved 5 km lanes prepare in 23.78 ms on a worker and produce 3.39 MiB
  before capacity padding. Eight straight 1024-leg histories reduce to 128 GPU
  segments, 48 KiB and 244.76 microseconds per preparation. These fixture timings
  measure worker preparation, not FPS. No additional ground samples are used.
- The focused binder test passes: resizing refreshes dependent materials;
  equal-descriptor content publication does not rebind them.
- On the production binary, the exact Summer Space School
  `traverse_apollo15.usda` contact gate passes 15 checks after 30 simulation
  seconds and at least 20 metres of ground travel, then verifies real airborne,
  roof and disconnected upright-landing behavior. All eight physical/render
  tire widths are approximately 0.30 m.
- The route gate passes eight checks, including all 2002 legs of a 5 km loop
  within the supplied DEM, missing-coverage failure and restoration of authored
  route facts and writer state. The fixture uses a 40 m circle around the first
  waypoint and an outer connector 300 m away, within this scene's terrain.
- `target/trail-long-path.png` was inspected headfully: the loop and connector
  follow terrain without gaps. A separate 45-observation drive measured 50.49 m;
  `target/trail-long.png` shows continuous wheel-width tracks reaching the rover.
  The long-path renderer fixture does not claim kilometres of physical driving.
- Owned High-quality API 48134 sessions exited through `Exit`; the port closed.
  External Twin scene source hashes remained unchanged. Other sessions were
  left alone. Skill validation and `git diff --check` pass.

Raw evidence: `target/trail-resize-test.log`,
`target/trail-long-history-test.log`, `target/trail-long-index-test.log`,
`target/trail-kilometre-build.log`, `target/trail-kilometre-gate-output.log`,
`target/trail-kilometre-route-output.log`, `target/trail-long-observations.json`,
`target/trail-long.png` and `target/trail-long-path.png`.

These are bounded visual footprints, not persisted terrain deformation.
Publication can lag by worker/render admission; it does not wait for another
half metre of travel. Generic GPU budget errors remain visible at their owner.
