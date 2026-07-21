# 43 — Orbital view: satellites, ground stations & the site frame

Status: design + implementation · Audience: engine + content authors

The celestial placement substrate: a Keplerian satellite, geodetically-placed
ground stations (Earth + Moon), the **site frame** that grounds scene-local prims
on a body, and a sandbox that can switch between the surface (moonbase terrain)
and a solar-system view of the same scene.

**Connectivity is not in this doc.** Links are a generic, domain-free kernel over
this geometry — see [`49-connectivity-link-kernel.md`](49-connectivity-link-kernel.md).

## 1. What already exists (verified 2026-07-06)

| Substrate | Where | Notes |
|---|---|---|
| Body catalog (Sun/EMB/Earth/Moon: radius, **GM**, and an optional `IauRotation`) | `lunco-celestial/registry.rs` `CelestialBodyRegistry::default_system` | NAIF ids; GM is exactly what a Kepler propagator needs. Rotation is **not** stored as a rate/axis pair — see below |
| Real ephemeris (VSOP2013/ELP) | `lunco-celestial-ephemeris` `EphemerisProvider::global_position(naif, epoch_jd)` → ecliptic J2000 AU | NoOp fallback installed by `CelestialPlugin` |
| Rotation model (IAU/WGCCRE) | `lunco-celestial/iau.rs` `IauRotation` | The published elements, verbatim: `pole_ra`, `pole_dec` (+ their per-century rates), `w0`, `w_rate`. Pole, spin and body-fixed rotation are all **derived** from them |
| Frame conventions | `coords.rs::ecliptic_to_bevy` (AU→m, obliquity, Y-up remap); `systems.rs::body_rotation_system` (grid rotation = `geo::body_rotation(desc, jd)`) | Body-fixed = grid frame under that rotation |
| big_space solar hierarchy + globes | `big_space_setup.rs` (Solar→EMB→Earth/Moon grids, `GlobeLod` cube-sphere Earth/Moon, Observer camera) | Nests under `WorldShellPlugin` root when present (`.after(WorldShellSet)`); only the `luncosim` app enables it today |
| Camera stack | `lunco-avatar`: `OrbitCamera`/`SurfaceCamera`/`FreeFlightCamera`, **`FrameBlend`** smooth transition, `FocusTarget`/`FollowTarget` commands | Smooth view changes ride `FrameBlend` |
| Georef anchor vocabulary | `lunco:anchor:lat/lon/height` + `metersPerUnit` → `TerrainGeoref` (`lunco-terrain-surface/georef.rs`), read by the USD→DEM bridge | Pure data; no lat/lon→cartesian math yet |
| Absolute positions | `lunco_core::coords::world_position_seeded` → DVec3 in the big_space root frame | Used by gravity/SOI/visuals |
| Ports | `lunco-core/ports.rs` `PortRegistry` + fn-pointer `PortBackend`; group pattern `lunco-cosim/ports.rs` (`AvianGroup`, e.g. `RANGE_SENSOR_GROUP`) | New scalar ports = new backend or new group |
| Discrete events | `lunco-core/telemetry.rs` `TelemetryEvent` (observer bus) | AOS/LOS edges go here |
| USD→component bridge | `lunco-usd-sim/lib.rs` `process_usd_sim_prim_read<R: UsdRead>` attr branches (e.g. `lunco:sensor:range` → `RangeSensor`) | Exact shape to clone for comms/orbit/anchor attrs |
| USD wiring | native `connectionPaths` (`rewire_usd_connections`), journaled `ApplyUsdOp` | Components wire ports without new Rust |
| Attr write-back for UI | `sandbox-edit inspector apply_usd_attribute_change` → `UsdOp::SetAttribute` (journaled) | Generic; only widgets are missing |
| Moonbase twin | `~/Documents/models/moonbase/twin/moonbase_scene.usda` — Shackleton connecting-ridge glb (16×16 km), structures incl. `comms_mast.usda`, skid/ackermann rovers | No georef anchor authored yet |

Missing (all greenfield): Kepler propagator, geodetic↔cartesian math, LOS/
occlusion/link layer, ground-station/satellite/antenna assets, sandbox solar
view + surface⇄orbital transition, positioning UI.

## 2. Design

