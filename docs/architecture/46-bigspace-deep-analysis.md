# 46 — big_space Deep Analysis: Root Cause of Celestial Jitter and the Corrected Architecture

Status: analysis / decision record. Supersedes the interim reasoning in
[45](45-big-space-correct-usage.md) and the view-pin workarounds it motivated.

---

> ## ⚠ Correction (2026-07-10) — this document's diagnosis was NECESSARY BUT NOT SUFFICIENT
>
> Everything below about `switching_threshold = 1e30` storing raw f32 is true, and
> enabling real cells (Phase 4) was required. **It did not stop the jitter**, and
> the reason is not in this document.
>
> `cell_edge_length` is a **precision** knob, not a scale knob.
> `LocalFloatingOrigin::translation` is an **f32** holding the floating origin's
> offset within one cell of that grid, bounded by
> `maximum_distance_from_origin = edge/2 + switching_threshold`.
> `propagate_origin_to_child` rebuilds the origin's position at each level as
> `cells × edge` (exact f64) **plus that f32** — re-splitting at the child cannot
> recover bits the parent already dropped. **The coarsest grid in a chain sets the
> precision floor for its entire subtree.**
>
> Phase 4 gave the Solar Grid `Grid::new(1e9, 1e8)` and the EMB `Grid::new(1e8, 1e7)`.
> Their f32 origin-offsets range to 6e8 m and 6e7 m, where f32 ULP is **64 m** and
> **4 m**. The lunar surface, Earth and the orbit lines all hang below those two
> grids, so all three inherited that floor — one cause, three symptoms. It re-rounds
> whenever the pin slides the tree, i.e. every frame the clock runs.
>
> **Fix:** every celestial grid → `Grid::new(2_000.0, 100.0)`, matching the root
> `WorldGrid`. Cells are `i64`, so small edges are free (1 AU / 2 km ≈ 7.5e7 cells).
>
> Measured in `crates/lunco-celestial/tests/grid_cell_edge_precision.rs` for a point
> **10 m from the camera**: per-frame jitter **4.0000 m → 0.000000000 m**; static
> offset 6.79 m → 45.8 µm.
>
> **Why it hid:** the pin early-returns on a paused clock, so a paused frame never
> re-rounds and is pixel-identical. The "0 changed pixels when paused" evidence used
> throughout this document's era proved nothing — it never exercised the pin. Jitter
> here is epoch-driven; test it with the clock running.
>
> Corollary: `is_local_origin_unchanged` compares that same f32, so with coarse cells
> it only changed in 32–64 m steps and GlobalTransform recomputation was skipped —
> that is why `touch_celestial_transforms` had to exist.



**Verification stamp.** Every claim below was verified against code as it exists —
not against docs, memory, or prior conclusions — across five passes:
(1) big_space 0.12.0 source + examples read in full; (2) exhaustive workspace
inventory of every `Grid::new` / `BigSpace` / `FloatingOrigin` /
`translation_to_grid` / `CellCoord` site; (3) line-by-line trace of the ephemeris
write path and the trajectory/mesh render path; (4) a self-audit that retracted
two of this document's own earlier claims; (5) an external review against field
practice (KSP, Star Citizen, Outerra, Elite, UE5, Godot) and the Kitten Space
Agency devlog record. Claims that were corrected en route are marked; nothing
here is carried forward on trust.

---

## 0. Executive summary

We use `big_space` **inverted**. The crate keeps rendered f32 coordinates small
near the camera by storing positions as an **integer cell + small f32 offset**;
the camera (the `FloatingOrigin`) travels through that partitioned world, and
cells absorb astronomical magnitude. LunCo instead pins the camera at cell
`(0,0,0)`, disables cell splitting on every celestial grid with a
`switching_threshold` of `1e30`, stores body positions as **raw f32 at up to
1.5e11 m**, and re-pins the whole solar tree to fake viewpoint changes.

The consequences are arithmetic, not stylistic:

| Quantity stored as raw f32 | Magnitude | f32 step (ULP) |
|---|---|---|
| EMB grid in Solar grid | ~1.5e11 m | **~16 km** |
| Moon grid in EMB grid | ~3.8e8 m | **~32 m** |
| Earth grid in EMB grid | ~4.7e6 m | ~0.5 m |

