# Implementation Tasks: 002-celestial-visualization

## Phase 0: Scenario System & Feature Flags (AD-5)

- [ ] 0.1 Add `sandbox` and `celestial` feature flags
  - Update `Cargo.toml` workspace and `lunco-sim-client/Cargo.toml` with feature definitions.
  - Move current `setup_scenario()` behind `#[cfg(feature = "sandbox")]`.
  - Create empty `setup_celestial_scenario()` as the `#[cfg(not(feature = "sandbox"))]` default.
  - Verify: `cargo run --features sandbox` loads flat-ground world. `cargo run` loads empty celestial world.
  - **Depends on**: None
  - **Requirement**: FR-018

## Phase 1: Foundation (Crate, Registry & `big_space`)

- [ ] 1.1 Create `lunco-sim-celestial` crate
  - Initialize boilerplate, `Cargo.toml` with dependencies: `big_space`, `celestial-ephemeris`, `celestial-time`, `bevy`.
  - Define `CelestialPlugin` with empty startup.
  - Add to workspace `Cargo.toml`.
  - Add as dependency of `lunco-sim-client`.
  - **Verify `celestial-ephemeris` compiles**: If it fails, switch to `vsop87` + `simple-elpmpp02` and adjust 3.1.
  - **Depends on**: 0.1
  - **Requirement**: FR-001

- [ ] 1.2 Implement `CelestialBodyRegistry`
  - Define `BodyDescriptor` struct (name, NAIF ID, radius, GM, SOI radius, parent ID, texture path, **rotation_rate_rad_per_day**, **polar_axis**).
  - Populate with Sun (NAIF 10), Earth-Moon Barycenter (NAIF 3), Earth (NAIF 399), Moon (NAIF 301).
  - Barycenter is a virtual node (no mesh, no radius) — only used for hierarchical positioning.
  - Sun is a virtual node (no mesh — rendered as light source only per AD-9).
  - Earth rotation: `2π / 0.99726968` rad/day, polar axis tilted 23.44°.
  - Moon rotation: `2π / 27.321661` rad/day (tidally locked).
  - Unit test: `test_registry_extensibility` — add Mars (NAIF 499) at runtime.
  - **Depends on**: 1.1
  - **Requirement**: FR-001, FR-020, SC-005

- [ ] 1.3 Setup `big_space` Nestable Grid Hierarchy (AD-1)
  - `lunco-sim-celestial` owns all `big_space` setup.
  - Configure root `BigSpace` with `i64` grid cells for solar system scale.
  - Create nested grids for Earth and Moon as children of barycenter node.
  - Position Earth offset ~4,671 km from barycenter center.
  - Attach `FloatingOrigin` to the camera entity.
  - Downstream crates (physics, rover, avatar) are unaware — they use local `Transform`.
  - **Depends on**: 1.2
  - **Requirement**: FR-005, FR-002

- [ ] 1.4 Verify `big_space` 0.12.0 nestable grid behavior
  - Write a minimal test spawning entities in nested grids and verifying positions.
  - Confirm floating origin works across parent-child grid boundaries.
  - Test: `test_grid_transition` — move entity across nested grid boundary, verify no position discontinuity.
  - **Depends on**: 1.3
  - **Requirement**: FR-005, SC-001

- [ ] 1.5 Implement `trait EphemerisProvider` (AD-4)
  - Define `trait EphemerisProvider` with `fn position(&self, body_id, epoch_jd) -> DVec3` (ecliptic J2000, AU).
  - Implement `CelestialEphemerisProvider` wrapping `celestial-ephemeris` crate.
  - Store as `Box<dyn EphemerisProvider>` resource so the implementation can be swapped.
  - Test: `test_ephemeris_provider_returns_positions` — verify non-zero positions for Earth, Moon.
  - **Depends on**: 1.1
  - **Requirement**: FR-016

- [ ] 1.6 Implement `celestial::coords` Utility Module (AD-6)
  - Implement `ecliptic_to_bevy(pos_au: DVec3) -> DVec3` (meters, Bevy Y-up).
  - Handles: ecliptic → equatorial (23.44° obliquity), equatorial → Bevy axes, AU → meters.
  - Test: `test_ecliptic_to_bevy_earth` — known Earth ecliptic position maps to expected Bevy coordinates.
  - Test: `test_au_to_meters` — 1 AU = 149,597,870,700 m.
  - **Depends on**: 1.1
  - **Requirement**: FR-019