Per docs 36/38: comms is a **domain, not a kernel** — no new crate. The only
new Rust is domain-neutral geometry in `lunco-celestial` (geodesy, Kepler,
sight-lines) plus thin bridges (USD attrs → components, a ports backend, UI
widgets). Everything vehicle-specific is USD content.

### 2.1 Frames — one canonical math frame

All link math runs in the **solar frame**: Bevy-axes (Y-up), meters,
heliocentric — i.e. `ecliptic_to_bevy(global_position(naif, jd))` for body
centers. Body-fixed points use the same rotation the render grids use:
`geo::body_rotation(desc, jd)` — the one shared helper, so math and visuals
cannot diverge.

### Rotation is the IAU/WGCCRE model, authored once

`geo::body_rotation` delegates to `IauRotation::rotation_bevy`. **The rotation
elements are stored exactly as the IAU/WGCCRE reports publish them** — pole right
ascension and declination (with their per-century rates), the prime-meridian angle
`W₀`, and the spin rate `Ẇ` — plus a body-specific periodic (nutation/libration)
series. Everything the engine consumes is **derived** from those:

| Derived quantity | From |
|---|---|
| `BodyDescriptor::polar_axis(jd)` | `IauRotation::pole_bevy` — the (α, δ) pole, transformed ICRF → engine |
| `BodyDescriptor::rotation_rate_rad_per_day()` | `Ẇ`, converted |
| the body-fixed rotation quaternion | `IauRotation::rotation_bevy` — pole tilt **composed with** the prime-meridian angle `W(t) = W₀ + Ẇ·d` |

> **Why derived and not stored.** These are not three independent knobs; they are
> three views of one published dataset. A cached `rotation_rate_rad_per_day` field
> alongside the elements is a value that can silently disagree with them. There is
> exactly one authored copy (`iau.rs`), and the rest are functions of it.

> **Why `W₀` cannot be pasted in as a spin angle.** `W₀` is published as the angle
> **east of the node of the body's equator on the ICRF equator** — *not* of this
> engine's ecliptic +X. Composing it as a naked spin about the pole is wrong by the
> angle between those two references. `iau.rs::icrf_to_bevy` / `bevy_to_icrf` are
> the explicit frame transform that makes the composition correct; the 23.44°
> obliquity skew is real and will produce a plausible-looking, wrong answer if you
> skip it. **Without a prime-meridian epoch at all, the Moon's near side does not
> face Earth** — that is the bug this model exists to prevent.

Geodetic convention (spherical, matches `TerrainGeoref` docs): body-fixed
position for lat φ, east-lon λ, height h is
`(R+h)·(cosφ·cosλ, sinφ, −cosφ·sinλ)` with Y = north pole.
ENU tangent basis: `Up` radial, `East = ∂/∂λ`, `North = Up × East` — matches
the terrain-georef ENU choice (East=+X, North=−Z, Up=+Y in local scenes).

**Solar azimuth is north-referenced** (`lunco-environment::solar`): radians
clockwise from north, `0 = N`, `+π/2 = E`. That is the standard solar convention;
a south-referenced azimuth is off by 180° and looks entirely plausible.

### 2.2 New celestial modules (`lunco-celestial`)

- `geo.rs` — `Geodetic{lat_deg, lon_deg, height_m}`, `body_rotation(desc, jd)`,
  `geodetic_to_body_fixed`, `solar_position_of_geodetic`, `LocalTangentFrame`
  (ENU basis + `to_solar`/`from_solar` for scene-local points). Component
  **`GeodeticAnchor{body: i32, geodetic: Geodetic}`**.
