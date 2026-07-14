# 45 — big_space: Contract Audit & Corrective Plan

Status: **analysis / decision record** (2026-07-07). Companion to doc 44; supersedes
its "interim hardening" framing with a precise diagnosis: the jitter/flicker family
is not bad luck or missing workarounds — LunCo violated three load-bearing contracts
of `big_space` 0.12, and every symptom followed from them. Citations are to the
crate source (`big_space-0.12.0`).

> ## Current state of the three violations
>
> | | Status |
> |---|---|
> | **V1** — `Transform` on the `BigSpace` root | **resolved.** The root is `BigSpace + Grid + GlobalTransform` with **no `Transform`** — big_space's canonical root shape. See the correction sections at the end of this doc, and doc [47](47-bigspace-option-b-execution.md) Phase 5/6. |
> | **V2** — cell binning disabled | **resolved.** `WorldGridConfig::switching_threshold` is **`100.0`** (it was `1e10`). |
> | **V3** — per-frame re-posing of the Solar Grid | **resolved.** The floating origin travels with the observer; the world is not re-posed around a point. |
>
> **`switching_threshold` is a PRECISION knob, not an extent knob.** big_space derives
> `maximum_distance_from_origin = cell_edge/2 + switching_threshold`, and
> `translation_to_grid` *short-circuits below it* — returning cell `(0,0,0)` and the
> whole position as a raw **f32** `Transform`. So a large threshold does not "make the
> world bigger": it **disables cell binning outright**, leaving f32 ULP alone to bound
> precision — **32 m at Earth–Moon distance** at the old `1e10`. Cells are `i64`, so a
> small threshold costs nothing (1 AU / 2 km ≈ 7.5×10⁷ cells). The same rule governs
> `cell_edge_length`, and the coarsest grid in a chain sets the precision floor for its
> entire subtree — see doc [46](46-bigspace-deep-analysis.md)'s correction box.
>
> The diagnosis below is kept because it is what a future change would have to
> re-derive to justify raising either knob again. **Do not raise them.**

## 1. The crate's model (what we signed up for)

- **Truth = integer `CellCoord` + small f32 `Transform`** relative to the cell
  center. `GlobalTransform` is the only derived, lossy value: recomputed in
  f64, downcast to f32, **relative to the floating origin's cell** — so it is
  small near the camera by construction.
- **`recenter_large_transforms` is the heart of the crate**: whenever an
  entity's translation exceeds `cell_edge/2 + switching_threshold`, magnitude
  moves into the integer cell and the f32 translation shrinks back to
  cell-size (`grid/cell.rs:80-111`). The docs are explicit: this "prevents
  Transforms from ever becoming larger than a single grid cell and thus
  prevents floating point precision artifacts" (`lib.rs:93-95`).
- `switching_threshold` is a small **hysteresis band** (examples use 0–100 m).
  It does not add precision; it only delays recentering.
- **The root carries `BigSpace + Grid + GlobalTransform` and must NOT have a
  `Transform`** — enforced by the crate's own debug validator
  (`validation.rs:224-238`).
- **`FloatingOrigin` sits on the moving observer** (the camera) in every
  example; the world is *not* re-posed to keep a point of interest at the
  world origin.
- Bodies at astronomical positions are placed by writing `cell + translation`
  (`grid.translation_to_grid(DVec3)`), moved by relative deltas; the shipped
  `BigSpaceCameraController` integrates velocity in f64 and splits deltas into
  cell + f32 every frame (`camera.rs:266-314`).
- Relative poses for gameplay: `CellTransform*` world queries (`b - a`), or
  `Grid::grid_position_double` + the `Grids` system param — **not** ad-hoc
  Transform-chain sums and **not** mid-frame `GlobalTransform` reads. Any
  `GlobalTransform` consumer must run `.after(TransformSystems::Propagate)`.