## Phase 2: Temporal Core (Basic Clocks)

- [ ] 2.1 Implement `SimulationClockSet` Resource
  - Define `CelestialClock` (Julian Date TDB epoch as `f64`, speed multiplier, pause state).
  - `AppClock` is Bevy's standard `Time` — document that it's always 1.0×.
  - Implement `celestial_clock_tick_system` that advances epoch by `speed * bevy_dt`.
  - Use `celestial-time` crate (AD-10) for JD TDB ↔ UTC conversion.
  - Initialize clock epoch from system UTC time → JD TDB on startup.
  - Test: `test_celestial_clock_tick` — verify epoch advances correctly at X1, X100, X1M.
  - Test: `test_celestial_clock_pause` — verify epoch holds when paused while Bevy Time continues.
  - Test: `test_clock_independence` — Bevy `Time` unaffected by CelestialClock speed changes.
  - Test: `test_jd_utc_roundtrip` — convert JD → UTC → JD, verify < 1ms drift.
  - **Depends on**: 1.1
  - **Requirement**: FR-010, FR-021

- [ ] 2.2 [P2] Create Time Scrubber UI
  - Add `egui` panel in `lunco-sim-client` for:
    - Current epoch (**UTC display** via `celestial-time` conversion from JD TDB).
    - Speed multiplier buttons/slider (X1, X10, X100, X1K, X10K, X100K, X1M).
    - Pause/Play toggle.
    - Date picker for jumping to specific epoch (UTC input → JD TDB via `celestial-time`).
  - **Depends on**: 2.1
  - **Requirement**: FR-007, FR-021

- [ ] 2.3 Implement `TimeWarpState` Resource (FR-027)
  - Define `TimeWarpState` resource with `speed: f64` and `physics_enabled: bool`.
  - `time_warp_state_system`: Set `physics_enabled = false` when celestial clock speed > 100×.
  - Physics crates can use `.run_if(|tw: Res<TimeWarpState>| tw.physics_enabled)` to gate their systems.
  - Test: `test_time_warp_disables_physics` — speed > 100× sets `physics_enabled = false`.
  - Test: `test_time_warp_enables_physics` — speed ≤ 100× sets `physics_enabled = true`.
  - **Depends on**: 2.1
  - **Requirement**: FR-027

## Phase 3: Celestial Mechanics (Ephemeris & Positioning)

> **System Ordering (FR-026)**: All celestial systems in this phase MUST be registered as `.chain()` in `CelestialPlugin` to enforce: clock tick → time warp → ephemeris → rotation → sun light → SOI → gravity → terrain → camera → clip planes.

- [ ] 3.1 Integrate Ephemeris via `EphemerisProvider` (AD-4)
  - Wire `CelestialEphemerisProvider` (from 1.5) into the update loop.
  - Implement `ephemeris_update_system` that queries `CelestialClock` epoch and computes body positions.
  - Use VSOP2013 for Earth position (heliocentric ecliptic J2000).
  - Use ELP/MPP02 for Moon position (geocentric ecliptic). **Convert geocentric → barycentric** before writing to Moon's grid: `moon_bary = moon_geocentric - earth_bary_offset` (see FR-002).
  - Convert via `celestial::coords::ecliptic_to_bevy()` (from 1.6).
  - Write positions to `big_space` grid cells in the appropriate nested grids.
  - Test: `test_earth_position_2026` — compare against JPL Horizons reference.
  - Test: `test_moon_position_2026` — compare against reference.
  - Test: `test_barycenter_offset` — verify Earth offset ~4,671 km from barycenter.
  - **Depends on**: 2.1, 1.3, 1.5, 1.6
  - **Requirement**: FR-016, SC-003

- [ ] 3.2 [P2] JPL SPK Kernel Support for Artemis 2
  - Use `celestial-ephemeris` SPK reader to load `.bsp` files.
  - Parse Artemis 2 trajectory data into timestamped position arrays.
  - **Depends on**: 3.1
  - **Requirement**: FR-016, FR-006