Every epoch tick rewrites these raw floats and they snap to the nearest
representable value. **That snapping is the jitter.** Every fix attempted before
this analysis (stored-chain pin, hold dead-bands, frame samplers, world re-pin)
operated *downstream* of the precision loss and could not restore bits f32 had
already discarded — which is why each round "looked better" and then regressed
the moment the clock or camera moved.

A second, independent defect family lives in the trajectory renderer (§4), and a
third in the frame hierarchy itself: the grids named "(Inertial)" **rotate**
(§5).

The fix is a physics/render space separation (§8) — validated line-by-line
against big_space's own solar-system example for the celestial half, and
convergent with how KSP, Star Citizen, and Kitten Space Agency structure the
same problem. The recommended shape ("Option B") keeps **one** render world and
makes physics a site-local coordinate context bridged into it.

---

## 1. Ground truth: how big_space 0.12.0 actually works

Source: `~/.cargo/registry/.../big_space-0.12.0`. Read in full: `grid/mod.rs`,
`grid/cell.rs`, `grid/propagation.rs`, `grid/local_origin.rs`,
`examples/minimal.rs`, `examples/planets.rs`.

### 1.1 Data model

A high-precision entity's truth is **`CellCoord` (i64 x/y/z) + a cell-sized f32
`Transform`**; absolute position = `cell × cell_edge_length + translation`. The
GPU only ever sees the f32 part, kept smaller than one cell, so render precision
is uniform everywhere. With a 10 km cell, worst-case precision is 0.5 mm and i64
reach is ~19.5 million light-years; a ~256 m–1 km cell gives sub-0.1 mm.

### 1.2 The two load-bearing systems

**Placement** — `Grid::translation_to_grid` (`grid/mod.rs:109-133`):

```rust
pub fn new(cell_edge_length: f32, switching_threshold: f32) -> Self {
    // maximum_distance_from_origin = cell_edge_length / 2.0 + switching_threshold
}
pub fn translation_to_grid(&self, input: impl Into<DVec3>) -> (CellCoord, Vec3) {
    if input.abs().max_element() < self.maximum_distance_from_origin as f64 {
        return (CellCoord::default(), input.as_vec3());   // cell 0, RAW f32
    }
    // otherwise: split magnitude into (integer cell, small f32 offset)
}
```

**Recentering** — `recenter_large_transforms` (`grid/cell.rs:82-111`) moves an
entity to a new cell when its f32 translation exceeds the same
`maximum_distance_from_origin`.

`switching_threshold` is a small hysteresis band (the shipped examples use 0).
Setting it to `1e30` makes **both** systems permanent no-ops for any real
position: placement returns raw f32 in cell 0, and recentering never fires.

### 1.3 Propagation is f64, origin-relative

`Grid::global_transform` (`grid/mod.rs:143-166`) composes the chain from the
floating origin to the entity **entirely in f64** — `LocalFloatingOrigin.rotation`
is a `DQuat`, child transforms are promoted via `as_dquat()`, and the single f32
cast happens at the end, on an already origin-relative (small) result. Rendered
`GlobalTransform`s are therefore camera-relative: precise near the origin
regardless of where in the universe the origin is.

Two implications used later: (a) f32 quats in the chain bound errors at the
**angular** level (~1e-7 rad), which is sub-pixel at any zoom (§4.3); (b) an
entity far from the floating origin gets a *large* f32 GlobalTransform — which is
what breaks physics if the origin flies away from the physics scene (§8).

### 1.4 The canonical examples do what we don't

`minimal.rs` places a mesh and the camera at 1e18 m: both via
`translation_to_grid → (cell, offset)`, camera carries `FloatingOrigin` +
`BigSpaceCameraController` and **travels**. Its own comment: *"we want to avoid
using bevy's f32 transforms alone and experience rounding errors. Instead, we use
this helper to convert f64 position into a grid cell and f32 offset."* It also
documents multiple independent BigSpaces ("split screen, or portals").

`planets.rs` is our exact scene (ship on a planet, in a solar system, among
stars): nested grids root → Sun → Earth with **default (small) thresholds**;
Earth placed at 1.496e11 m and the Moon at 3.85e8 m via `translation_to_grid`;
ground objects and the **camera** placed the same way inside Earth's grid;
`FloatingOrigin` rides the camera with a real `cam_cell`; one camera renders
ground-to-stars; lighting reads GlobalTransforms only after
`BigSpaceSystems::PropagateLowPrecision`. There is no world-re-pinning anywhere
in the crate — the concept doesn't exist because the crate solves viewing the
other way (move the origin).

