# Feature Specification: Celestial Visualization & World Foundation

**Feature Branch**: `002-celestial-visualization`
**Created**: 2026-03-31
**Updated**: 2026-04-01
**Status**: Implemented & Stable
**Dependencies**: References `006-time-and-integrators` (advanced time/PhysicsMode), `009-coordinate-frame-tree` (advanced CFT)
**Input**: User description: "I want to have model of Earth/Moon/Sun system, kind of like kerbal. It has to be simple. I want to use exponential camera. I want to be able to rotate it. Ideally I want to visualise position/trajectory of Artemis 2 mission there. Earth and Moon should be simple spheres. Position of three bodies must be real based on real data. We should be able to set time when it happens. If we get close we should be able to drive rovers."

## Scope & Philosophy

This spec lays the **foundational world architecture** for a solar-scale lunar colony simulator. The immediate deliverable is a working Sun/Earth/Moon system with real ephemeris, an exponential camera, and basic surface interaction. However, every system is designed as an **extensible foundation** — hardcoded for three bodies now, but structured so that adding Mars, its moons, Lagrange points and the full solar system is a data-driven extension, not a rewrite.

**This spec owns:**
- Basic clock architecture (Celestial + Application clocks — the minimum needed for visualization and time scrubbing)
- Celestial body registry and rendering
- Gravity model interface (point-mass implementation)
- `big_space` integration, floating origin, and all spatial setup
- Basic surface terrain tiles
- Exponential observer camera (macro-scale)
- Trajectory visualization
- SOI transitions and grid re-parenting

**This spec does NOT own (deferred to other specs):**
- Advanced time decoupling, integrators, PhysicsMode state machine → `006-time-and-integrators`
- Full coordinate frame tree with URDF/USD mapping → `009-coordinate-frame-tree`
- Full astronomical environment (Lagrange points, n-body, asteroid belts) → `018-astronomical-environment`
- Environmental hazards (radiation, thermal) → deferred
- Multiplayer networking → `005-multiplayer-core` (but we design for server authority)
- Spatial partitioning / interest management → deferred

---

## Architecture Decisions (Resolved)

**Decision:** `lunco-celestial` owns all `big_space` setup (root grid, nested body grids, floating origin). **The Golden Bridge Protocol**: To satisfy `big_space`, `TransformPlugin` MUST be disabled in the client crate. UI and Window hit-testing are maintained via a custom `fix_spatial_components_for_non_grid_entities` shim that manually backfills `GlobalTransform` for non-grid entities.
**Rationale:** Resolves the hard conflict between `big_space`'s spatial requirements and Bevy's default UI systems. Minimizes changes to the 8 existing crates while ensuring full interactivity.

**SOI Transfer (Earth → Moon):** When an entity crosses from Earth's SOI into Moon's SOI, the celestial plugin performs a **grid re-parenting**:
1. The entity's world-space `f64` position is computed from its current grid cell + local transform.
2. The entity is moved into the Moon's nested grid (Bevy parent change).
3. A new local `Transform` is calculated relative to the Moon's grid origin.
4. From the physics/rover crate's perspective, **nothing changes** — they still read/write the same local `Transform`. The grid migration is transparent to downstream crates.
5. The global `Gravity` resource is updated to the new body's surface gravity.

### AD-2: Gravity Source
**Decision:** Global avian `Gravity(Vec3)` resource, set by the celestial plugin based on the body the camera/avatar is nearest to.
**Rationale:** Zero changes needed to existing physics crates — they already read avian's `Gravity` resource. Single-body-at-a-time is the operational reality for months (one player on one body). Per-entity gravity adds complexity with no immediate benefit. Extension path to per-entity gravity exists when multi-body orbital sim is needed.