- [ ] 3.3 Spawn Celestial Bodies from Registry (AD-11)
  - Iterate `CelestialBodyRegistry`, spawn entities with:
    - `CelestialBody` component
    - **Icosphere mesh `ico(5)`** (10,242 verts) for Earth and Moon (AD-11). Material with texture if specified.
    - **Sun: NO mesh** — spawn only as a positional reference for the `DirectionalLight` (AD-9)
    - Position in parent's `big_space` nested grid
  - Barycenter node: no mesh, just a transform parent.
  - Implement **Sun Light System** (AD-9):
    - `DirectionalLight` rotation computed from Sun's ephemeris position relative to camera focus body.
    - **UI screen-space marker** indicating Sun direction (small glyph/icon).
    - Updated each frame as bodies move.
  - **Depends on**: 3.1, 1.3
  - **Requirement**: FR-001, FR-003, FR-022, FR-023

- [ ] 3.4 Implement Body Rotation System (AD-7)
  - `body_rotation_system`: For each body with `rotation_rate_rad_per_day` and `polar_axis`, compute rotation quaternion from epoch.
  - `rotation_angle = (epoch_jd - J2000) * rotation_rate`.
  - Apply as `Transform` rotation on the body entity (affects texture orientation).
  - **Earth**: Sidereal rotation around tilted polar axis. Continents must face correct direction for epoch.
  - **Moon**: Tidal locking — rotation rate matches orbital angular velocity. Same face always toward Earth.
  - Test: `test_earth_rotation_24h` — after 1 sidereal day, Earth rotation angle ≈ 2π.
  - Test: `test_moon_tidal_lock` — Moon's near side faces Earth at multiple orbital positions.
  - **Depends on**: 3.3, 2.1
  - **Requirement**: FR-020, SC-007

- [ ] 3.5 Implement Sun UI Screen-Space Marker (AD-9)
  - Render a small sun glyph/icon at the screen-space projection of the Sun's direction.
  - Always visible regardless of camera zoom level or Sun distance.
  - Updated each frame from Sun's ephemeris position relative to camera.
  - Test: `test_sun_marker_visible` — marker renders when Sun is within viewport.
  - **Depends on**: 3.3
  - **Requirement**: FR-003, FR-022

## Phase 4: Gravity & SOI

- [ ] 4.1 Implement `GravityModel` Trait & Point-Mass
  - Define `trait GravityModel` with `fn acceleration(&self, pos: DVec3) -> DVec3`.
  - Implement `PointMassGravity` using GM from registry.
  - Attach gravity model to each `CelestialBody` entity.
  - Test: `test_point_mass_gravity_earth` — surface ≈ 9.81 m/s².
  - Test: `test_point_mass_gravity_moon` — surface ≈ 1.625 m/s².
  - Test: `test_gravity_inverse_square` — verify falloff at 2R, 5R, 10R.
  - **Depends on**: 1.2
  - **Requirement**: FR-011

- [ ] 4.2 Global Gravity Resource System (AD-2)
  - Implement `update_global_gravity_system`: find nearest body to camera/avatar, compute surface gravity, write to avian `Gravity` resource.
  - Existing physics crates read `Gravity` unchanged — zero modifications.
  - Test: `test_global_gravity_update` — when avatar moves from Earth area to Moon area, `Gravity` changes from 9.81 to 1.625.
  - **Depends on**: 4.1
  - **Requirement**: FR-011

- [ ] 4.3 Implement SOI System & Grid Re-parenting (AD-1)
  - Compute SOI radius for each body using Laplace SOI formula: $r_{SOI} = a \cdot (m/M)^{2/5}$ (chosen over Hill sphere exponent 1/3 for patched-conics convention alignment).
  - `soi_check_system`: For each entity with position, determine dominant body by SOI boundary.
  - On SOI transition, perform grid re-parenting:
    1. Compute entity's world-space `f64` position from current grid
    2. Re-parent entity to new body's nested `big_space` grid
    3. Compute new local `Transform` relative to new grid origin
    4. Update global `Gravity` to new body
  - Transparent to downstream crates — they see only a `Transform` update.
  - Test: `test_soi_boundary_moon` — entity at 60,000 km from Moon center is in Moon SOI; at 70,000 km is in Earth SOI.
  - Test: `test_grid_reparenting` — entity moved between grids has correct local Transform.
  - **Depends on**: 4.1, 1.3
  - **Requirement**: FR-012