- `kepler.rs` — `KeplerianElements{a_m, e, inclination_deg, raan_deg, argp_deg,
  mean_anomaly_deg, epoch_jd}`; `position_bevy_m(gm, jd)` solves Kepler (Newton).
  Component **`KeplerOrbit{body: i32, elements}`**.

  > **The elements are referenced to the BODY'S EQUATOR, not the ecliptic.**
  > Inclination is measured from the body's equatorial plane and RAAN about the
  > body's pole — the same pole latitudes use in `geo`, so `i = 90°` really does
  > fly over the geographic poles of the rendered globe. `position_bevy_m` returns
  > a **pole-up orbit frame** (pole = +Y); lifting it into the engine frame is
  > `geo::equatorial_frame` (tilts +Y onto the body's real pole), and only *then*
  > may the body's spin be composed. Collapsing those two steps measures
  > inclination about the **ecliptic** pole instead — for Earth's 23.44° tilt that
  > puts an ISS-like `i = 51.6°` orbit visibly in the wrong plane.
### 2.3 Site frame (scene ⇄ body)

A scene root prim may author `lunco:anchor:lat/lon/height` (+
`lunco:anchor:body`, default Moon 301). The bridge inserts `GeodeticAnchor` and
a `SiteFrame` resource is derived from the root-prim anchor: scene origin sits
at that geodetic point, scene axes = ENU (East=+X, North=−Z, Up=+Y). This one
anchor gives every scene-local antenna (rover mast, base mast) a solar
position, with or without the visual solar hierarchy.

### 2.4 USD vocabulary (authored, no schema code)

```
double lunco:anchor:lat / lon / height   # existing namespace, reused
int    lunco:anchor:body = 301           # NAIF id (399 Earth, 301 Moon)

int    lunco:orbit:body = 301
double lunco:orbit:semiMajorAxisM, lunco:orbit:eccentricity,
       lunco:orbit:inclinationDeg, lunco:orbit:raanDeg,
       lunco:orbit:argPeriapsisDeg, lunco:orbit:meanAnomalyDeg,
       lunco:orbit:epochJd            # default J2000
```

Bridged in `process_usd_sim_prim_read` (lunco-usd-sim gains a
`lunco-celestial` dep — no cycle).

### 2.5 Assets (content)

- `assets/components/comms/antenna.usda` — component: small dish geom + comms
  attrs; referenced into rover vessels (`skid_rover`, `ackermann_rover`) as a
  `Comms` scope.
- `assets/components/comms/ground_station.usda` — dish + `lunco:anchor:*` +
  antenna attrs.
- `assets/vessels/satellites/relay_sat.usda` — bus + panels + antenna +
  `lunco:orbit:*` defaults (lunar ELFO: a 6540 km, e 0.6, i 57.7°, ω 90° —
  apolune dwells over the south pole, Lunar-Pathfinder-like).
- Moonbase twin: scene-root anchor (Shackleton connecting ridge ≈ 89.45° S,
  136.7° W), antenna flag on `comms_mast`, `RelaySat` + `DSS_Madrid`
  (40.4314° N, −4.2481° E, Earth 399) prims.
- `assets/scenes/tests/comms_demo.usda` — minimal headless-testable
  scene (anchor + rover antenna + satellite + Earth station).

### 2.6 Solar-system view in the sandbox

The sandbox already runs `WorldShellPlugin` + big_space; the solar hierarchy is
designed to nest under the shell root. Enablement is per-twin:
`twin.toml [celestial] enabled = true` (moonbase: on; default sandbox twin:
off — existing test scenes unchanged). When enabled the sandbox app adds
`CelestialPlugin` + `EphemerisPlugin` (dropping its direct `GravityPlugin`
add — CelestialPlugin includes it).

**Site anchoring** (the seamless part): rather than moving the scene to the
Moon, a system pins the solar hierarchy so the site's geodetic point coincides
with the scene origin and ENU aligns with scene axes — i.e. `SolarSystemRoot`
gets `rotation = R_enu⁻¹`, `translation = −R·p_site(jd)`, re-projected on epoch
change. Scene physics/content never move; the Moon globe appears under the
terrain patch, Earth and Sun stand in the correct sky positions, and zooming
out is one continuous space (camera scroll + `FocusTarget{Moon|Earth}` +
`FrameBlend` for scripted transitions). Satellites with `KeplerOrbit` are
positioned on their central body's grid each epoch tick (hidden when the
hierarchy is disabled).

### 2.7 Positioning UI ("settings")

Inspector gains a **Comms & Orbit** section (selection-based) when the selected
prim authors anchor/orbit/comms attrs: lat/lon/height, orbital elements,
range/elevation-mask DragValues. Edits write journaled
`UsdOp::SetAttribute` via the existing `apply_usd_attribute_change` AND update
the live component (the sim-prim bridge only runs once per prim).

The comms overlay this section once claimed had landed **did not exist** — nothing
rendered link state at all until `lunco-render-bevy/src/link_viz.rs`, which draws a
line per node pair coloured by state and is toggled with `ToggleLinkViz` rather than
a settings-menu checkbox. Connectivity itself is doc 49's, not this doc's.