### AD-3: Camera Architecture
**Decision:** Two cameras with explicit handoff:
- **ObserverCamera** (owned by `lunco-celestial`): Macro navigation. Focus on celestial bodies. Exponential zoom. `big_space`-aware.
- **AvatarCamera** (owned by `lunco-avatar`): Surface-level. Follows possessed rover. Orbit with linear zoom.
- **Camera Migration**: The active camera dynamically re-parents itself to the nearest `Grid` ancestor of its `focus_target` to maintain absolute coordinate precision (FR-005).
- Only one `Camera3d` active at a time. Ground View transition deactivates ObserverCamera, activates AvatarCamera.
**Rationale:** The migration system ensures that the floating origin always operates within the target body's local measurement system, eliminating z-fighting and coordinate drift.

### AD-4: Ephemeris Source
**Decision:** `celestial-ephemeris` crate (0.1.1-alpha.2), wrapped behind a `trait EphemerisProvider` for swappability.
**Rationale:** Single dependency provides VSOP2013 (planets), ELP/MPP02 (Moon), and JPL SPK reader (Artemis 2 trajectories). Alpha status accepted because the trait abstraction allows swapping to `vsop87` 3.0.0 + `simple-elpmpp02` 0.1.0 without changing any consuming system code. Fallback crates are stable and proven.

### AD-5: Scenario System
**Decision:** Feature flags — `sandbox` vs `celestial` (default). `cargo run` → celestial world. `cargo run --features sandbox` → flat-ground rover world.
**Rationale:** Developers need fast iteration on rover physics without loading the full celestial stack. Feature flags are the simplest mechanism. Default is celestial because that's the product — sandbox is a development convenience.

### AD-6: Coordinate Conversion
**Decision:** Shared utility module `celestial::coords` providing `ecliptic_to_bevy(pos_au: DVec3) -> DVec3` (in meters).
**Rationale:** Multiple systems need this conversion (ephemeris updates, trajectory rendering, SOI checks). Centralizing it prevents inconsistencies and bugs. The conversion chain (ecliptic J2000 → equatorial via 23.44° obliquity rotation → Bevy Y-up axes → AU-to-meters) is non-trivial and must be consistent everywhere.

### AD-7: Body Rotation
**Decision:** Simple constant-rate rotation based on epoch. Earth: sidereal (~360°/23h56m). Moon: tidally locked.
**Rationale:** Earth must show correct continent orientation when observed from the Moon's surface (SC-007). Tidal locking is essential — the Moon's near side must always face Earth. Constant-rate rotation is sufficient because precession/nutation/libration effects are sub-degree over the timescales we visualize. Full IAU rotation models are overkill for Phase 1.

### AD-8: Skybox
**Decision:** Skipped for Phase 1. Black background.
**Rationale:** Focus engineering effort on mechanics, not visuals. A cubemap starfield is a single-task addition later.

### AD-9: Sun Rendering
**Decision:** Sun IS rendered as a visible sphere mesh (`ico(4)`) in the root Solar Grid. It also serves as a `DirectionalLight` source + a UI screen-space marker icon.
**Rationale:** Providing a physical Sun mesh enhances the sense of scale when observing the solar system. Precision issues are mitigated by the Adaptive Optics (AD-13) which prevents the near-clip plane from swallowing the Sun at 1 AU range.

### AD-10: Time Conversion
**Decision:** Use `celestial-time` crate for Julian Date (TDB) ↔ UTC conversion.
**Rationale:** Comes as a transitive dependency of `celestial-ephemeris` (AD-4). Handles proper astronomical time scales (TDB, TT, UTC) including leap seconds. Avoids reinventing JD↔UTC math with edge cases.

### AD-11: Planet LOD Strategy
**Decision:** Single icosphere mesh per body (Bevy `Sphere::new(radius).mesh().ico(5)` → 10,242 vertices). Hard cut to terrain tile at altitude threshold. No billboard rendering for distant planets.
**Rationale:** At 5 subdivision levels, an icosphere looks smooth from any distance relevant to the Earth-Moon system. Billboard sprites for tiny distant planets add complexity without visible benefit at this scale (Earth and Moon are always nearby the camera focus). A hard cut to terrain is acceptable for Phase 1 — seamless sphere-to-terrain transition (quadtree CDLOD) is a major rendering undertaking deferred to a terrain spec. Extension: add billboard phase and LOD transitions later.

