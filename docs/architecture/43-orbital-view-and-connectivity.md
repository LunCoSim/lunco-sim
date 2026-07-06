# 43 — Orbital view & rover↔satellite↔Earth connectivity

Status: design + first implementation slice · Audience: engine + content authors

The MoonDAO connectivity demo needs four things on top of the existing substrate:
a Keplerian satellite, geodetically-placed ground stations (Earth + Moon), a
line-of-sight/link-availability layer publishing ports, and a sandbox that can
switch between the surface (moonbase terrain) and a solar-system view of the
same scene.

## 1. What already exists (verified 2026-07-06)

| Substrate | Where | Notes |
|---|---|---|
| Body catalog (Sun/EMB/Earth/Moon: radius, **GM**, rotation rate, polar axis) | `lunco-celestial/registry.rs` `CelestialBodyRegistry::default_system` | NAIF ids; GM is exactly what a Kepler propagator needs |
| Real ephemeris (VSOP2013/ELP) | `lunco-celestial-ephemeris` `EphemerisProvider::global_position(naif, epoch_jd)` → ecliptic J2000 AU | NoOp fallback installed by `CelestialPlugin` |
| Frame conventions | `coords.rs::ecliptic_to_bevy` (AU→m, obliquity, Y-up remap); `systems.rs::body_rotation_system` (grid spins `days_since_j2000 · rate` about `polar_axis`) | Body-fixed = grid frame under that rotation; angle 0 at J2000 |
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
`DQuat::from_axis_angle(polar_axis, days_since_j2000 · rate)` (shared helper so
math and visuals cannot diverge).

Geodetic convention (spherical, matches `TerrainGeoref` docs): body-fixed
position for lat φ, east-lon λ, height h is
`(R+h)·(cosφ·cosλ, sinφ, −cosφ·sinλ)` with Y = north pole, λ=0 on +X at J2000.
ENU tangent basis: `Up` radial, `East = ∂/∂λ`, `North = Up × East` — matches
the terrain-georef ENU choice (East=+X, North=−Z, Up=+Y in local scenes).

### 2.2 New celestial modules (`lunco-celestial`)

- `geo.rs` — `Geodetic{lat_deg, lon_deg, height_m}`, `body_rotation(desc, jd)`,
  `geodetic_to_body_fixed`, `solar_position_of_geodetic`, `LocalTangentFrame`
  (ENU basis + `to_solar`/`from_solar` for scene-local points). Component
  **`GeodeticAnchor{body: i32, geodetic: Geodetic}`**.
- `kepler.rs` — `KeplerianElements{a_m, e, inc_deg, raan_deg, argp_deg,
  mean_anomaly_deg, epoch_jd}`; `position_m(gm, jd)` solves Kepler (Newton) and
  returns ecliptic-frame meters relative to the central body. Component
  **`KeplerOrbit{body: i32, elements}`**.
- `comms.rs` — component **`CommsAntenna{max_range_m, min_elevation_deg}`**;
  resource `CommsLinks` (all pairwise `SightLine{range_m, elevation_deg,
  occluded_by, connected}` + per-antenna Earth-route BFS); systems gated on
  epoch/topology change. Antenna solar position resolution order:
  `GeodeticAnchor` → `KeplerOrbit` → scene-local (entity world position mapped
  through the **site frame**, below). Occlusion is the doc-36 analytic
  body-sphere test against every registry body (a body an endpoint stands on is
  excluded — the elevation mask owns the horizon there). AOS/LOS edges emit
  `TelemetryEvent`s. `CommsPlugin` registers components, systems, and a
  `PortBackend` publishing per-antenna ports:
  `link/<peer>/connected|range_m|elevation_deg`, `route/earth/connected|hops`.

### 2.3 Site frame (scene ⇄ body)

