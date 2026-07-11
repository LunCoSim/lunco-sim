# 47 — Option B Execution Plan (big_space physics/render split)

Implementation plan for the architecture in [46](46-bigspace-deep-analysis.md) §8.2
("Option B": one render world + physics as a site-local coordinate context).
Grounded in three prerequisite audits completed 2026-07-09:

- **avian go/no-go: GO** — solver/narrowphase/broadphase/spatial-query read zero
  GlobalTransforms; avian 0.6.1 runs on `Position`/`Rotation`/`ColliderPosition`
  with `PhysicsTransformConfig { all flags false }`. Caveats: keep colliders
  direct-children-of-body; set Position/Rotation/ColliderScale at spawn.
- **single-cell assumptions: 18 must-fix** (netcode `pos_q` has no cell on wire;
  ~17 sites bypass the cell-aware helpers). Canonical helpers
  `world_position` / `world_vector` / `world_to_grid_local` /
  `world_position_seeded` already exist in `lunco-core/src/coords.rs`.
- **celestial colliders: safe to drop** — bodies are already render-only
  (no `RigidBody`, no consumer queries them; possession raycasts meshes; gravity
  uses `GravityProvider` math; SOI uses the `SOI` radius component).

**Verification rule (every phase):** measure with the simulation clock RUNNING —
the ephemeris epoch gate froze every earlier "stable" measurement. Use screenshot
bursts + pixel-delta at a fixed camera, both at 1× and high warp.

Each phase is independently shippable and testable. Risk gates (where to pause
for a human check) are marked 🛑.

---

## Phase 1 — Standalone fixes (no architectural commitment; lands on current tree)

Wanted by Option B regardless; none require the audits. Order: grid pair first
(establishes the frame structure everything parents into), then line anchor.

**Status (2026-07-09):**
- **1b continuous orbit-line anchor — DONE.** `trajectory_alignment_system` now
  subtracts the tracked body's *current* frame translation from `path.anchor`
  each frame and splits the result through the parent grid, so the curve rides
  the frame continuously (no rebuild-snap). Builds; tests green; visual confirm
  pending a live run.
- **1c spacecraft marker → `translation_to_grid` — DONE** (`missions.rs`).
- **1a grid pair — DEFERRED to Phase 6** (the careful 4-file refactor touching
  the site-pin `stored_in_solar` math; not jitter-killing; folds into the
  Phase 6 cleanup).
- **1d CQ-214 (per-frame mesh re-dirty) — deferred** (perf, not visible jitter).

### 1a. Inertial/rotating **grid pair** per body — `crates/lunco-celestial/src/big_space_setup.rs`, `systems.rs`, `trajectories.rs`
Split each body's single grid into two:
- **Inertial grid** (ephemeris translation only, `body_rotation_system` does NOT
  touch it) — the real "Inertial" frame.
- **Rotating body-fixed child grid** (carries the spin from
  `body_rotation_system`; surface grid + tiles + sites nest under it).

Parent inertial content (trajectory views, satellite orbit frames) to the
inertial grid. **Delete the per-frame counter-rotation** and the
non-anchored-spin-inheritance bug (`trajectories.rs:551`). This is the
correct big_space structure (§5 of doc 46) and removes an entire class of hack.

### 1b. Continuous orbit-line anchor — `crates/lunco-celestial/src/trajectories.rs`
Stitch the tracked body's *current propagated* position into its orbit line each
frame instead of freezing the anchor at `aligned_epoch`. Kills the confirmed
"offset from its orbit unless I scroll away" drift-then-snap. KSA-precedented
(v2025.11.9).

### 1c. Mission marker → `translation_to_grid` — `crates/lunco-celestial/src/missions.rs:262-287`
Route the spacecraft marker through grid placement (currently raw f32 at ~4e8 m).

### 1d. Gate `trajectory_alpha_update_system` — `trajectories.rs:424-478`
Stop re-dirtying every trajectory mesh every frame for a color-only update (CQ-214).

**Verify:** build green; orbit lines stable (clock running); rover drive unaffected.

---

## Phase 2 — Route absolute-position reads through cell-aware helpers (prerequisite: real cells)

Pure refactor — under the current single-cell config every `CellCoord` is zero, so
routing reads through `world_position`/`world_vector` is an **observable no-op**.
This is what makes the Phase 4 cell-flip safe. Targets (from the audit):

- `lunco-avatar/src/lib.rs`: orbit_system GT deltas (~1151-1183), camera-distance
  calcs (2383, 2397, 2658) → `world_position_seeded` / `world_vector`.