### AD-12: Sphere-Terrain Layering
**Decision:** When terrain tiles spawn, the sphere mesh remains visible underneath. The terrain tile sits ON TOP of the sphere at `radius + 0.01m` offset.
**Rationale:** Hiding the sphere when terrain spawns creates a black void at the horizon — completely immersion-breaking. Keeping the sphere visible underneath provides a continuous horizon. The curvature error of a 10km flat tile on the Moon is only 7.2 meters (formula: $h = R - \sqrt{R^2 - (d/2)^2}$) — invisible to the user. The smooth sphere texture at the horizon is acceptable for Phase 1, and far better than black void.

### AD-13: Dynamic Camera Clip Planes
**Decision:** Surface-Aware Dynamic `near` clip plane that scales with distance to the body surface.
**Rationale:** $near = (min\_dist\_to\_surface \times 0.001).max(0.1).min(10000.0)$. The 10km upper clamp is CRITICAL — it prevents the near-plane from becoming large enough to clip the Sun or distant planetary neighbors when focusing on a body at 1 AU range. Bevy's Infinite Reverse-Z handles the far plane (extended to 1e15m for solar visibility).

---

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Macroscopic Celestial Navigation & Focus (Priority: P1)

As a mission observer, I want to view the Earth, Moon, and Sun system from a detached camera and focus on specific objects using both hotkeys and mouse clicks.

**Acceptance Scenarios**:

1.  **Given** the simulation is started in celestial mode (default), **When** the observer camera is active, **Then** bodies are positioned for the **current UTC time** by default.
2.  **Given** the observer clicks on a celestial body, **Then** the camera centers its rotation and zoom behavior around that target.
3.  **Given** a new body is added to the `CelestialBodyRegistry`, **When** the simulation runs, **Then** its position is computed from ephemeris and it appears in the scene without code changes.

### User Story 2 - Exponential Multi-Scale Camera with Ground View (Priority: P1)

As a user, I want to seamlessly zoom from a view of the entire Earth-Moon system down to the lunar surface, and transition into a "Ground View" (Rover/Avatar) mode where the UI and character movement remain responsive regardless of simulation speed.

**Acceptance Scenarios**:

1.  **Given** the camera is zooming, **When** the distance to the target changes by orders of magnitude, **Then** `big_space` nestable grids manage coordinate precision — the camera operates within the target body's local grid.
2.  **Given** the camera is within 1km of the surface, **When** the user selects "Ground View", **Then** the ObserverCamera is deactivated and the AvatarCamera is activated. The Avatar's movement and the UI animations use **Application Clock (independent of Celestial Clock speed)**.
3.  **Given** the camera transitions from Earth-focused to Moon-focused, **Then** the `big_space` floating origin smoothly re-parents into the Moon's nested grid without visual discontinuity.

### User Story 3 - Detailed Mission Trajectory & Customizable Overlays (Priority: P2)

As a space enthusiast, I want to see the past and future flight path of Artemis 2, with customizable fading altitudes.

**Acceptance Scenarios**:

1.  **Given** a trajectory dataset is loaded (from JPL SPK kernel via `celestial-ephemeris`), **When** rendered, **Then** past segments are solid lines and future segments are dashed, in the parent body's reference frame.
2.  **Given** the camera approaches a trajectory, **When** within proximity threshold, **Then** trajectory segments smoothly fade in; when distant, they fade out.

### User Story 4 - Variable Speed Time Scrubbing (Priority: P2)

As a mission planner, I want to scrub through time at different speeds (X1 to X1,000,000) and see celestial bodies move to their correct positions.

**Acceptance Scenarios**:

