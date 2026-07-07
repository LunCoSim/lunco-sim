# 44 — Surface & Orbital Views: Local Space / Celestial Space Split

Status: **proposed** (target architecture). Doc 43's site-anchored solar
hierarchy is the as-built baseline; this doc explains why that design is
structurally fragile and specifies the replacement.

## 1. The as-built design and its systemic failure mode

Doc 43 renders the sky by making the ENTIRE solar hierarchy a rigid subtree of
the scene's `WorldGrid`, re-pinned every epoch tick so the geodetic site
coincides with the scene origin (`anchor_solar_frame_to_site`). Scene content
and physics stay near the origin; Earth/Sun stand in the correct sky.

That single decision — *a 10¹¹-metre transform subtree, re-posed every tick,
inside the same tree every gameplay system walks* — produced an entire class
of bugs, each fixed individually but all sharing one root:

| Symptom | Mechanism |
|---|---|
| Whole-frame white/black strobe | two `GlobalTransform` propagators raced over the re-pinned subtree |
| Ground pitch-black | the Sun body mesh (on the light axis) pancaked into every shadow cascade |
| Surface jitter | gravity field read transforms **mid-chain**, between the ephemeris write and the anchor re-pin |
| Earth blinking / missing | change-gated GT propagation froze tiles at the pose of their unlucky spawn frame |
| Click Earth → teleported 10¹¹ m | a command observer mixed two GT conventions captured mid-propagation |