---

## 2. LunCo's actual structure (exhaustive inventory)

### 2.1 Every production grid in the workspace

| Grid | edge, threshold | Site |
|---|---|---|
| **WorldGrid** (root) | **2000 m, 1e10** | `lunco-core/src/world.rs:75` (`WorldGridConfig::default`, not overridden) |
| Solar Grid (Inertial) | 1e9, **1e30** | `big_space_setup.rs:181` |
| EMB Grid (Inertial) | 1e8, **1e30** | `big_space_setup.rs:264` |
| Earth Grid (Inertial) | 1e4, **1e30** | `big_space_setup.rs:277` |
| Moon Grid (Inertial) | 1e4, **1e30** | `big_space_setup.rs:355` |
| Earth/Moon Surface Grids | 1e3, **1e30** | `big_space_setup.rs:323/397` |

Hierarchy: `WorldRoot (BigSpace)` → `WorldGrid` → `Solar` → `EMB` →
`Earth`/`Moon` grids → bodies + surface grids. There are no other production
`Grid::new` calls (only tests use small thresholds).

The root threshold of 1e10 matters: the Solar Grid's site-pin translation
(~1.5e11 m) exceeds it, so **that single placement splits into real 2000 m
cells** — the one place cell-splitting is active, and the reason the surface pin
can hold the site stable at all. Everything *inside* the celestial tree is raw
f32.

### 2.2 The floating origin is pinned by convention