1.  **Given** the simulation is at **X1 speed**, **When** running, **Then** the **Celestial Clock** advances in real-time and body positions update from ephemeris.
2.  **Given** the simulation is at **≥X100 speed**, **When** time is compressed, **Then** body positions update correctly from ephemeris. High-fidelity physics integration is suspended (deferred to `006` for the `PhysicsMode` state machine).
3.  **Given** time is scrubbed, **When** UI/Camera input is received, **Then** the **Application Clock** ensures UI remains responsive regardless of celestial time speed.

### User Story 5 - Lightweight Reference Textures (Priority: P2)

As a user, I want the Earth and Moon to have basic surface markings (continents/craters) so I can orient myself, without requiring high-resolution image assets.

**Acceptance Scenarios**:

1.  **Given** the macroscopic view, **When** the Earth is rendered, **Then** a lightweight (rasterized PNG, <1MB) continent map is visible as a UV-mapped texture.
2.  **Given** the Moon is rendered, **When** zoomed in, **Then** major craters and features are visible as simple markings on a UV texture.

### User Story 6 - Basic Surface Terrain & Rover Interaction (Priority: P1)

As a user, I want to land on the Moon and drive a rover on a simple flat terrain tile.

**Acceptance Scenarios**:

1.  **Given** the camera is near the lunar surface, **When** "Ground View" is entered, **Then** a flat terrain tile (configurable size: 1×1 km to 10×10 km) spawns at the surface position with a collision mesh inside the Moon's nested `big_space` grid.
2.  **Given** a terrain tile exists, **When** a rover is spawned, **Then** the rover drives on the tile using the existing `lunco-physics` systems. The global `Gravity` resource is set to Moon gravity (1.625 m/s²).
3.  **Given** the terrain tile size is changed via configuration, **When** the simulation restarts, **Then** the tile dimensions reflect the new configuration.

### User Story 7 - Sandbox Mode (Priority: P1)

As a developer, I want to run the flat-ground rover sandbox for quick physics iteration without loading the celestial system.

**Acceptance Scenarios**:

1.  **Given** the application is built with `--features sandbox`, **When** it runs, **Then** the current flat-ground scenario loads (1000m plane, rovers, Earth gravity). No `big_space` overhead.
2.  **Given** the application is built without extra features (default), **When** it runs, **Then** the celestial world loads.

---

## Requirements *(mandatory)*

### Functional Requirements

-   **FR-001**: **Extensible Body Registry**: A data-driven `CelestialBodyRegistry` resource that describes each body (name, ephemeris ID, radius, GM, parent body). Hardcoded for Sun/Earth/Moon now; extensible to any body that has ephemeris data.
-   **FR-002**: **Hierarchical Reference Frames (Foundation)**: Each `CelestialBody` defines a local reference frame via a `big_space` nested grid. Child bodies are positioned relative to their parent. Earth-Moon barycenter offset (~4,671 km) MUST be accounted for in the hierarchy. **Moon geocentric → barycentric conversion**: ELP/MPP02 returns the Moon's position relative to Earth's center (geocentric). Since the hierarchy parent is the Earth-Moon Barycenter, the Moon's position MUST be converted: `moon_bary = moon_geocentric - earth_bary_offset`. This is the simple foundation for `009-coordinate-frame-tree`.
-   **FR-003**: **Solar Lighting**: Sun is rendered as a `DirectionalLight` source only (not a visible sphere — AD-9). Direction computed from Sun's ephemeris position relative to camera focus. A UI screen-space marker indicates Sun direction. Updates as bodies move.
-   **FR-004**: **Exponential Observer Camera**: Macro-level camera owned by `lunco-celestial` with focus targets, exponential zoom sensitivity ($\Delta d = d \times k$), and orbiting/free-float modes. Camera input uses **Application Clock**. Coexists with the avatar camera via explicit handoff (AD-3). **Initial state**: On launch, ObserverCamera focuses on Earth at ~50,000 km distance. **Input gating**: Only the active camera consumes input; the inactive camera's input systems are gated by an `ActiveCamera` marker component (FR-028).
-   **FR-005**: **Multi-Scale Rendering via `big_space`**: `lunco-celestial` owns all `big_space` setup (AD-1):
    -   **Hierarchical Grids**: Multi-level nesting (Parent Grid -> Child Anchor Grid). Allows for coarse solar-scale cells and fine planetary-scale cells simultaneously.
    -   **Root Grid**: Solar system scale (Sun at origin). Grid cell precision: `i64`.
    -   **Body Grids**: Each major body (Earth, Moon) gets its own nested `Grid` anchor for local entities (rovers, surface features).
    -   **Floating Origin**: `FloatingOriginPlugin` on the camera. When the camera focuses on a body, it re-parents to that body's nested grid.
    -   **The Golden Bridge**: `TransformPlugin` is disabled; UI is maintained via manual transform backfilling for Non-Grid entities.