## 3. Implementation notes (landed with this doc)

- `lunco-celestial`: `geo.rs` (Geodetic, `GeodeticAnchor`, `SiteAnchor`,
  tangent frames, `body_rotation` — now shared with `body_rotation_system`),
  `kepler.rs` (`KeplerianElements`, `KeplerOrbit`, Newton solver),
  `placement.rs` (`anchor_solar_frame_to_site`,
  `place_celestial_bound_entities`).
- **`CelestialConfig`** `{spawn_hierarchy, spawn_observer_camera}` gates the
  hierarchy: defaults preserve `luncosim`; the sandbox runs the celestial
  stack dormant and `enable_celestial_on_site_anchor` flips it when a
  site-anchored scene loads. The hierarchy spawn moved Startup→Update
  (idempotent, `run_if`), the Observer Camera + its FloatingOrigin claim are
  config-gated, mission loading is gated on the hierarchy, and CelestialPlugin
  no longer clobbers a host's `Gravity` choice (and guards double-adds of
  Terrain/Gravity plugins).
- Sandbox adds `CelestialPlugin` + `EphemerisPlugin` (all platforms, wasm
  included);  the ephemeris dep moved out of the native-only
  section.
- USD bridge: `lunco-usd-sim/src/celestial.rs`, called from
  `process_usd_sim_prim_read`.
- Assets: `components/comms/{antenna,ground_station}.usda`,
  `vessels/satellites/relay_sat.usda`, `scenes/tests/comms_demo.usda`;
  `skid_rover.usda` gained a `Comms` scope; the moonbase twin gained the site
  anchor, mast antenna attrs, `RelaySat_1`, `DSS_Madrid`.
- Inspector: **Comms & Orbit** section (anchor lat/lon/height/body, Kepler
  elements, antenna range/mask — live component update + journaled
  `SetAttribute`) + read-only link list and Earth-route status.
- Tests: `lunco-celestial` unit tests (geodesy round-trips, ENU, rotation
  consistency, Kepler radii/period/inclination, occlusion, port tokens) +
  `lunco-usd-sim/tests/comms_connectivity.rs` (attr bridge → links → ports on
  a deterministic test ephemeris; near-side direct + limb relay-route cases).

### Globe tile assembly (fixed here)

`globe_lod.rs` used `set_parent_in_place` after authoring each tile's
grid-local `(CellCoord, Transform)`. That bevy method **overwrites the child
`Transform`** from its current `GlobalTransform` — identity on a fresh spawn —
so every tile's placement became `identity.reparented_to(surface_grid_global)`:
zero at startup (all tiles collapsed onto the body centre = the long-standing
"globe invisible" TODO) and camera-distance garbage later (exploded shards
from orbit). Fix: atomic `ChildOf` in the spawn bundle (the sanctioned
`migrate_to_grid` shape; `set_parent_in_place` is clippy-banned workspace-wide
for exactly this class of bug). The
`test_tile_positions_match_grid_decomposition` test was rewritten to mirror
the real f64 round-based `Grid::translation_to_grid` (its old f32/floor
simulation manufactured ~0.1 m of artifact error).

### Focus camera vs surface state (fixed here)

Clicking a planet (or `focus()` from rhai) while in a surface view produced a
jittering, half-black orbital view. Two independent defects in `lunco-avatar`:

- **Rotation fight**: `on_focus_command` stripped
  `OrbitCamera/FreeFlightCamera/SpringArmCamera` but left `SurfaceCamera` (and
  `SurfaceRelativeMode`/`GravityBody`) on the avatar. `surface_camera_system`
  runs *after* `orbit_system` in the PostUpdate chain and rebuilds the
  rotation as a ground-level tangent frame each frame — the camera orbits the
  body but looks at the horizon, and its radial "up" swings as the orbit arm
  eases, so the planet sweeps in and out of frame. Fix: focus now strips the
  surface components too (same set `on_leave_surface` already removed).