The invariant that keeps a unified tree correct is brutal: **no system may
read two spatially-related transforms at different points of the frame, and
no transient multi-writer state may ever be observable.** Bevy's parallel
executor, change detection, command observers, and third-party plugins
(Avian, big_space's compat pass) all violate this by default. Every new
consumer of celestial state re-exposes the disease.

## 2. Target architecture: two spaces, one bridge

Split what is *simulated and touched* from what is only *seen at a distance* —
the classic local-space/scaled-space split (KSP, Star Citizen, Outer Wilds all
converge here).

### 2.1 Local Space (unchanged)

`WorldRoot → WorldGrid` — scene content, terrain patch, physics, avatar,
FloatingOrigin. Metres-scale f32. **Nothing in local space is re-posed by the
epoch tick.** Avian sees a conventional transform tree (root keeps its
`Transform`). The DEM terrain within the streaming radius (~10–100 km) lives
here, georeferenced to the site once at load.

### 2.2 Celestial Space (new)

Everything beyond local range — Earth, Sun, Moon globe (far side), satellites,
trajectories — leaves the `WorldGrid` tree entirely and renders through a
dedicated **celestial pass**:

- A separate `RenderLayer` + camera that **only rotates** with the main
  camera (never translates). Depth-wise it draws behind local space.
- Bodies are placed on a **normalized celestial sphere**: direction from the
  observer (f64, straight from the ephemeris + site frame) × a fixed rig
  radius (e.g. 10⁵ m), with radii scaled to preserve angular size. For a
  surface observer the parallax error vs. true placement is exactly zero for
  the Sun and < 0.01 px for Earth: direction and angular size are the *only*
  observables at those distances.
- Positions are recomputed **every frame from the ephemeris as pure math**
  (`CelestialSnapshot` resource, one writer, f64). No 10¹¹-magnitude value
  ever enters a `Transform`, so there is nothing for propagation, physics,
  or observers to race over — the entire bug class of §1 becomes
  *unrepresentable*.

### 2.3 The snapshot is the single source of truth

```rust
/// One writer (PreUpdate, in CelestialEpochSet). Everyone else reads this —
/// never the transform tree.
struct CelestialSnapshot {
    epoch_jd: f64,
    /// Per body: observer-relative direction (unit, f64), true distance (m),
    /// body radius (m), body orientation (DQuat).
    bodies: Vec<BodyState>,
    /// Site frame: geodetic origin, ENU basis in ecliptic axes.
    site: SiteFrame,
}
```

Gravity, comms LOS, focus commands, sun-light steering, the celestial render
rig, shader params — all consume the snapshot. A consumer can run in any
schedule at any time: the snapshot is immutable for the frame. (This
formalizes what the Jul-07 fixes did ad hoc with `CelestialEpochSet`
ordering and the never-zero Solar Grid rule.)

### 2.4 Orbital view = swap which space is primary

Surface⇄orbital is an explicit **mode switch**, not a continuous crawl of one
camera across 8 orders of magnitude:

- **Surface mode**: camera in local space; celestial pass draws the sky.
- **Orbital mode** (focus a body / zoom past ~100 km): the camera *becomes*
  a celestial-space camera. Local space collapses to a site marker on the
  body. Bodies use real (scaled) geometry: globe LOD tiles keyed to the
  celestial rig, at rig-scale coordinates (10⁵–10⁷ m — comfortably f32).
- The **transition** is a camera crossfade at the threshold altitude, driven
  by one system that owns both cameras (extends `reconcile_scene_viewport`'s
  single-authority pattern). Focus commands only ever say "make body X the
  view target at distance D" — spatial math happens inside the owning system
  at a fixed schedule point (the `PendingFocus`/First-schedule pattern,
  already shipped).

### 2.5 What big_space is still for

big_space keeps earning its place in local space (large local worlds, rover
traverses, multi-kilometre bases) and for **true orbital flight** of piloted
vessels around one body (vessel physics in that body's grid). What it stops
doing is *carrying the whole solar system as one rigid pose tree*.

## 3. Interim hardening rules (as-built tree, shipped Jul 07)

Until the split lands, the unified tree survives only under these invariants —
enforce them in review:

1. **The Solar Grid has exactly one writer** (`anchor_solar_frame_to_site`).
   `ephemeris_update_system` must never touch id 10 — a transient un-anchored
   pose, even mid-chain, IS observable by parallel systems.
2. Everything reading celestial transforms in `PreUpdate` orders
   `.after(CelestialEpochSet)`.
3. Celestial `Transform`s are change-touched every frame
   (`touch_celestial_transforms`) so change-gated propagation can never
   serve a stale pose.
4. big_space's high-precision propagation is explicitly ordered after the
   bevy-compat plain pass (`WorldShellPlugin::configure_sets`); the root
   keeps its `Transform` (Avian requires the standard convention).
5. **No spatial math in command observers.** Observers record intent;
   a system at a fixed schedule point (First) applies it on
   frame-consistent transforms, and always via same-instant GT *deltas*.
6. Distances/positions for UI/API (`QueryEntity`) are heliocentric-absolute;
   scene-frame consumers must convert via the site frame, never mix.

## 4. Migration slices

1. **`CelestialSnapshot` resource** — add the single writer; port gravity,
   sun steering, comms LOS, and `apply_pending_focus` to read it. Transform
   tree becomes render-only. (Small, immediately de-risks new consumers.)
2. **Celestial render rig** — normalized-sphere pass for Sun/Earth/satellites
   in surface mode; remove those bodies from the WorldGrid subtree; delete
   the per-tick Solar Grid re-pin (the site frame lives in the snapshot).
3. **Orbital mode** — celestial-rig camera + crossfade; globe LOD keyed to
   the rig; `FocusEntityById` targets rig coordinates.
4. **Cleanup** — retire `touch_celestial_transforms` and the compat-ordering
   workaround (nothing at 10¹¹ m remains in the tree to protect).

## 5. Regression scenes & tests

- `scenes/test/site_anchor_minimal.usda` — a bare pad + site anchor + epoch,
  no vehicles: the smallest scene that exercises anchoring, lighting, and the
  celestial pass. Screenshot-burst byte-uniformity = the flicker regression
  test (the Jul-07 method, automatable via `--api` + `CaptureScreenshot`).
- Headless assertion: Earth body pose stable (< 1 px angular drift) across
  200 frames in a site-anchored scene; focus command lands the camera within
  `distance ± 1%` of the target.
- Keep `shackleton_sun_stays_grazing_and_gets_lit_epochs`
  (lunco-celestial-ephemeris) as the frame-convention canary.
