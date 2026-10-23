# Planetary flight and contact ownership

## Failure and owners

A recorded tutorials Griffin flight reached y=1055.283365 m while its resident
static geometry implied a WorldBounds upper limit of 1054.730048 m. The
`finite_world_exit` policy paused all 20 bodies of the articulated object. The
simulation clock continued. Static collider residency is not a planetary flight
domain. BigSpace already recenters Grid-direct entities; the native bridge
preserves the f64 physics pose through that representation change.

`lunco-physics` owns the generic unbounded-domain declaration and escape facts.
Celestial placement applies it to a valid projected site grid and withdraws it
with the site anchor. The existing Rhai escape policy still owns containment
choices for actual finite-world exits and non-finite state. Backend numeric
admission remains independent and required.

`lunco-terrain-globe` owns one f64 surface geometry producer and balanced
cube-sphere selection. `lunco-celestial-spatial` consumes it for native contact
and rendering, using the existing body-fixed grid, GravityBody association,
GlobeHandoff and native Avian collider constructor. There is no duplicate sphere
formula, alternate physics world, point clamp or dependency change. The existing
DEM ring owns the crop interior. The celestial picking sphere stays pick-only.

## Preparation and lifecycle

Physical demand comes from seeded dynamic bodies, independently of cameras.
Current and velocity-projected surface foci refine a complete balanced cover.
Bounded asynchronous cooks publish in source-key order. New support must have
valid native AABBs before obsolete support retires. Near contact, the existing
PhysicsHolds and typed SimulationProgress mechanisms admit preparation without
advancing unsupported physics. Invalid settings/cooks fault visibly. Generated
entities are SceneOwned; the scene resource retires tasks and cached leases,
and SceneTeardown releases the progress key.

The geometry spine retains f64 positions/normals. Its explicit GPU adapter
narrows only rendered attributes. The shared handoff retains native collar
geometry as an Arc and derives the cached render mesh from it. Contact uses the
same below-datum surface and crop cutout rather than a mean-radius sphere that
would obstruct negative-elevation terrain.

## Hook candidacy

- Owner/input/output: physics reads the active grid's typed domain declaration
  to determine whether finite static bounds apply. Celestial projection derives
  that fact from a valid SiteAnchor. Continuous contact selection/cooking reads
  native poses, gravity association and shared surface geometry and produces
  native static colliders.
- Scope/lifecycle: installed with the existing physics/celestial plugins;
  domain declaration follows site lifetime, generated surface state follows the
  scene lifetime.
- Policy: the existing required `physics.body_escape` Rhai hook retains its
  policy decisions. Numeric geometry, frame conversion, contact support and
  admission are shared engine invariants and do not need a second policy hook.
- Failure semantics: missing support retains a preparation hold; invalid native
  geometry raises RuntimeFaults. No timeout fallback admits unsupported flight.
- Production test: `planetary_flight_surface` observes attached flight across
  cells, off-crop swept contact, below-datum local contact and collar contact.
  `escape_containment` retains ordinary bounded-scene policy coverage.

## Verification boundary

The previous regular production binary emits FAIL for the new authored fixture:
its collar drop penetrates y=-520 m while the off-crop body remains paused near
its spawn position. Native document operations authored and saved the USD
fixture; the document inspection reported a clean saved root layer.

The generic gate drives launch velocity in the admitted native frame, matching
operator input after loading. A separate authored-initial-velocity admission
error is reported to optimization: a pre-mount WorldGrid pose seed can defer
its velocity rebase until after the epoch-dependent site mount and rotate the
source velocity into the wrong axes. That owner repair is outside this surface
change; the gate does not certify authored nonzero initial velocity.

A High quality at 1280 x 720 full Griffin manual flight reached 28.37 km and cell Y=14 without
an escape pause on production `38e24da5b488`. A roughly 300 m/s engine-off return
with deployed ramps contacted the off-crop globe but later diverged in its
articulated state; this is recorded for optimization and is not a landing pass.

Final production `e5b29513a72f` passes the 14-check authored gate at 60 Hz,
one Compute thread and zero jitter: peak 3169.200870 m; cells Y=0..2;
attachment error <=1.20e-7 m; final primary Y=-499.000005 m. Off-crop and collar
sphere-centre heights are -499.000183 m and -498.997997 m over the -500 m datum.
Normal contact speeds remain below 4e-5 m/s while tangential rolling is allowed.
The same fixture fails on the previous regular `f40cbd06c9c2` binary.

The existing bounded `escape_containment` gate passes 5 checks. Focused Rust
checks pass: 16 shared-geometry/coverage tests, one unbounded-domain test
(including non-finite state and domain withdrawal) and one site-grid lifetime
test. The final default production build succeeds; its existing
`UsdStageProjectionPlan` unused-import warning is outside the changed owners.

A High-quality 1280 x 720 production capture confirms the globe surrounds the
local terrain when the bound scene camera renders to an image. Globe LOD reads
the presentation binding independently of target storage; rejecting image
outputs had omitted the Moon in offscreen captures. The native contact cover
remains independent of that camera. This run does not qualify the separate
60-FPS mission acceptance or the high-energy articulated-impact case.

The owned final rendered session also completed a full `RestartScene` cycle:
readiness returned to ready with no hold, fault or pending participant, and
`GRIFFIN_SURFACE_OPS` emitted PASS for all 16 checks at 101.366667 scene seconds.
The lander's native physics remained admitted. The task-owned API 4393 session
was exited after capturing this receipt. Screenshots, build/test logs and source
operation receipts remain in the task's ignored evidence directories.