-   **FR-006**: **Trajectory Rendering**: Render trajectories with past (solid) and future (dashed) segments. Trajectories are computed in the parent body's reference frame. Fade based on camera proximity. Artemis 2 trajectory loadable from SPK kernel via `celestial-ephemeris`.
-   **FR-007**: **Time Scrubber UI**: `egui` panel for mission epoch control and speed multipliers (X1..X1M).
-   **FR-008**: **Customizable Proximity Fading**: Visibility thresholds for overlays, trajectory lines, and **configurable surface grid size** (default 10km sectors).
-   **FR-009**: **Mouse Interaction**: Select focus targets via raycasting on celestial bodies.
-   **FR-010**: **Basic Clock Architecture**: Two clocks owned by this spec:
    -   **Celestial Clock**: Julian Date TDB internally, UTC for display. Scrubbable (X1 to X1M). Drives ephemeris queries and body positions.
    -   **Application Clock**: Standard Bevy `Time` (always 1.0×). Drives UI, Camera, and Avatar movement.
    -   *(The Robotics Clock and advanced PhysicsMode transitions are owned by `006-time-and-integrators`.)*
-   **FR-011**: **Pluggable Gravity Model Architecture**: A `trait GravityModel` interface that allows different gravity implementations per body and per scale. This spec implements **point-mass gravity** as the default. The global avian `Gravity` resource is set by the celestial plugin based on the nearest body (AD-2).
    -   **Surface gravity**: Constant downward vector derived from body's GM and radius.
    -   **Orbital gravity**: Point-mass attraction from the dominant body (SOI parent).
    -   **Extension path**: Swap to spherical harmonics, mascon models, etc. in future specs.
-   **FR-012**: **Sphere of Influence (SOI) & Grid Re-parenting**: Each body has an SOI radius computed via the Laplace formula: $r_{SOI} = a \cdot (m/M)^{2/5}$ (chosen over Hill sphere for patched-conics convention alignment). SOI determines which body's gravity dominates and which `big_space` nested grid an entity belongs to. On SOI crossing, the celestial plugin transparently re-parents the entity to the new grid (AD-1). Downstream crates see only a local `Transform` update.
-   **FR-013**: **Surface Coordinate System**: Each body provides a basic lat/lon/altitude coordinate system. Camera altitude above surface determines "near surface" thresholds for Ground View transitions and terrain tile spawning.
-   **FR-014**: **Basic Surface Terrain Tiles**: Flat collision-enabled tiles spawned at the surface when camera/avatar is near ground level. Configurable size (1×1 km to 10×10 km). Lives in the body's nested `big_space` grid. Foundation for streaming terrain in a future spec.
-   **FR-015**: **Lightweight Texturing**: Use <2MB total assets. Earth gets a rasterized continent map (PNG UV-mapped). Moon gets a grayscale feature map. No SVG-on-sphere.
-   **FR-016**: **Ephemeris Integration via `celestial-ephemeris`** (AD-4): Single dependency providing VSOP2013, ELP/MPP02, and SPK reader. Wrapped behind a `trait EphemerisProvider` so the implementation can be swapped to alternative crates (e.g., `vsop87` + `simple-elpmpp02`) without changing consuming systems.
-   **FR-017**: **Server Authority Design**: All celestial state (body positions, clock state) is computed deterministically. All celestial computation is isolated in systems that can run headless. Designed so a future server can own the state and clients receive it.
-   **FR-018**: **Feature-Flagged Scenario System** (AD-5):
    -   Default (`celestial`): Full celestial world with bodies, grids, observer camera.
    -   `sandbox` feature: Flat-ground rover sandbox (current `setup_scenario`). No `big_space`.