- [ ] 4.4 Surface Coordinate System
  - Implement `SurfaceCoordinates` component (lat/lon/alt) for entities near a body.
  - Conversion: `grid_position_to_surface_coords(body, pos) -> SurfaceCoordinates`.
  - Conversion: `surface_coords_to_grid_position(body, coords) -> GridPos`.
  - Camera altitude from surface coordinates — used for "near surface" threshold.
  - Test: `test_surface_coordinates` — verify lat/lon/alt for known Moon positions.
  - **Depends on**: 4.1
  - **Requirement**: FR-013

## Phase 5: Navigation (Two-Camera System — AD-3)

- [ ] 5.1 Implement `ObserverCamera` (Macro Camera)
  - Owned by `lunco-sim-celestial`.
  - Dual-mode: Orbiting (around focus target) vs Free-float.
  - Target-based focusing: click or hotkey to focus on Earth, Moon, or any entity.
  - When focus changes, smoothly transition into target body's nested grid.
  - Input uses Bevy `Time` (AppClock), unaffected by CelestialClock speed.
  - Active in celestial mode; deactivated during Ground View.
  - **Initial state**: Focus on Earth at ~50,000 km distance on launch.
  - **Input gating**: Only active camera consumes input via `ActiveCamera` marker component (FR-028).
  - **Depends on**: 1.3
  - **Requirement**: FR-004, FR-009

- [ ] 5.2 Exponential Zoom Sensitivity
  - Scroll zoom: $\Delta distance = current\_distance \times k$ (k ≈ 0.1 per scroll step).
  - Minimum distance: body radius × 1.01 (just above surface).
  - Maximum distance: 10 AU.
  - Smooth interpolation (lerp) for zoom transitions.
  - **Depends on**: 5.1
  - **Requirement**: FR-004, SC-001

- [ ] 5.3 Focus Target Raycasting
  - Raycast from mouse position against celestial body colliders.
  - On click: set hit body as camera focus target.
  - Hotkeys: `1` = Sun, `2` = Earth, `3` = Moon (extend dynamically from registry order).
  - **Depends on**: 5.1, 3.3
  - **Requirement**: FR-009

- [ ] 5.4 Ground View Transition (Observer → Avatar Handoff)
  - When ObserverCamera altitude < 1 km and user presses activation key:
    1. Record current position/orientation in body's nested grid
    2. Deactivate ObserverCamera (`is_active: false`)
    3. Spawn/activate AvatarCamera at same position (uses existing `lunco-sim-avatar`)
    4. Spawn terrain tile at surface position (Phase 6)
    5. Set global `Gravity` to body surface gravity
    6. **Sphere mesh stays visible** — provides horizon (AD-12)
  - Reverse transition: deactivate AvatarCamera, reactivate ObserverCamera. Despawn terrain tile.
  - **Depends on**: 5.1, 4.2, 6.1
  - **Requirement**: FR-004, SC-002, AD-3

- [ ] 5.5 Dynamic Camera Clip Planes (AD-13)
  - `update_clip_planes_system`: Adjust camera `near` clip based on altitude.
  - Formula: `near = max(altitude * 0.001, 0.1)`, clamped to `[0.1, 1000.0]`.
  - Bevy Infinite Reverse-Z handles far plane — no adjustment needed.
  - Runs every frame for the active camera (ObserverCamera or AvatarCamera).
  - Test: `test_clip_near_at_surface` — at 1m altitude, near = 0.1m.
  - Test: `test_clip_near_at_orbit` — at 1000km altitude, near = 100m (clamped).
  - **Depends on**: 4.4, 5.1
  - **Requirement**: FR-025, SC-008

## Phase 6: Surface Interaction (Terrain)

- [ ] 6.1 Basic Terrain Tile System (AD-12: Sphere Stays Visible)
  - `TerrainTileConfig` resource: tile size (default 10×10 km), spawn threshold altitude (default 50 km), max tile size 10 km (curvature constraint).
  - `terrain_spawn_system`: When camera altitude drops below threshold near a body, spawn a flat rectangular mesh + `RigidBody::Static` + `Collider::cuboid`.
  - **Tile positioned at `body_radius + 0.01m`** from body center — sits on top of sphere to avoid Z-fighting.
  - **Sphere mesh stays visible underneath** — provides continuous horizon. Curvature error: 7.2m for 10km tile on Moon (acceptable).
  - Tile entity lives in the body's nested `big_space` grid.
  - Test: `test_terrain_tile_collision` — spawn tile, drop rigid body, verify collision.
  - Test: `test_terrain_tile_size_config` — verify configurable dimensions.
  - Test: `test_terrain_above_sphere` — tile position is exactly at radius + 0.01m.
  - **Depends on**: 4.4, 1.3
  - **Requirement**: FR-014, FR-024, SC-004