- Physics: no supported "one global f32 world" mode. The changelog's intent is
  a **32-bit physics sim per partition/grid** inside the big space — i.e. run
  Avian in the local frame of one grid whose contents stay near that grid's
  origin.

## 2. LunCo's three contract violations

| # | Violation | Where | Consequence (observed) |
|---|-----------|-------|------------------------|
| V1 | `Transform` on the `BigSpace` root (added for Avian) | `lunco-core/src/world.rs` | The root matches bevy-compat's *plain* propagation root query (`bevy_compat.rs:11-23`), which then walks the ENTIRE high-precision tree with f32 math, racing `propagate_high_precision` (no mutual ordering in the crate). The whole-frame strobe. Our `configure_sets` ordering makes the race deterministic, but the plain pass still rewrites every GT every frame — wasted work and a standing trap for anything reading GTs mid-frame. |
| V2 | `switching_threshold = 1e10` (WorldGrid — **the historical value; it is `100.0` now**), effectively `∞` elsewhere | `WorldGridConfig` | Recentering never fires; `translation_to_grid` early-returns cell (0,0,0) below 1e10 m. The app is a **raw f32 absolute-coordinate world** wearing big_space as a costume. At 4×10⁸ m the ULP is 32–64 m → orbital-view jitter of camera, lines, and content; at 1e9+ it is worse. The user's diagnosis — "wrong usage of big_space coordinates" — is exactly right. |
| V3 | Per-frame re-posing of the Solar Grid to pin the site at the world origin (doc 43's `anchor_solar_frame_to_site`) | `lunco-celestial/src/placement.rs` | Inverts the crate's model (the floating origin is supposed to ride the camera; the world is not re-posed around a point). Forces `is_local_origin_unchanged = false` → full-subtree GT recompute every frame, creates the transient mixed-convention windows that produced the phantom-target/teleport class of bugs, and required `touch_celestial_transforms` + ordering hacks to survive. |

Secondary effects of V2: because the app has *never* run with a moving origin
cell, code and content accumulated origin-absolute assumptions. Splitting just
the orbit camera into real cells (tried 2026-07-07) immediately exposed them —
plain-propagated geometry (the scene `Ground` cube) rendered in the wrong
convention whenever the camera cell ≠ 0 ("a plane emerges").

Not a big_space issue, but found in the same investigation:
`lunco_core::coords::world_position_seeded` sums nested grid translations
**without grid rotations** — under the site-anchored (rotated) Solar Grid it
resolves positions in a rotated-away direction. The crate-native replacements
are `CellTransform*` / `grid_position_double` / `Grids::parent_grid`, or a
same-instant `GlobalTransform` **delta** (origin cancels; its *length* is
convention-independent).

## 2.1 The Avian constraint (verified in avian3d 0.6.1 source)

`PhysicsTransformPlugin` runs Bevy's own `mark_dirty_trees →
propagate_parent_transforms → sync_simple_transforms` **inside the physics
schedule** (`propagate_before_physics: true`), then `transform_to_position`
copies `Position = GlobalTransform.translation` for every body the user
didn't move (`physics_transform/mod.rs:95-110, 187-235`). Two hard
consequences:

- **Avian needs a propagation root WITH a `Transform`** to reach our tree in
  its pre-step pass — that is the real reason `WorldRoot` carries one (and
  with it, violation V1). Removing it without restructuring starves physics
  of fresh `GlobalTransform`s (bodies free-fell in the 2026-07-07 bisect).
- **Avian's `Position` is `GlobalTransform`-derived, and big_space `GT`s are
  floating-origin-relative.** If the floating origin ever changes cells, every
  GT shifts by the cell delta and Avian re-reads all bodies as "teleported"
  into observer-relative coordinates. Physics is only correct while the
  origin's cell NEVER changes.

So the single-cell convention (V2) is not an accident — it is **load-bearing
for physics**. A bare `switching_threshold` flip is UNSAFE: the moment the
camera-origin recenters, physics breaks. Enabling real cells requires first
decoupling physics from observer-relative `GlobalTransform`s.

## 3. Corrective architecture

With 2.1 established, the correct target is doc 44's **two-space split**, for
which big_space has native support: *"Your world can have multiple BigSpaces,
and they will remain completely independent. Each big space uses the floating
origin contained within it"* (`floating_origins.rs:30-34`).

1. **Local space** (physics + scene): its floating origin stays pinned at the
   site anchor — GTs are site-frame-stable forever, so Avian is correct *by
   construction*, and the root-Transform hack can be retired by giving the
   local tree its own conventional root. Everything here is metre-scale;
   single-cell is fine and intended.
2. **Celestial space** (sky + orbital view): separate rendering pass — a
   second `BigSpace` whose floating origin rides the orbital camera, or doc
   44's normalized celestial sphere (no large coordinate ever reaches an f32).
   Surface⇄orbital is the camera mode switch of doc 44 §2.4.
3. **Crate-native relative math** everywhere: retire
   `world_position_seeded`; use `CellTransform*` / `grid_position_double` /
   `Grids`, GT deltas sampled in `First`, or the future `CelestialSnapshot`.
4. **Validator on** (`BigSpaceValidationPlugin`, debug builds) once each
   space's hierarchy is canonical — the regression guard for all of this.

Interim invariants that hold the current single-space world together (do not
break in review): the floating origin's cell must stay (0,0,0) — no entity
may split the camera or scene content into cells; the compat→hp→low
propagation ordering (`WorldShellPlugin::configure_sets`) stands as long as
the root carries a `Transform`; f32 vertex/transform magnitudes above ~1e6 m
in anything the camera can approach must be cell-anchored (see the trajectory
anchor pattern in `trajectories.rs`).

## 4. Issues that are NOT big_space (verified separately)

- **Trajectory/orbit polylines** (`lunco-celestial/src/trajectories.rs`) are
  single `LineStrip` meshes with f32 vertices up to 4×10⁸ m under the Earth
  frame: model-view cancellation ≈ 64 m of per-frame wobble, visible only at
  close range ("orbits flicker", "moon offset from its orbit"). Fix: chunk the
  polyline into cell-anchored segments (vertices local to a `CellCoord`d
  chunk origin), or move lines to the doc-44 celestial render rig.
- **`comms_demo_test.usda` has a 400×400 m flat `Ground` cube at y=0** — it
  z-fights/occludes the DEM (georeferenced to the same height) and *is* the
  flat gray "lunar surface" seen in recent captures. The DEM makes it
  redundant; it should be removed (or sunk) once rover spawn placement on the
  DEM colliders is re-verified.

## ⚠ Correction (2026-07-10, Phase 6 landing): ordering did NOT fully resolve V1

§2's claim that the V1 race is "resolved by ordering" holds only for
GlobalTransforms the high-precision pass **also writes**. Verified in
big_space 0.12 source (`grid/propagation.rs`), HP propagation never writes:

1. **a root's GT unless `Grid` and `BigSpace` are on the SAME entity**
   (the root branch queries `(&Grid, &mut GlobalTransform), With<BigSpace>`), and
2. **a cell-entity whose direct parent is not a `Grid`**
   (the cell branch does `grids.get(parent.parent())` and silently skips).

Our shell split them — `WorldRoot` = `BigSpace` only, `WorldGrid` = `Grid`
only — so **both the root's and the `WorldGrid`'s GlobalTransforms were
written exclusively by the f32 compat pass, as identity, forever.** That is
accidentally correct while the floating origin's cell is (0,0,0) — the
world-pin era, and §3's "interim invariant" above — and wrong by the full
camera distance once the origin travels (Phase 6 orbital view): every
Transform-only entity composing off the root/`WorldGrid` renders in surface
convention and stands still while the HP-owned world moves — "planets jump
around when I rotate". The §3 interim invariant (origin cell must stay 0) is
hereby RETIRED: with the fix below, the origin may travel.

**Fix (landed):** `WorldRoot` now carries `Grid::new(cell_edge, 100.0)`
alongside `BigSpace` (`lunco-core/src/world.rs`), making it a legal big_space
root; `WorldGrid` becomes an ordinary cell-entity under a real grid, so HP
owns both GTs. The Avian root `Transform` stays (compat still re-walks the
tree, but now loses everywhere by ordering). Regression test:
`lunco-core/tests/world_shell_origin_tracking.rs`.

Class 2 remains for `BodyFixed` trajectory views parented to body entities
(today only the invisible `Artemis 2 Moon-Relative`): a cell-entity under a
non-grid parent is rendered by the compat pass only. Any future visible
BodyFixed view must parent to a grid, or drop its `CellCoord`.

## ⚠ Addendum (2026-07-11): the strobe's writer is AVIAN's propagation, not (only) the compat pass

The measured "~1 frame in 5–9 renders plain-f32 GTs for anything
`touch_celestial_transforms` doesn't force-dirty" (3 385 jump events/15 s
when the touch list was deleted) finally has a mechanism. There are THREE
plain-f32 whole-tree GlobalTransform writers in the app:

1. big_space's bevy-compat `propagate_parent_transforms` (PostUpdate) —
   ordered before `PropagateHighPrecision` by `WorldShellPlugin` ✓;
2. **avian's `propagate_before_physics`** (`avian3d
   physics_transform/mod.rs:99-104`, default ON) — registers *bevy's own*
   `propagate_parent_transforms` + `sync_simple_transforms` **inside the
   `PhysicsSchedule`**, i.e. it rewrites every GT reachable from the
   root-`Transform` `WorldRoot` in absolute convention on every 60 Hz physics
   tick. In PostUpdate, big_space's change-gated HP pass rewrites only
   changed/origin-moved entities and SKIPS the rest — so on tick frames
   avian's plain values survive to the renderer. 60 Hz vs a ~300 fps
   uncapped renderer = 1 frame in ~5. **This system was never ordered
   against big_space and cannot be — it runs in a different schedule.**
3. `sync_simple_transforms` (both registrations) — parentless entities only.

Corollaries:
- Physics is CORRECT under the traveling origin *because of* writer 2:
  inside the `PhysicsSchedule`, GTs are avian-propagated absolute, so
  `transform_to_position` (which reads `GlobalTransform`,
  `physics_transform/mod.rs:188`) never sees big_space's origin-relative
  values. Do NOT flip `propagate_before_physics: false` in isolation —
  physics would then read origin-relative render GTs and break.
- `touch_celestial_transforms` is the de-facto reconciler between writer 2
  and the HP pass; it stays until physics gets its own transform domain.
- This is the precise, mechanical statement of why Phase 5 (doc 47) /
  the §3 two-space split is the real fix: physics must stop sharing
  `GlobalTransform` with the render world.

### Phase 5 landed (2026-07-11): the physics transform domain

`BigSpacePhysicsBridgePlugin` (`lunco-usd-avian/src/big_space_bridge.rs`,
registered in the sandbox after `PhysicsPlugins`) disables all three of
avian's f32 sync systems — `propagate_before_physics` (writer 2 above),
`transform_to_position`, `position_to_transform`; all runtime `run_if`
gates on `PhysicsTransformConfig` — and owns the sync in the f64 cell
chain:

- **READ** (`pose_to_position`, Prepare): a body re-reads
  `Position`/`Rotation` from `world_pose_seeded` ONLY when its own
  `(CellCoord, Transform)` differs from the `BridgeShadow` copy captured at
  the bridge's last write — i.e. when an external writer (spawn, teleport,
  gizmo, USD animation, anchor system, big_space recentring) moved it. A
  fired body re-reads all descendant bodies too (chassis teleport carries
  jointed wheels). Standalone no-body colliders (sensor zones) are covered;
  body-attached child colliders keep avian's `ColliderTransform` path.
- **WRITEBACK** (`position_to_pose`, Writeback): Dynamic bodies only; the
  solved world pose is written to `Transform` relative to the parent frame
  (nearest ancestor body's fresh solve, else the ancestor grid's chain
  pose) and the CURRENT cell. Cells are never written — big_space's
  `recenter_large_transforms` owns the re-split, which round-trips through
  the READ rule. Jointed sub-bodies without `CellCoord` (rover wheels) get
  their local transform against the chassis' solved pose.
- The 2026-07-09 `narrow_phase` island panic (`islands/mod.rs:547`) was the
  FIRST bridge dirtying every static's `Position` every tick — whole-world
  contact churn corrupted avian's island bookkeeping. The shadow gate is
  the fix: statics at rest are never touched.
- `Position` is now the **BigSpace root frame** (absolute), not the
  collapsed plain-propagation frame: cell offsets are honoured (a body >1
  cell from the site no longer collapses onto it) and physics is
  magnitude-proof (integration-tested settling at 2e8 m with cell-local
  `Transform`s). Consumers that compared avian `Position` against render
  `GlobalTransform` coincide only near the floating origin — same caveat
  as before, now stated.
- With writer 2 gone, render GTs are big_space-owned exclusively; the
  strobe writer no longer exists. `touch_celestial_transforms` stays until
  its removal is re-measured under the bridge (`LUNCO_JUMP_PROBE=1`) — do
  not delete the two together.

### Canonicalization follow-up (same day) — LANDED on the second attempt

- **First attempt (reverted within the hour):** removing WorldRoot's
  `Transform` sank every live rover at damping-terminal ~17 m/s. Avian's
  `propagate_collider_transforms` (ColliderTransformPlugin — NOT among the
  three syncs Phase 5 disables) only descends from tree roots WITH a
  `Transform`; with it frozen, `update_collider_scale`'s child branch read a
  stale `ColliderTransform`, and the sandbox Ground — a UNIT cube with
  `xformOp:scale = (4000, 0.2, 4000)` — collapsed to a ~1 m collider. The
  camera-relative jump probe stayed SILENT throughout (the chase cam falls
  with its rover): co-falling is invisible to relative probes; absolute API
  position checks caught it.
- **Second attempt (landed):** the bridge now owns collider transforms.
  `propagate_collider_transforms_rootless` (bridge module, same
  `PhysicsTransformSystems::Propagate` set) recomputes every collider's
  `ColliderTransform` from its `ColliderOf` chain — plain nodes compose
  translation/rotation/scale, rigid-body nodes reset translation/rotation
  and keep the running scale, faithful to avian's recursion — with NO tree
  root involved. avian's own pass still runs and no-ops on rootless trees.
  With that in place: **WorldRoot is `Transform`-free (big_space-canonical)
  and `BigSpaceValidationPlugin` is re-enabled** in sandbox debug builds.
  Pinned by `bridge_physics.rs::
  scaled_child_collider_ground_settles_without_root_transform` and the
  structural ABSENCE assert in `world_shell_origin_tracking.rs`.
  Consequence: any app that spawns the world shell AND avian physics must
  register `BigSpacePhysicsBridgePlugin` (the sandbox does; `luncosim` has
  no physics content).
- **Trajectory views only carry `CellCoord` under Grid parents**
  (`trajectory_alignment_system`): views spawn cell-less; the alignment
  system inserts the cell when parenting to a grid and removes it when
  falling back to a plain body entity (the last known class-2 violation —
  "Artemis 2 Moon-Relative"). A cell-entity under a non-grid parent is
  invalid; a plain `Transform` child there is the correct, compat-propagated
  form. This part LANDED.