-   **FR-019**: **Coordinate Conversion Utility** (AD-6): Shared `celestial::coords` module converting ecliptic J2000 (AU) → Bevy coordinate space (meters, Y-up). Handles obliquity rotation and unit scaling.
-   **FR-020**: **Body Rotation** (AD-7): Each body rotates based on epoch. Earth: sidereal rotation (~360°/23h56m) around its tilted polar axis. Moon: tidally locked — rotation synchronized to orbital position. Earth must appear realistically oriented when observed from the lunar surface.
-   **FR-021**: **Time Scale Conversion** (AD-10): Use `celestial-time` crate for Julian Date (TDB) ↔ UTC conversion. Time scrubber UI displays UTC; internal clock stores Julian Date.
-   **FR-022**: **Sun as Mesh + Light Source** (AD-9): Sun is rendered as a physical sphere mesh (`ico(4)`). It also carries a `DirectionalLight` and a screen-space UI marker. Optics fixes (FR-025) ensure it remains visible across solar system distances.
-   **FR-023**: **Planet Rendering Strategy** (AD-11): Each body is rendered as a single icosphere mesh (`ico(5)`, 10,242 vertices). At altitude < threshold, a hard cut transitions to terrain tiles. No billboard phase or multi-LOD mesh for Phase 1.
-   **FR-024**: **Sphere-Terrain Layering** (AD-12): When terrain tiles are active, the sphere mesh remains visible underneath. Tiles are offset at `radius + 0.01m` to avoid Z-fighting. Sphere provides the horizon; tiles provide collision and local detail. Curvature error: 7.2m per 10km tile on Moon (acceptable).
-   **FR-025**: **Dynamic Camera Clip Planes** (AD-13): Camera `near` clip plane adjusts dynamically based on surface distance ($altitude \times 0.001$). CLAMPED to range [0.1, 10000.0]. The 10km maximum limit is essential for maintaining solar visibility at 1 AU scale.
-   **FR-026**: **System Ordering**: All celestial systems MUST execute in deterministic order: clock tick → ephemeris update → body rotation → sun light → SOI check → gravity update → terrain spawn → camera → clip planes. Registered as `.chain()` in `CelestialPlugin`.
-   **FR-027**: **TimeWarp State Interface**: A `TimeWarpState` resource published by `lunco-celestial` indicates current time compression speed and whether physics should be active (`physics_enabled = false` when `speed > 100×`). Physics crates gate their systems on this resource. Full PhysicsMode state machine deferred to `006-time-and-integrators`.
-   **FR-028**: **Input Conflict Resolution**: Only the active camera (ObserverCamera or AvatarCamera) consumes input. An `ActiveCamera` marker component gates input systems. During handoff, the marker is atomically moved between cameras.

### Key Entities

-   **CelestialBodyRegistry**: Resource listing all bodies with physical parameters.
-   **CelestialBody**: Component on each body entity. References registry entry.
-   **ObserverCamera**: Macro-level camera. Focus targets, exponential zoom, `big_space`-aware. Deactivated during Ground View.
-   **AvatarCamera**: Surface-level camera (existing `lunco-avatar`). Activated during Ground View.
-   **SimulationClockSet**: Resource grouping Celestial and Application clocks.
-   **GravityModel**: Trait for pluggable gravity computation.
-   **SurfaceTile**: Component for flat terrain tiles with collision meshes.
-   **SurfaceCoordinates**: Component providing lat/lon/alt for entities near a body's surface.
-   **TimeWarpState**: Resource indicating current time compression speed and whether physics is enabled.
-   **ActiveCamera**: Marker component on the currently active camera entity. Gates input systems.