- **Far plane clipped Earth**: `update_avatar_clip_planes_system` required
  `&CellCoord` on `CelestialBody` entities — but bodies deliberately carry
  none (they sit at their grid origin), so zero bodies matched and the
  body-less fallback pinned `far = 1e7 m`. Earth focus sits at 1.9e7 m →
  the entire planet was beyond the far plane (black screen). The Moon
  (5.2e6 m at 3 radii) stayed inside it, which is why Moon focus mostly
  rendered. It also compared the camera's grid-local position against each
  body's *own-grid-local* position — meaningless across grids. Fix: both
  sides use `GlobalTransform` translations — big_space rebases them around
  the floating origin, so they are in ONE consistent frame every frame.

  ⚠ An intermediate fix used `world_position_seeded` — do not repeat it.
  That helper sums nested grid translations WITHOUT applying grid
  rotations, so across the site-anchored Solar Grid (rotation `align`,
  translation ~1.5e11 m) the ecliptic-axis and site-axis terms don't
  cancel: body "absolute" positions came out as ~1e11 m mixed-frame
  garbage, the near/far planes flapped every epoch tick, and the whole
  viewport strobed white (terrain visible) ↔ black (near-clipped). The
  helper is only valid when every grid on the path has identity rotation.
  The near plane also subtracts 20 km of terrain headroom — `min_dist`
  measures to the reference sphere, and a raised site (Shackleton ridge
  ≈ 1.2 km) would otherwise clip the ground underfoot.
- **Orbital depth-precision flicker** (separate from the strobe above; it
  survived the `GlobalTransform` fix): the surface loaded fine but orbital
  Earth/Moon still flickered. `near` was pinned to ≤ 100 m
  (`(min_dist − 20 km)·0.01`, clamped `[0.1, 100]`) while `far` tracks the
  farthest body — the Sun at ~1.5e11 m. In orbital view the focused body
  sits ~2e7 m out, ~0.01 % into the depth range; under reverse-Z (1/z) all
  precision bunches at the near plane, so the globe lands in the starved
  tail and adjacent LOD tile seams z-fight, strobing frame-to-frame as the
  camera micro-jitters. The surface is unaffected because you look at near
  terrain, which hogs the 1/z precision. Fix: anchor the near plane to the
  nearest surface — `near = (min_dist − 20 km).max(0.1)`, no upper clamp —
  so the viewed body sits *at* the near plane where reverse-Z precision
  peaks; on the surface `min_dist` collapses to ~0 and `near` floors at
  0.1 m (unchanged). The rule: an adaptive near plane must scale with
  viewing distance, never sit at a fixed ceiling.
- **Globe LOD flapping**: `subdivide_face` split on a sharp
  `dist < arc·factor` threshold and focus parks the camera at exactly
  3.0 radii — on-threshold noise re-decided the leaf set every frame,
  despawning/respawning tiles with fresh mesh assets (the planet flickered
  in and out). Now a ±5% dead band keyed on the resident leaf set
  (resident leaf splits below 0.95·T, split node re-merges above 1.05·T).
  The LOD also followed `cameras.iter().next()` — an arbitrary `Camera3d`,
  including offscreen USD-preview cameras, and archetype moves can flip
  iteration order per frame — it now follows only the active
  window-targeting camera.
- **Sunlit arrival**: focusing a body now derives yaw/pitch so the camera
  arrives on the sunlit side (sun direction from `GlobalTransform`s,
  re-expressed in the target grid's frame, +0.4 rad off the sun line so
  the terminator stays visible). Preserving the incoming yaw/pitch dropped
  the camera on an arbitrary — usually night — side, where an unlit planet
  disk is invisible.

Frame realignment terrain ⇄ orbital falls out of these: the orbit camera
always looks at its target, so focusing a body gives a planet-centred view
(gimbal up = the body grid's axis) and focusing the rover restores the
site-ENU frame (gimbal up = scene +Y = local vertical — ground beneath). The
site anchor's position/tilt on the Moon is already what pins those two frames
together. A smooth up-vector *blend* while scroll-zooming a body orbit
(Google-Earth style) is deferred — at a polar site scene-up and pole-up are
antiparallel, so a naive slerp degenerates; needs a great-circle path choice.

Deferred (tracked, not in this slice): `CommsLink.mo` Friis/data-rate/buffer
model (doc 36 layer B — ride the `comms-degradation` subsystem toggle),
DomeLight starfield sky, terrain raycast occlusion near-field, per-station
antenna pattern/gain, comms overlay gizmos, per-instance orbit epoch
authoring UI.
