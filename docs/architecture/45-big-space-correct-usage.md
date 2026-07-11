# 45 — big_space: Contract Audit & Corrective Plan

Status: **analysis** (2026-07-07). Companion to doc 44; supersedes its "interim
hardening" framing with a precise diagnosis: the jitter/flicker family is not
bad luck or missing workarounds — LunCo violates three load-bearing contracts
of `big_space` 0.12, and every symptom follows from them. Citations are to the
crate source (`big_space-0.12.0`).

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
| V2 | `switching_threshold = 1e10` (WorldGrid), effectively `∞` elsewhere | `WorldGridConfig` | Recentering never fires; `translation_to_grid` early-returns cell (0,0,0) below 1e10 m. The app is a **raw f32 absolute-coordinate world** wearing big_space as a costume. At 4×10⁸ m the ULP is 32–64 m → orbital-view jitter of camera, lines, and content; at 1e9+ it is worse. The user's diagnosis — "wrong usage of big_space coordinates" — is exactly right. |
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