`FloatingOrigin` rides the active camera (or the neutral `OriginAnchor` when no
camera exists). Camera systems *do* call `translation_to_grid` and write cells
(`lunco-avatar/src/lib.rs:948/1069/1372/1546`) — but under thresholds 1e10/1e30
the result is always cell `(0,0,0)`, so the pin is threshold-induced, and
documented rather than enforced (`avatar/lib.rs:1362-1370`: "keeps the camera in
cell (0,0,0)… Do NOT re-split without doing it for the whole world"). Several
sites additionally hard-assume single-cell: networking sync
(`sync.rs:323/1064/1703`, `cell: [0;3]`), `session.rs:550`, and explicit
`CellCoord::default()` resets (`systems.rs:95`, `missions.rs:274`,
`trajectories.rs:577/590`).

### 2.3 The ephemeris write path (verified line-by-line)

`ephemeris_update_system` (`lunco-celestial/src/systems.rs:14-103`):

- All math is f64 (`DVec3` from the provider, through `ecliptic_to_bevy`, into
  `translation_to_grid`), with a **single terminal f32 cast**.
- Each frame-grid receives its position **in its parent grid's frame**, written
  as `CellCoord` + `Transform` — which the `1e30` threshold degrades to raw f32
  in cell 0 (§1.2). Body entities are hard-zeroed on their own grid. The Solar
  Grid (id 10) is skipped so the site pin isn't clobbered.
- The write is **epoch-gated** (`systems.rs:30-33`, `|Δjd| < 1e-9` → return).
  A paused clock freezes the chain — which is why static screenshot bursts kept
  measuring "stable" while the user saw jitter whenever time ran. Test with the
  clock advancing.
- Frames are **consistent**: the provider (`lunco-celestial-ephemeris/src/lib.rs:
  244-281`) returns Earth (399) as `earth − emb` and Moon (301) as
  `(moon geocentric) + (earth − emb)` = Moon−EMB. An earlier draft suspected a
  geocentric/EMB mismatch from telemetry (`Earth 4.5e6` vs `Moon 3.6e8`);
  **refuted** — those are the physically correct EMB-relative offsets (the
  barycenter sits inside Earth; the 81:1 mass ratio makes the magnitudes look
  asymmetric).
- Two systems touch celestial transforms every frame regardless of the epoch
  gate: `body_rotation_system` (unconditional rotation write, §5) and
  `touch_celestial_transforms` (`systems.rs:259-278`, deliberate `set_changed()`
  on all celestial Transforms in site-anchored scenes).

### 2.4 The physics constraint that motivated the pin

avian 0.6.1 copies `Position` from `GlobalTransform` each physics step
(`physics_transform/mod.rs:222`, `position.0 = transform_translation`) after
running bevy's transform propagation in `FixedPostUpdate`
(`propagate_before_physics: true` by default). The `WorldRoot` must carry a
`Transform` for that propagation (removing it drops rovers through the ground;
the resulting compat-pass race is resolved by ordering — see the comment at
`world.rs:100-112`). The *accurate* statement of the constraint (sharpened in
§9-P3): physics entities must stay **near their space's floating origin, with no
origin cell-crossings mid-simulation**. Pinning the origin at the site satisfies
that; pinning it for the entire universe was the over-generalization that froze
the whole architecture into single-cell f32.

---

## 3. Root cause chain — from `1e30` to every reported symptom

| Symptom (user reports) | Mechanism |
|---|---|
| "Lunar surface jitters / blinks" (historic) | Site pin composed from ideal f64 vs the *stored* f32 chain → fixed earlier by the stored-chain pin; residual causes were shadow acne + tile-swap holes (separately fixed) |
| "Earth jitters", "close view of the Moon jitters / bad coordinate system" | §2.3: raw-f32 grid translations re-quantized on every epoch tick (16 km @ EMB-in-Solar, 32 m @ Moon-in-EMB), composing into everything body-anchored |
| "Offset from its orbit unless I scroll away", orbit lurch at warp | §4.1 drift-then-snap: the orbit line's anchor is frozen per rebuild while the body moves continuously |
| "Focused Earth but it shows ground" | §6: the surface scene renders at the origin AND the view pin drags the celestial tree into the same near-origin space; the globe isn't covered by the hide gate |
| "Camera jumps back and forth when Earth focused" (historic) | Mid-frame GT reads + feedback in the target estimator — fixed by First-schedule sampling, but only masked the layer below |

The band-aid history (stored-chain pin → dead-bands → `OrbitFrameSample` →
`OrbitalViewPin` → tracked-frame line parenting) all improved *stability of a
bad coordinate*. None could restore precision destroyed at placement time. The
lesson driving §8: **precision must be created where positions are stored
(integer cells), not recovered at render time.**

---

## 4. The trajectory renderer's own defects (independent of §3)

`trajectories.rs`; verified, including a correction pass on the audit itself.

### 4.1 Drift-then-snap — the visually confirmed mechanism

Anchored orbit views subtract the tracked body's position (frozen at the
rebuild's `aligned_epoch`) from every vertex, and parent the mesh at exact zero
on the body's frame. The frame moves continuously with the ephemeris; the baked
anchor doesn't. The whole curve **translates with the body between rebuilds,
then snaps back** when the async rebuild lands — up to ~0.4–0.5% of the orbit
scale per cycle (Moon view: 0.02 d × orbital speed ≈ 1.8e6 m; Earth view: 0.5 d
≈ 1.3e9 m). At close zoom that is 10–20% of the view: the body sits visibly off
its own orbit line, and the offset shrinks angularly as you zoom out — matching
the report *"it's offset from its orbit, unless I scroll away"* verbatim.

Precedent: Kitten Space Agency hit and shipped exactly this fix — changelog
v2025.11.9 injects the body's *actual propagated position point* into its orbit
line, "this fixes misaligned orbit lines."

### 4.2 Baked f32 vertex quantization — real, but geometry-bounded

Points are f64 (`Vec<DVec3>`) but cast `as_vec3()` at mesh build
(`trajectories.rs:395-407`); one LineStrip spans the whole orbit, so
far-from-anchor vertices carry large quanta (Earth view far side ~3e11 m →
32 km). *Corrected:* a far vertex is also viewed from far — 32 km at 3e11 m
subtends ~1e-7 rad, sub-pixel. Quantization matters only where large-magnitude
vertices pass **close to the camera**: non-anchored paths (mission trajectories
with `anchor = ZERO`, `BodyFixed` views) — e.g. a cislunar arc at ~4e8 m carries
32 m quanta on the segment next to the camera. Anchored views are effectively
safe.

### 4.3 "Per-frame counter-rotation shimmer" — RETRACTED

The audit initially claimed the f32 counter-rotation quat injects r×1e-7 m of
per-frame noise (30 km at Earth's far side). The displacement figure is true but
irrelevant: a rotation error is **angular** (~1e-7 rad ≈ 0.02 arcsec) at any
radius — sub-pixel at every zoom. big_space's f64 propagation (§1.3) bounds it
there. The counter-rotation's real problem is architectural (§5), not numerical.

### 4.4 Adjacent defects

- **Non-anchored inertial views inherit the parent frame's spin**: the
  counter-rotation exists only in the anchored branch (`trajectories.rs:551`);
  the non-anchored branch parents to the rotating reference frame with identity
  rotation — e.g. the relay satellite's inertial orbit line slowly rotates with
  the Moon's 27-day spin. Disappears for free under §5's grid pair.
- **Mission spacecraft markers bypass `translation_to_grid`**
  (`missions.rs:262-287`): raw f32 at ~4e8 m → the marker itself is 32 m-quantized.
- **`trajectory_alpha_update_system` re-dirties every trajectory mesh every
  frame** (colors only; known as CQ-214) — a full mesh re-upload per frame.
- There is **no `KeplerOrbit` ellipse renderer**; satellite entities are placed
  correctly per epoch via `translation_to_grid` (`placement.rs:303-320`).

---

## 5. The frame-hierarchy defect: "(Inertial)" grids rotate

`body_rotation_system` (`systems.rs:111-125`) writes the body's spin quaternion
to the **grid entity** — so "Earth Grid (Inertial)" and "Moon Grid (Inertial)"
are not inertial. Each body has ONE grid serving two incompatible roles:
inertial ephemeris anchor *and* rotating body-fixed frame.

The rotating-grid pattern is legitimate big_space usage **for body-fixed
content** (surfaces, tiles, sites — the docs passage quoted in the code comment
is about exactly that). But it leaves inertial content with no inertial parent:
trajectory lines get parented to the spinning grid and "fixed" with a per-frame
f32 inverse quat (§4.3's hack).

**Correct structure: a grid pair per body** — an inertial grid (ephemeris
translation only, no spin) with a rotating body-fixed child grid (spin; carries
the surface tree). Inertial content parents to the inertial grid; the
counter-rotation deletes entirely; §4.4's spin-inheritance bug disappears.

---

## 6. The "ground in front of Earth" compositing defect

The surface/physics scene renders at the origin. `OrbitalViewPin` *also* drags
the celestial tree into that same near-origin space to fake an orbital
viewpoint. `orbital_pin_scene_visibility` hides tagged `GridAnchor` scene roots,
but the lunar globe is a `GlobeTiles` surface, not a tagged root — so Earth gets
pinned into the sky **and** the lunar ground stays drawn in front. Two correct
scenes composited on top of each other: the visible symptom of never actually
separating the spaces.

---

## 7. How the field solves this (external research, cited)

Every shipping surface-to-orbit engine converges on one pattern: **store/simulate
in f64 or an integer hierarchy; render camera-relative in small f32.**

### 7.1 Numbers everything works around

f32 ULP: ~1 mm at 8 km, ~0.5 m at Earth radius, **32 m at the Earth–Moon
distance, 16 km at 1 AU**. f64 at 1 AU: ~30 µm — geometrically sufficient, but
the GPU pipeline is f32 (WGSL has no f64, Metal has no shader doubles), so f64
alone still needs camera-relative rendering. (O'Neil 2002; Godot's published
precision tables; corroborated by KSA's "shake at ~8 km" figure.)

### 7.2 The comparators

- **KSP**: f64 on-rails universe → SOI-relative origin → Krakensbane (floating
  origin, recenters continuously) → ~2.3 km PhysX bubble (the only full-physics
  region) → ScaledSpace, a render-only **1:6000** proxy (the "6,000,000" in
  circulation is Jool's radius, not the factor). Handoff lesson (HarvesteR,
  "Smooth Transitions", 2015): on a moving-boundary frame switch, solve for the
  exact crossing and re-seed from the interpolated state — the class of our
  one-frame tile-swap bugs.
- **Star Citizen**: 64-bit world + Zone system; each Object Container is a local
  origin; ship interiors are Local Physics Grids (moving reference frames);
  camera-relative render. Cost of 64-bit positioning: "marginal" (Sean Tracy,
  2016) — the GPU never sees the doubles.
- **Outerra**: f64 CPU + per-patch camera-relative rebase + logarithmic depth —
  continuous cm-to-orbit, no scene transitions.
- **Elite Dangerous**: native-double and double-single (two-float) libraries —
  "millimetre precision from inputs of tens of billions of millimetres" (Ross,
  2018).
- **UE5 LWC**: f64 CPU but **integer-tile + local-float on the GPU** — the same
  core idea as big_space.
- **Aevyrie's decision matrix** (big_space docs) rejects periodic recentering,
  camera-relative world-moves, and f64 transforms (GPU still f32; precision
  degrades with distance) in favor of integer cells: uniform precision, absolute
  drift-free coordinates (multiplayer source of truth).
- **Depth**: Bevy already renders reversed-Z with f32 depth (zero-error in
  Reed/NVIDIA's analysis); multi-frustum or log depth only if a single pass
  can't span the range.

### 7.3 Case study — Kitten Space Agency (first-party devlogs, 2024–2026)

The one major post-KSP entrant, unusually transparent (ahwoo.com devlogs,
per-build changelogs, dev forum). Their architecture:

- **f64 sim + camera pinned at zero.** Dean Hall: *"The simulation is powered as
  much as possible by doubles… Rendering is then done with the camera always at
  zero, pushing any floating point issues far out to the edges of the camera
  where they are not perceptible."* Custom f64 math library; camera-relative
  ("Ego") transforms with origin shift; **GPU hardware doubles explicitly
  rejected** (Blackrack) in favor of split-float emulation where needed.
- **Physics = local f32 bubbles with their own floating origins** over the f64
  KEPLER layer: *"Modified flight physics to use a local floating origin. This
  is a vital pre-step to enabling collisions, as most collision physics use
  single precision"* (v2026.5.7). Per-cluster solvers on a common time grid,
  reconciled against on-rails positions. Structurally our split — with the
  bubble origin at the *locale*, not the camera.
- **One render representation, one camera** — sprite→sphere→billboard LOD,
  reversed-Z, forward+; no ScaledSpace analog anywhere in their record. Their
  real-scale Sol pre-alpha ships on this (caveat: real scale is their test
  scaffold; the final game targets a fictional ~KSP-to-2.5× system — but the
  tech demonstrably handles real scale).
- **Orbit lines**: they hit and fixed our §4.1 bug (inject the body's propagated
  position into its line, v2025.11.9); line transparency/darkening moved to GPU;
  culling for "thousands of celestial bodies".
- Patched conics only (n-body ruled out for base game); inter-frame time moved
  f32→f64 after high-warp precision issues (same lesson as our lunco-time rule).

Where KSA differs: **no integer cells** — raw f64 end-to-end. That is
stack-idiom, not a correctness disagreement: they own the whole framework
(BRUTAL, C#/Vulkan, "framework not engine"), so f64 + camera-relative is natural
there. In Bevy, big_space is the idiomatic implementation of the same render-side
idea, composing with the `Transform` ecosystem, with uniform (non-degrading)
precision. Shipping titles exist on both representations; what they share is the
invariant that matters: **the render frame is camera-relative.**

---

## 8. Recommended architecture

### 8.1 Principles (all five passes agree)

1. **Truth is f64 state** (ephemeris, geodesy, orbits). Entity transforms — in
   any space — are projections of it. No physics-relevant system reads another
   space's `GlobalTransform`s.
2. **Celestial world uses big_space as designed**: real thresholds, bodies via
   `translation_to_grid`, magnitude in `CellCoord`, and a `FloatingOrigin` that
   **travels**. "Focus Earth" = fly the origin, never rewrite the world.
3. **Physics lives in a small site-local frame** whose origin never moves and
   never cell-crosses; avian sees only small, stable coordinates.
4. **Rendering is camera-relative** (big_space gives this for free) with
   reversed-Z; one camera until measurements demand a far pass.

### 8.2 Option B — ONE render world, physics as a coordinate context (recommended)

KSA's factoring, translated to our stack:

- **One BigSpace** — the celestial render world, planets.rs-style. No second
  render hierarchy, no second origin: the origin-management systems (§9-P2) need
  no per-space scoping, and no camera-sync machinery between spaces ever exists.
- **Physics runs in a site-local bubble frame.** avian 0.6.1 supports full
  decoupling with stock flags (`PhysicsTransformConfig { transform_to_position:
  false, position_to_transform: false, propagate_before_physics: false }`,
  verified `physics_transform/mod.rs:130-165`). We own one bridge:
  `Position/Rotation` (site-relative, small) ↔ `cell + Transform` under the Moon
  body-fixed grid (exact, via `translation_to_grid`). The bubble origin is the
  site by construction; the camera flies anywhere without touching physics.
- **One camera** renders rover-to-stars (planets.rs and KSA both demonstrate
  this at true scale with reversed-Z). A far pass or scaled proxy is added only
  if depth/perf measurements demand it.

### 8.3 Option A — two BigSpaces (fallback)

A separate small physics BigSpace (origin = site anchor, avian inside) plus the
celestial render BigSpace. Needs: per-space origin management (§9-P2), the local
origin moved off the camera (§9-P3), and pose-sync between the two spaces'
cameras for the composited surface view. Choose only if the Option-B avian
bridge surfaces solver internals that read `GlobalTransform` in ways the config
flags don't gate — audit that first.

### 8.4 Frame hierarchy (either option)

Grid **pair** per body (§5): inertial grid (ephemeris translation only) →
rotating body-fixed child grid (spin; surface tree, sites, tiles). Inertial
content (trajectory lines, satellites' orbit frames) parents to the inertial
grid. `body_rotation_system` writes the child only.

### 8.5 What gets deleted

`OrbitalViewPin` and its scene-hide machinery, the surface pin's astronomical
re-pin branch, the `1e30` thresholds, the trajectory counter-rotation, the hold
dead-bands and `OrbitFrameSample`'s celestial branch — all exist only to prop up
the inverted model.

---

## 9. Risk audit of the plan itself (self-audit findings)

- **P1 — partial cell-splitting is a known failure mode.** An earlier experiment
  gave only the camera real cells and was reverted ("a plane emerges"). That was
  not evidence single-cell is load-bearing — it was evidence the single-cell
  *assumptions* (§2.2 list: networking `cell:[0;3]`, session, GT-delta camera
  paths, explicit cell resets) must be removed **first**, then the whole
  celestial tree flipped **at once**.
- **P2 — three systems assume ONE global `FloatingOrigin`**: recovery
  (`world.rs:238`), window-camera switch (`camera_switch.rs:246-250`), gizmo
  drag (`gizmo.rs:108/230/234`), plus avatar spawns stripping priors. Option B
  keeps one origin and dissolves this; Option A must scope them per-space first.
- **P3 — the physics origin must be the site, not the camera.** Sharpened avian
  constraint: entities near a **never-moving** origin, no cell-crossing
  re-expressions mid-sim. Option B satisfies it by construction.
- **P4 — cross-space reads must become f64 state math.**
  `update_local_gravity_field` walks the celestial entity chain
  (`gravity.rs:103-166`) and already has to freeze while the view pin displaces
  the tree (`gravity.rs:118`) — that hold is this coupling showing as a symptom.
  Gravity, SOI, and focus math move to f64 state. (Comms LOS already complies.)
- **P5 — clicking survives a render-only celestial world**: body selection is
  bevy_picking (mesh-based, `selection.rs:184-237`), not avian raycasts. The
  `Collider::sphere` on bodies (`big_space_setup.rs:219/316/390/504`) needs a
  consumer audit before deletion.
- **P6 — walked-back overclaims**: planets.rs validates the celestial half only
  (single-space, no physics); the split is justified by the avian constraint +
  KSP/SC/KSA patterns. And the scaled-space proxy is deferred — KSA and
  planets.rs both render true scale with one camera and reversed-Z.

---

## 10. Implementation roadmap

Ordered so each phase is independently shippable and testable. **Verification
rule for every phase: measure with the clock RUNNING** (the epoch gate froze
every earlier "stable" measurement) — screenshot bursts at 1× and high warp,
plus the pixel-delta comparison at fixed camera.

**Phase 0 — audits (no behavior change).**
Enumerate and neutralize single-cell assumptions (§9-P1 list). Audit avian
internals for `GlobalTransform` reads not gated by the config flags (Option B
go/no-go). Audit `Collider` consumers on celestial bodies (P5).

**Phase 1 — standalone fixes (land before/independent of the split).**
(a) Continuous trajectory anchor: stitch the body's propagated position into its
orbit line each frame instead of freezing per rebuild — kills the confirmed
"offset from its orbit" lurch (KSA-precedented). (b) Grid pair per body (§8.4):
move `body_rotation_system`'s write to the body-fixed child; re-parent surface
trees; parent trajectories to the inertial grid; delete the counter-rotation.
(c) Route mission markers through `translation_to_grid`. (d) Gate
`trajectory_alpha_update_system` (CQ-214).

**Phase 2 — real cells in the celestial tree.**
After Phase 0: replace every `1e30` with real thresholds, place bodies via
`translation_to_grid`, flip the whole tree in one change. This alone kills the
16 km/32 m re-quantization (§3). The site pin keeps working (it already walks
stored cells + transforms).

**Phase 3 — Option B bubble bridge.**
Disable avian's transform sync; implement the `Position ↔ cell+Transform` bridge
against the Moon body-fixed grid; move gravity/SOI/focus reads to f64 state
(P4). Physics behavior must be bit-identical before/after (replay a drive
sequence).

**Phase 4 — traveling origin + view unification.**
Let the `FloatingOrigin` fly: "focus Earth" becomes an origin transfer
(interpolated or cut) in the one render world. Delete `OrbitalViewPin`, the
astronomical pin branch, scene-hide, dead-bands (§8.5). Surface↔orbital is now
one continuous camera move — the original product goal.

**Phase 5 — measured polish.**
Only if measurements demand: far-pass split or line chunking for non-anchored
trajectories near the camera (§4.2), GPU orbit-line generation (KSA direction),
origin-transfer easing.

---

## 11. Source ledger

**First-hand (this repo / vendored crates):** big_space 0.12.0
(`grid/mod.rs`, `grid/cell.rs`, `grid/propagation.rs`, `grid/local_origin.rs`,
`examples/minimal.rs`, `examples/planets.rs`); avian3d 0.6.1
(`physics_transform/mod.rs`); `lunco-celestial` (`big_space_setup.rs`,
`systems.rs`, `placement.rs`, `trajectories.rs`, `gravity.rs`, `missions.rs`,
`globe_lod.rs`), `lunco-celestial-ephemeris/src/lib.rs`, `lunco-core/src/world.rs`,
`lunco-avatar/src/lib.rs`, `lunco-networking/src/sync.rs`,
`lunco-usd-bevy/src/camera_switch.rs`, `lunco-sandbox-edit/src/{gizmo,selection}.rs`.

**External:** Aevyrie, big_space docs (decision matrix, precision table); Thorne,
GRAPHITE 2005 (floating origin / "spatial jitter"); O'Neil, Game Developer 2002;
Cozzi & Ring, *3D Engine Design for Virtual Globes* 2011 (RTE); Reed/NVIDIA 2015
(reversed-Z); Outerra blog 2009/2012/2013 (log depth, camera-relative); UE5 LWC
docs; Godot large-world docs + GPU-double article; Thall 2006 (df64); KSP wiki
(Krakensbane/Deep Space Kraken), NathanKell RSS wiki (ScaledSpace 1:6000),
HarvesteR "Smooth Transitions" 2015; Tracy/GamersNexus 2016 + Star Citizen wiki
(zones/OCS); Ross/80.lv 2018 (Elite); Romanyuk/SpaceEngine 2017; Fuentes GDC 2022
(MSFS). **KSA (first-party):** Hall FAQ statements (kittenspaceagency.wiki.gg,
Discord-cited), ahwoo.com devlogs — "The End Of The Beginning" (Dec 2025),
"Pre-Alpha Public Release" (Dec 2025), Blackrack "Planetary rings" (Mar 2026),
gravhoek "Celestial Coordinate Frames" (Dec 2025); per-build changelogs
v2025.11.4.2742, v2025.11.9.2894, v2026.3.6.3818, v2026.5.7.4397, v2026.6.8.4680;
Rian Drake forum post on BRUTAL (Dec 2025); Falanghe, Game Developer interview
(Nov 2025); Moluf, space.com interview (Dec 2025).

**Soft/hedged:** Outer Wilds player-at-origin (community analysis, not
first-party); KSA final-scale plans (stated intent, subject to change).