- [ ] 6.2 Rover on Moon Integration
  - Spawn rover on terrain tile using existing `lunco-sim-rover-raycast` spawn functions.
  - Verify rover drives with Moon gravity (1.625 m/s²) — no modifications to rover crates needed.
  - Verify sphere visible at horizon while driving.
  - Test: `test_rover_on_moon_tile` — rover sits on tile without falling through, wheels have traction.
  - **Depends on**: 6.1, 4.2
  - **Requirement**: SC-004

## Phase 7: Scientific Visualization (Overlays & Textures)

- [ ] 7.1 Trajectory Pipeline
  - Data structure for trajectory segments (array of timestamped positions in parent body frame).
  - Render past segments as solid lines (Gizmos or line meshes).
  - Render future segments as dashed lines.
  - Fade alpha based on camera distance to trajectory.
  - **Depends on**: 3.1
  - **Requirement**: FR-006

- [ ] 7.2 [P2] Customizable Proximity Fading
  - Implement alpha-fading for trajectories and grids based on camera altitude/distance.
  - Configurable thresholds via resource.
  - **Depends on**: 5.1, 7.1
  - **Requirement**: FR-008

- [ ] 7.3 Lightweight Texture Preparation
  - Source/create equirectangular PNGs (<500KB each):
    - Earth: continent outlines on blue background.
    - Moon: grayscale feature map.
  - Apply via `StandardMaterial` UV-mapped to sphere meshes.
  - **Depends on**: 3.3
  - **Requirement**: FR-015

## Phase 8: Verification & Headless

- [ ] 8.1 Ephemeris Accuracy Test Suite
  - `test_earth_2020_2030`: Validate Earth position at 10 epochs across 2020-2030 against JPL reference.
  - `test_moon_2026`: Validate Moon position from ELP/MPP02 against reference.
  - `test_barycenter_hierarchy`: Verify the full Sun → Barycenter → Earth/Moon chain.
  - **Depends on**: 3.1
  - **Requirement**: SC-003

- [ ] 8.2 Precision Regression Tests
  - `test_jitter_1au`: Spawn entities at 1 AU in root grid, verify render Transform precision.
  - `test_jitter_earth_moon`: Spawn at Earth-Moon distance, verify.
  - `test_jitter_surface`: Spawn at 1m above surface in nested grid, verify.
  - `test_grid_transition`: Move entity across nested grid boundary, verify no position discontinuity.
  - **Depends on**: 1.4
  - **Requirement**: SC-001

- [ ] 8.3 Headless Integration Test
  - Run full `CelestialPlugin` (clock, ephemeris, gravity, SOI) for 1000 ticks without a window.
  - Verify deterministic: same epoch → same body positions.
  - Verify no panics from missing render resources.
  - **Depends on**: All phases
  - **Requirement**: Constitution VIII

- [ ] 8.4 Gravity & SOI Test Suite
  - `test_point_mass_earth_surface`: 9.81 m/s² ± 0.01.
  - `test_point_mass_moon_surface`: 1.625 m/s² ± 0.01.
  - `test_soi_transition_updates_gravity`: Entity crossing Moon SOI boundary triggers `Gravity` resource update.
  - `test_gravity_inverse_square`: Verify falloff at 2R, 5R, 10R.
  - `test_reparenting_transparent`: Physics crate reads the same `Transform` component before and after re-parenting (values change, but the component type is the same).
  - **Depends on**: 4.1, 4.2, 4.3
  - **Requirement**: FR-011, FR-012

- [ ] 8.5 Scenario System Tests
  - `test_sandbox_feature`: Build with `--features sandbox`, verify flat-ground world loads.
  - `test_celestial_default`: Build without features, verify celestial world loads.
  - **Depends on**: 0.1
  - **Requirement**: FR-018, SC-006
