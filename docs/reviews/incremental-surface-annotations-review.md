# Incremental terrain annotations review

The mutable index belongs to `lunco-terrain-surface`; segment identity belongs
to each `SurfaceCurveAnnotation` producer. A terrain has fixed DEM-local bounds,
stable record addresses and reusable adaptive-node/reference blocks. This avoids
moving coordinate bounds and global record relocation during ordinary edits.
The canonical contract is in
[USD-driven visuals](../architecture/50-usd-driven-visuals.md).

DRY discovery found the existing annotation owner, shared terrain shader,
Bevy image lifecycle and renderer texture dependency reader. The change extends
those owners. `lunco-materials::float_texture` supplies the missing render-free
typed partial-upload seam; `lunco-render-bevy` consumes it through wgpu row
writes. Image capacity growth uses the shared descriptor/rebinding path, which emits
an explicit dependent-material asset modification event. The low-level seam
test checks that event, including its absence for equal-descriptor content.
There is no new terrain sampler, physics model or authored schema.

Each worker exclusively owns one terrain index, applies source/segment deltas
in deterministic key order and retains an undo journal for touched state.
It prepares both partial texels and complete capacity-growth bytes off-thread.
Admission is globally bounded to two workers, serial within each terrain.
Generation checks control presentation publication; worker completion does not
change simulation state. Invalid operations roll back and report terminal owner
errors. Last-source removal and `SceneTeardown` cancel outstanding work and
retire pending uploads. A repaired input can explicitly re-admit canonical state.

Hook candidacy: these are hot-path spatial math, allocation invariants and typed
render-resource application. Inputs are f64 segment deltas, style, terrain extent
and bounded settings; outputs are transactional index patches and upload work
counters. Installation is the generic terrain/renderer pair, with no required
Rhai decision hook. Product route policy remains in Rhai. Production Rhai gates
exercise the consumer behavior, including invalid coverage and recovery.

Verification on the production High-quality Apollo 15 scene:

- The shared-feature focused mechanism command passes both named
  `surface_annotation_` tests: 10000-segment local edits, rollback, recycled
  nodes/records and density rejection; and real dependent-material asset events
  for descriptor changes, with no event for ordinary content.
- `vehicle_trail_contact.rhai` passes 17 checks, including 30 simulation seconds
  of driving, bounded producer deltas, retained publication, physical tire width,
  airborne/inverted rejection and disconnected landing.
- `route_surface_annotation.rhai` passes 12 checks, including all 2002 retained
  legs, an edit that updates exactly 4 segments and 31 nodes with 2432 bytes of
  source work, unchanged full-upload count and texture identity, and a render
  acknowledgement at or beyond that source's upload sequence. Invalid coverage
  fails visibly and the original route and writer state restore.
- Rendered captures show the loop/connector and continuous wheel-width tracks.
  Owned API 48281 was closed through `Exit`, with process and port release.
- Production build, skill catalogue validation and diff whitespace checks pass.

Artifacts are under `.cache/terrain-incremental-evidence/`: mechanism/build logs,
`final-runtime-validation.log`, `incremental-runtime.log`, schema and rendered
captures. No FPS result is inferred from compile checks or bounded-work counts.
