# Wheel trail contact and terrain review

Reviewed 2026-10-01 in `codex/lunar-soil`.

The contact sampler and sparse terrain annotation renderer own this change.
Avian/mobility provide solved contacts, suspension geometry and wheel dimensions;
`VehicleTrailPlugin` records bounded, disconnected physics-frame history. DEM
collider support resolves through the existing `ColliderTileOf` contract. The
shared terrain fragment renderer owns the ground footprint, including its width
edges, rather than a sampled mesh fitted to the ground. Ordinary static supports
keep their contact-plane ribbons. The canonical contract is in
[USD-driven visuals](../architecture/50-usd-driven-visuals.md#motion-trails-are-bounded-physics-history).

Contact admission runs after current wheel rays in `FixedPostUpdate`, uses the
physics heading without visual wheel spin, and requires real compressed,
load-bearing, static support. Airborne, inverted and unsupported contacts end a
stroke; landing does not join it to the previous stroke. A moving endpoint makes
sub-spacing motion visible without appending history every tick. Full track
width is the realized physical tire width. Paused width edits invalidate
presentation without requiring a physics tick.

Ownership/hook review: these are generic continuous contact, geometry and
presentation mechanisms. Their inputs are existing typed physics facts, their
outputs are bounded render history and annotations, and they are installed in
the shared viewport/recorder visual plugin. They do not add a scenario rule,
terrain query model, USD schema, document writer or interpreted per-contact
loop. Missing contact ends recording; malformed annotation data and unsupported
shaders report the existing terminal publication error. Frame/scene teardown
retires the disposable owner entities.

Streaming annotation sources contain independent segments. The worker reduces
contiguous blocks of at most 64 legs, bounding worst-case simplification work
and preserving turns and contact breaks within one percent of the half-width.
Snapshot sources remain complete. Streaming admission interleaves the newest
legs across sources and stops at budget pressure, retiring older history rather
than introducing holes inside the admitted recent stroke. Candidate growth is
bounded before index allocation. The default shader work remains at most 64
segments per cell and 4096 total. A stable image asset identity permits continuous
uploads without restarting terrain material readiness. The render binder tracks
image descriptors and marks dependent materials for bind-group preparation when
a resize replaces the GPU texture. Equal-descriptor content updates retain the
existing GPU texture and bind group. All declared custom-material image roles
share one dependency reader. This lifecycle mechanism belongs in
`lunco-render-bevy::shader_look`, rather than in route or wheel policy. Source topology,
snapshot revisions, settings and shader changes still fence publication;
continuous revisions coalesce within that generation.

Verification and evidence:

- `cargo build -j 4 -p lunco-luncosim --bin luncosim` passed.
- Two filtered trail-history tests passed with production feature unification
  (`cargo test -j 4 -p lunco-luncosim-edit-ui -p lunco-luncosim --lib ui::trail::tests`):
  moving-endpoint/contact-break behavior and bounded/frame-reset history.
- Four focused `lunco-terrain-surface` annotation tests passed: sparse long
  stroke, invalid/overcrowded snapshot rejection, snapshot/live-head priority
  under pressure, and bounded streaming reduction with bends and gaps.
- Eight straight 1024-leg histories reduced from 8192 source legs to 128 GPU
  segments and a 48 KiB image. One 100-preparation diagnostic averaged
  257.852 microseconds per index build. This is worker preparation evidence
  for that fixture, not an FPS measurement or the curved-path worst case.
- The production `vehicle_trail_contact.rhai` gate passed 15 checks in the
  exact Summer Space School `traverse_apollo15.usda` scene: grounded motion,
  0.30 m physical/render widths for all eight wheels, sub-spacing endpoint
  updates, terrain publication, airborne rejection without history growth,
  actual inverted landing without wheel-track growth, and disconnected
  upright landing.
- In that same session, `route_surface_annotation.rhai` passed six checks,
  including missing-coverage failure and restoration while retained tracks
  share the terrain annotation owner.
- Owned API 48134, High quality, 1280-by-720. Headful captures were inspected
  for grounded tracks, airborne motion, roof landing and the separate landing
  stroke. Raw evidence: `target/trail-gate.log`,
  `target/trail-gate-observations.json`, `target/trail-ground.png`,
  `target/trail-air.png`, `target/trail-roof.png`,
  `target/trail-landing.png`, `target/trail-integrated.png`, and
  `target/trail-annotation-tests.log`. The external Twin scene source hash
  was unchanged. The owned session exited through API `Exit`, and its port
  closed. Other active simulators were left alone.

The rendering product has bounded history; GPU budget pressure can shorten its
oldest retained portion. This is footprint presentation, not tire deformation
or persisted terrain state. Continuous publication can lag by worker/render
admission; it no longer waits for another half metre of vehicle travel.

Integration retained main `d21f193f3`, including its authored axle/steering,
HUD, filtered terrain-detail and shadow changes. The integrated production
build passed, and the final owned High-quality session passed both the
15-check trail gate and six-check route gate. Runtime command documentation
was regenerated from the settled integrated schema without changes.

Sustained-render verification: the binder descriptor-change regression test
passed. The production trail gate now drives for 30 simulation seconds and
requires at least 20 metres of ground travel before its airborne/roof/landing
checks; all 15 checks passed, as did the six route checks. A separate 45-observation
headful drive measured 50.49 metres from its first to last observation. Its
`target/trail-long.png` capture shows continuous tracks reaching both wheel
lanes, instead of a frozen short prefix. The final integrated capture was also
inspected. Both owned API 48134 sessions exited through `Exit`.
Evidence: `target/trail-resize-test.log`, `target/trail-resize-final-build.log`,
`target/trail-resize-gate-output.log`, `target/trail-long-observations.json`.
No extra terrain sampling, history points, fragment budget or per-frame material
rebuild was introduced: dependency scanning/rebinding runs on image descriptor
changes, while equal-descriptor uploads preserve the binding.