---

## Success Criteria *(mandatory)*

-   **SC-001**: Zero visual jitter at all scales — verified by rendering at 1 AU, 384,400 km (Earth-Moon), 1 km, and 1 m distances.
-   **SC-002**: UI and Camera remain responsive even when Celestial Clock is at X1,000,000 speed or paused.
-   **SC-003**: Body positions match reference data within acceptable tolerance (VSOP2013/ELP accuracy) for Sun/Earth/Moon at any epoch in 2020-2030.
-   **SC-004**: Rover can drive on a terrain tile spawned at the lunar surface with functional collision and Moon gravity (1.625 m/s²).
-   **SC-005**: Adding a new body to the registry (e.g., Mars) requires only data — no code changes to rendering or positioning.
-   **SC-006**: `cargo run --features sandbox` runs the flat-ground rover sandbox without celestial overhead.
-   **SC-007**: Earth viewed from the Moon's surface shows correct continent orientation for the given epoch.
-   **SC-008**: No Z-fighting or clipping artifacts at any camera altitude from 1 AU to 1m above surface.

---

## Assumptions

-   `big_space` 0.12.0 is compatible with Bevy 0.18.1 (confirmed — it targets `bevy ^0.18.0`).
-   `big_space` nestable grids support the parent-child grid hierarchy needed for Sun → Earth → Moon.
-   `celestial-ephemeris` 0.1.1-alpha.2 compiles and provides correct VSOP2013/ELP/MPP02 output. **Risk accepted** — alpha status mitigated by the `trait EphemerisProvider` abstraction allowing crate swap.
-   `celestial-time` crate (dependency of `celestial-ephemeris`) handles TDB ↔ UTC conversion correctly.
-   Earth-Moon barycenter offset (~4,671 km from Earth's center) is handled by positioning Earth relative to the barycenter in the hierarchy.
-   Global avian `Gravity` resource is sufficient for single-body-at-a-time simulation.

---

## Deferred / Out of Scope

-   **Patched conics / n-body propagation** → `018-astronomical-environment`
-   **Lagrange points, asteroid belts** → `018-astronomical-environment`
-   **Spherical harmonics / mascon gravity models** → `018` or dedicated spec
-   **PhysicsMode state machine (FullPhysics/OnRails/HybridBlend)** → `006-time-and-integrators`
-   **Robotics Clock** → `006-time-and-integrators`
-   **Analytical Accounting (power/thermal during time warp)** → `006` + `029-power-systems`
-   **Deformable terrain, heightmaps, terrain streaming** → future terrain spec
-   **Environmental hazards (radiation, thermal, dust)** → future specs
-   **Multiplayer networking** → `005-multiplayer-core`
-   **Spatial partitioning / interest management** → future spec
-   **Per-entity gravity (multiple bodies simultaneously)** → future upgrade from global `Gravity`
-   **Skybox / star background** → deferred (AD-8)
-   **Sun as visible sphere / billboard** → deferred (AD-9), currently light-only
-   **Precession, nutation, libration** → future (body rotation is constant-rate for Phase 1)
-   **Billboard LOD for tiny distant planets** → deferred (AD-11), single sphere mesh sufficient for E-M scale
-   **Seamless sphere-to-terrain transition (quadtree CDLOD)** → future terrain spec (AD-12), hard cut acceptable for Phase 1
-   **Logarithmic depth buffer** → not needed, Bevy Infinite Reverse-Z handles it (AD-13)
-   **SysML v2 mapping of celestial state** → deferred. Constitution III requires SysML v2 as structural blueprint; celestial body registry and clock state will need SysML v2 serialization in a future spec.