A scene root prim may author `lunco:anchor:lat/lon/height` (+
`lunco:anchor:body`, default Moon 301). The bridge inserts `GeodeticAnchor` and
a `SiteFrame` resource is derived from the root-prim anchor: scene origin sits
at that geodetic point, scene axes = ENU (East=+X, North=−Z, Up=+Y). This one
anchor gives every scene-local antenna (rover mast, base mast) a solar
position, with or without the visual solar hierarchy.

### 2.4 USD vocabulary (authored, no schema code)

```
bool   lunco:comms:antenna = true
double lunco:comms:maxRangeM        # default 1e12 (unconstrained)
double lunco:comms:minElevationDeg  # default 0 (surface antennas)
string lunco:comms:id               # stable peer id for port names; derived
                                    # when absent (parent leaf when the prim
                                    # is a generic "Comms" child — two rovers'
                                    # antennas stay distinct)

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
- `assets/scenes/sandbox/comms_demo_test.usda` — minimal headless-testable
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
the live component (the sim-prim bridge only runs once per prim). A
settings-menu toggle draws a comms overlay (direction rays colored by link
state).

## 3. Implementation notes (landed with this doc)

- `lunco-celestial`: `geo.rs` (Geodetic, `GeodeticAnchor`, `SiteAnchor`,
  tangent frames, `body_rotation` — now shared with `body_rotation_system`),
  `kepler.rs` (`KeplerianElements`, `KeplerOrbit`, Newton solver),
  `comms.rs` (`CommsAntenna{max_range_m,min_elevation_deg,id}`,
  `CommsLinkState`, `CommsLinks`, `update_comms_links`, ports backend,
  AOS/LOS `TelemetryEvent`s, `CommsPlugin`),
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
  included) + `CommsPlugin`; the ephemeris dep moved out of the native-only
  section.
- USD bridge: `lunco-usd-sim/src/celestial.rs`, called from
  `process_usd_sim_prim_read`.
- Assets: `components/comms/{antenna,ground_station}.usda`,
  `vessels/satellites/relay_sat.usda`, `scenes/sandbox/comms_demo_test.usda`;
  `skid_rover.usda` gained a `Comms` scope; the moonbase twin gained the site
  anchor, mast antenna attrs, `RelaySat_1`, `DSS_Madrid`.
- Inspector: **Comms & Orbit** section (anchor lat/lon/height/body, Kepler
  elements, antenna range/mask — live component update + journaled
  `SetAttribute`) + read-only link list and Earth-route status.
- Tests: `lunco-celestial` unit tests (geodesy round-trips, ENU, rotation
  consistency, Kepler radii/period/inclination, occlusion, port tokens) +
  `lunco-usd-sim/tests/comms_connectivity.rs` (attr bridge → links → ports on
  a deterministic test ephemeris; near-side direct + limb relay-route cases).

### Link policy is rhai data

The per-pair link verdict goes through the `comms.link.connected` hook
(`lunco_celestial::COMMS_LINK_HOOK`). The authored rule is
`assets/scripting/policy/comms_link.rhai` (`link_connected(ctx)` — ctx map:
`a/b, range_m, elev_a/b, min_elev_a/b, occluded, occluded_by, max_range_m`),
registered as a built-in policy by `lunco-scripting`. Scenarios re-shape it
live via the generic rhai verb **`register_hook(id, entry, src)`**
(world_bridge — works for ANY lunco-hooks seam: merge policy, RBAC,
control authority, comms). The Rust range+mask+occlusion rule remains only as
the fallback for a missing/broken script. `assets/scenarios/comms_demo.rhai`
is the worked example: link-table readout from the `comms:*` ports, the
surface→orbit→Earth camera tour, and a "solar storm" policy swap that cuts
direct DSN paths and then restores the canonical rule.

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

Deferred (tracked, not in this slice): `CommsLink.mo` Friis/data-rate/buffer
model (doc 36 layer B — ride the `comms-degradation` subsystem toggle),
DomeLight starfield sky, terrain raycast occlusion near-field, per-station
antenna pattern/gain, comms overlay gizmos, per-instance orbit epoch
authoring UI.