- `lunco-sandbox-edit/src/commands.rs:2723` (GT delta) + `gizmo.rs:89/130/183`
  (Transform reads) → `world_vector` + `CellCoord`.
- `lunco-celestial/src/globe_lod.rs:114` (GT delta) → `world_vector`.
- Remove/conditionalize cell-zero resets: `systems.rs:95`, `missions.rs:274`,
  `trajectories.rs:577/590`.

**Verify:** byte-identical behavior vs pre-Phase-2 (screenshot diff at same
clock/camera) — confirms the refactor is truly a no-op while cells are still zero.

🛑 **Risk gate:** confirm no behavior change before proceeding.

---

## Phase 3 — Netcode cell-awareness (prerequisite: real cells; breaks replication otherwise)

The wire format is the hard blocker — `SnapshotEntry.pos_q` ([i32;3] fixed-point)
saturates at ±2147 km and carries no cell.

- Add cell coordinate to `SnapshotEntry` (`sync.rs:78-82`).
- `gather_snapshot` (1482-1496): read CellCoord, not Transform-only.
- Snapshot apply (1070): stop hardcoding `cell: [0;3]`.
- `ViewCenterMsg` (322-325) + `send_view_center_updates` (2018-2032) +
  `recompute_interest` (1703-1712): cell-aware AOI.

(Reference: avatar tutorial streaming already sends `[i64;3]` cell — `sync.rs:272`.)

**Verify:** networking tests + headless dual-client sync round-trip.

🛑 **Risk gate:** netcode round-trip stable before proceeding.

---

## Phase 4 — Real cells (the flip)

Replace `Grid::new(edge, 1.0e30)` with real thresholds across the **whole
celestial tree at once** (`big_space_setup.rs:181/264/277/323/355/397`); place
bodies via `translation_to_grid` so magnitude lives in `CellCoord`. Root
`WorldGrid` (edge 2000, threshold 1e10) already splits correctly.

**Verify (clock running):** the 16 km (EMB-in-Solar) and 32 m (Moon-in-EMB)
re-quantization gone; surface stability preserved; networking still round-trips.

🛑 **Risk gate:** this is the irreversible structural change — confirm with a
live run before Phase 5.

---

## Phase 5 — Option B core: avian bubble bridge

- Set `PhysicsTransformConfig { propagate_before_physics: false,
  transform_to_position: false, position_to_transform: false,
  transform_to_collider_scale: false }`.
- Implement the **Position↔(cell+Transform) bridge**: physics runs in a
  site-local bubble (small f32 `Position`/`Rotation`); bridge syncs to the Moon
  body-fixed grid's `cell + Transform` via `translation_to_grid`. Bubble origin
  = static site anchor, never the camera.
- Set Position/Rotation/ColliderScale at spawn; keep colliders
  direct-children-of-body (audit caveat).
- Move gravity/SOI/focus reads to f64 state (doc 46 P4); `update_local_gravity_
  field` no longer walks the celestial entity chain.

**Verify:** physics bit-identical — replay a recorded drive sequence; rover
dynamics, collisions, wheel raycasts unchanged.

🛑 **Risk gate:** physics behavior must be provably unchanged before Phase 6.

---

## Phase 6 — Traveling origin + unification (retire the pin)

- Let `FloatingOrigin` travel: "focus Earth" = origin transfer (interpolated) in
  the one render world.
- **Delete** `OrbitalViewPin` + its scene-hide machinery, the surface pin's
  astronomical re-pin branch, `OrbitFrameSample`'s celestial branch, hold
  dead-bands, the celestial body `Collider::sphere` (render-only now).
- Surface↔orbital becomes one continuous camera move — the original product goal.

**Verify:** surface→Earth→drag→zoom→Backspace round-trip; no jitter at any zoom;
single origin.

---

## Phase 7 — Measured polish (only if data demands)

- Far-pass split or non-anchored line chunking (doc 46 §4.2) if a single camera's
  depth range can't span rover↔Earth.
- GPU orbit-line generation (KSA direction) if CPU line cost shows up.

---

## Sequencing rationale

Phase 1 is pure win on the current tree. Phases 2→3→4 are a chain: cell-aware
reads → cell-aware netcode → flip cells. Phase 5 (avian bridge) is independent
of the cell flip but should land after, so the bubble bridge composes with real
cells. Phase 6 deletes the band-aid only after 4+5 prove the new model. Each
🛑 gate is a natural pause point.
