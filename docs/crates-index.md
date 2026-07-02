# LunCoSim Crates Index

This document provides a comprehensive index of all crates in the LunCoSim workspace, categorized by their functional domain and architectural responsibility. It serves as a navigation guide for both developers and AI agents.

---

## 1. Workspace & Core Foundation
Low-level primitives, document/journal systems, time, and cross-cutting concerns (storage, assets, theming, settings).

| Crate | Responsibility |
| :--- | :--- |
| **`lunco-core`** | Core primitives (`DigitalPort`, `PhysicalPort`, the typed `Mutation<P>` command substrate, `SimTick`), coordinate systems, the `SceneViewport` (active-camera binding), the `ControlKernelRegistry` (drive-allocation kernels), and canonical diagram data types. |
| **`lunco-command-macro`** | Procedural macros for the typed command system (`#[Command]`, `#[on_command]`, `register_commands!`; re-exported by `lunco-core`). |
| **`lunco-workspace`** | Headless editor session management: open Twins, active documents, perspectives, and recents. |
| **`lunco-twin`** | The simulation unit on disk: folder structure, `twin.toml` manifest parsing, and file indexing. |
| **`lunco-twin-journal`** | Canonical Twin-scoped op log: Lamport-ordered entries, DAG parents (for future merges), Streams + Composition, ChangeSets, Markers (named milestones), Branches, `UndoManager`. CRDT-shapable schema; in-memory backend today, yrs-swap-ready. |
| **`lunco-doc`** | Foundation for structured artifacts (Modelica, USD, SysML): the `DocumentHost` container and atomic `DocumentOp` pattern with built-in undo/redo. |
| **`lunco-doc-bevy`** | Bevy ECS integration for the Document System: lifecycle events, `JournalResource` (Bevy wrapper around the canonical Twin journal), `BevyJournalSink` for remote-replay, `EditorIntent` keybindings, `Presence` collab seed. |
| **`lunco-storage`** | I/O abstraction layer (`Storage` trait — Native FS, Memory, future WASM/Remote backends). The single write path; raw `std::fs` is disallowed. |
| **`lunco-assets`** | Unified asset management: cache resolution across worktrees, versioned downloads (`Assets.toml`, SHA-256), and texture processing. |
| **`lunco-cache`** | Generic resource cache with in-flight deduplication for resolved URIs and parsed artifacts. |
| **`lunco-settings`** | Centralised user-settings: one JSON file (`~/.lunco/settings.json`), namespaced sections, auto-persist on change. |
| **`lunco-theme`** | Centralized design tokens (Catppuccin-based) for consistent UI across all panels and domains. |
| **`lunco-time`** | Unified mission-time spine (architecture doc 19): `MissionClock`/`TimeTransport`/`WorldTime`, the `TimeDomain` clock tree + animation transport, and the `scales` projection layer over `celestial-time`. |

---

## 2. Simulation Engine
The "Laws of Nature" — celestial mechanics, environmental state, terrain, obstacle fields, batch experiments, and co-simulation orchestration.

| Crate | Responsibility |
| :--- | :--- |
| **`lunco-celestial`** | Orbital mechanics (ephemeris abstraction), gravity, body-fixed rotation, and Sphere of Influence (SOI) transitions; sun-light driven from ephemeris. |
| **`lunco-celestial-ephemeris`** | Concrete high-fidelity ephemeris provider for `lunco-celestial` (VSOP2013 + ELP/MPP02 via `celestial-ephemeris`); the heavy, non-Windows-MSVC half of the celestial split and the one place `celestial-time` is allowed. |
| **`lunco-environment`** | Per-entity position-dependent environment state (atmosphere, radiation, local gravity). |
| **`lunco-terrain-core`** | Projection-agnostic terrain LOD spine: quadtree-CDLOD selection, tile-grid math, and the `HeightSource` trait. Pure (std + serde), shared by both the planar DEM streamer and the cube-sphere planetary tiler. |
| **`lunco-terrain-globe`** | Whole-body cube-sphere terrain tiling (orbital/planetary scale): quadtree-CDLOD globe, avian heightfield collision ring, `big_space` anchoring; the "globe" projection of the terrain family over the shared `lunco-terrain-core` LOD spine. |
| **`lunco-terrain-surface`** | Local high-detail DEM ground terrain (surface scale): heightfield colliders, CDLOD tile streaming, `big_space` per-tile anchoring, and the layered color pipeline; the "surface" projection of the terrain family. |
| **`lunco-obstacle-field`** | Procedural crater + rock field generation (with LOD) for rover testing. |
| **`lunco-experiments`** | Backend-agnostic experiment / batch-run registry: models a single Fast Run as a first-class artifact (params, bounds, trajectory) with `RunStatus` (`Pending`/`Queued`/`Running`/`Done`/`Failed`/`Cancelled`) and `RunBounds`; the sim backend plugs in via the `ExperimentRunner` trait, parallel runs schedule across a worker pool. |
| **`lunco-cosim`** | Multi-engine orchestration (Modelica, FMU, GMAT, Avian) via explicit input/output wiring (`SimConnection`), following FMI/SSP causality. |

---

## 3. Vessel Control & Hardware
The "Brains and Brawn" — Flight Software (FSW), On-Board Computer (OBC), mobility physics, and robotics assembly.

| Crate | Responsibility |
| :--- | :--- |
| **`lunco-mobility`** | Parameterized surface-vehicle physics: contact-plane raycast wheels (incl. leaning bikes), suspension, drive mixing, rocker-bogie differential. |
| **`lunco-robotics`** | High-level assembly logic and rover structural definitions (`assembler`). |
| **`lunco-avatar`** | Human-interaction layer: composable camera **rigs** (SpringArm, Orbit, FreeFlight, Surface) and control intents. (Camera *selection* / viewport lives in `lunco-usd-bevy` + `lunco-core::SceneViewport`.) |
| **`lunco-obc`** | Hardware interface emulation (ADC/DAC) between digital FSW registers and physical units. |
| **`lunco-fsw`** | Decentralized Flight Software architecture for coordinating vessel subsystems (GNC, Power, etc.). |
| **`lunco-hardware`** | Concrete physical actuators and sensors bridging `PhysicalPort` values to the `avian3d` physics engine. |
| **`lunco-controller`** | Translation of raw user input (Keyboard/Gamepad) into typed `VesselIntent` actions for FSW. Yields a vessel to its owning session (spec 034), so the human never fights an autopilot. |
| **`lunco-autopilot`** | Headless autonomous driver as a first-class actor: an `AiAgent` session that possesses + drives a vessel via `SetPorts` (spec 034). Multi-actor (each vessel → one owning session, human or autopilot). Behaviour is a `lunco-behavior` tree authored as DATA (`BehaviorSpec`, rhai/JSON — hot-swappable via `SetAutopilotBehavior`) with Rust nav-math leaves. No avatar/UI dep. |

---

## 4. USD Integration Layer
Modular bridge between OpenUSD and Bevy, covering visuals, physics, simulation metadata, and materials.

| Crate | Responsibility |
| :--- | :--- |
| **`lunco-usd`** | High-level USD orchestrator (`UsdPlugins`) and mapper for LunCo-specific engineering metadata (`lunco:*`). |
| **`lunco-usd-bevy`** | Core visual bridge (`UsdBevyPlugin`): maps USD hierarchy, shapes, transforms, and `timeSamples` animation to Bevy entities/components. Owns composition/flattening (`compose.rs`, `flatten_stage`), USD `def Camera` translation + rover-mounted camera followers (`camera.rs`, `camera_mount.rs`), and the single-authority viewport-camera reconciler + `SetActiveCamera` switch (`camera_switch.rs`). |
| **`lunco-usd-avian`** | Physics bridge (`UsdAvianPlugin`): maps `UsdPhysics` schemas (RigidBody, Colliders, all joint kinds + drive API) to Avian3D — the single home for joint construction. |
| **`lunco-usd-sim`** | Simulation-schema bridge (`UsdSimPlugin`): intercepts specialized vehicle/cosim schemas (e.g., PhysX Vehicles) and maps them to LunCo models. |
| **`lunco-materials`** | The one general self-describing `ShaderMaterial` (any `.wgsl` per-instance; params reflected from the shader's `struct Material`) for the USD rendering pipeline. |

---

## 5. Networking & API
External communication, ECS replication, telemetry extraction, and distributed attributes.

| Crate | Responsibility |
| :--- | :--- |
| **`lunco-networking`** | Multiplayer layer: transport-agnostic replication, authentication, and collaborative edit logs. Host-authoritative planes broadcast on connect + change: the **journal plane** (convergent op-log merge), the **scenario plane** (CID asset manifest + scenario sync), the **scripted-policy plane** (rhai merge/authorize/drive-kernel hooks distributed so every peer runs the identical one), and per-peer AOI snapshot routing. |
| **`lunco-api`** | Transport-agnostic API core: introspection-based command discovery and ULID entity registry. |
| **`lunco-telemetry`** | Generic reflection-based data extraction engine for "No-Code" telemetry mirroring. |
| **`lunco-attributes`** | String-based distributed tuning registry for mapping SysML paths to raw ECS memory. |

---

## 6. Workbench & UI Tools
The editor shell, visualization framework, generic 2D canvas, in-scene/sandbox editing tools, render look, and web boot.

| Crate | Responsibility |
| :--- | :--- |
| **`lunco-workbench`** | The IDE-like frame: docking engine, perspective presets, panel registration, and the reactive `Panel`/`PanelCtx` API. |
| **`lunco-ui`** | Reusable UI infrastructure: cached widgets, 3D world panels, command builders. |
| **`lunco-viz`** | Domain-agnostic visualization: `SignalRegistry`, LinePlots, and future 3D/Rerun bridges. |
| **`lunco-canvas`** | Stateful 2D scene editor substrate for diagrams and annotation overlays. |
| **`lunco-sandbox-edit`** | In-scene editing tools: spawn systems, transform gizmos, and inspector panels. |
| **`lunco-render`** | Shared render-look configuration: the single source of truth for lunar sun shadows (and future exposure/AA/sky look settings). |
| **`lunco-web`** | Shared web-frontend boot library for wasm apps: streaming loader + `WebReadyPlugin` (signals the HTML loader on first paint). |

---

## 7. Scripting & Modeling
Logic engines for dynamic simulation behavior, the tool registry, and industrial modeling.

| Crate | Responsibility |
| :--- | :--- |
| **`lunco-modelica`** | Modelica integration: AST-based editing, compilation via Rumoca, diagram visualization, and the Modelica workbench + worker pool. |
| **`lunco-scripting`** | Runtime-agnostic, language-neutral world bridge with **rhai** as the default (browser-capable) backend; Python is an optional one-shot-eval backend, Lua a reserved (unimplemented) backend id; logic providers cover scenarios and sequencing. |
| **`lunco-tools`** | Backend-agnostic tool trait + registry: a *tool* is a named, reusable bundle of callable functions whose implementation is pluggable (rhai/native/future). Deliberately dependency-free (rhai-free) — owns only the abstraction + global registry + discovery. |
| **`lunco-tools-rhai`** | rhai adapter binding for the `lunco-tools` registry: `RhaiTool` (source) + `NativeRhaiTool` (native Rust), and `refresh`, which binds every registered tool into a rhai `Engine` as a static module callable as `name::fn(...)`. |
| **`lunco-hooks`** | Language-agnostic hook registry: a *hook* is a named, deterministic-flagged decision point (`HookValue` in/out) whose implementation is pluggable. Backs the first-class policies — journal **merge** order, RBAC **authorize** gate, scripted **drive kernels** — as data, not Rust branches. Dependency-free (no rhai/bevy). |
| **`lunco-hooks-rhai`** | rhai backend for `lunco-hooks`: compiles a rhai `source` + `entry` fn and registers it under a hook id (`register_rhai_hook`), so any hook point can be authored in rhai and hot-replaced. |
| **`lunco-behavior`** | Dependency-free behaviour-tree kernel (mechanism split from any prelude): `Ctx`-driven tick-tree, HBT-ready. Not yet wired into a runtime. |
| **`lunco-tutorial`** | Data-driven, rhai-scripted tutorial shell: a tutorial *is* a USD scene + `*.rhai` orchestrator using the ordinary scripting substrate (task sequencer, `mission(me)`, `cmd:*` bus, HUD). Adds only the launcher panel, `StartTutorial`/`SkipTutorial`/progress, and the `SubsystemToggles` progressive-fidelity switch. UI-gated; holds no sim state (spec 011). |

---

## 8. Applications
Primary entry points and simulation assembly targets.

| Crate | Binary | Responsibility |
| :--- | :--- | :--- |
| **`luncosim`** | `luncosim` | The flagship windowed simulator: celestial bodies + ephemeris, solar-system-scale `big_space`, orbital camera, and the full FSW/hardware/mobility/robotics/avatar stack under the workbench. |
| **`lunco-sandbox`** | `sandbox`, `joint_minimal` | Ground-physics test bed (ground mobility + physics, loaded from USD): USD scene + Avian physics + the sandbox edit tools. A composition root (`SandboxCorePlugin` + optional `SandboxUiPlugin`/`SandboxHeadlessPlugin`) shared by the `sandbox` GUI and `sandbox-server` headless binaries. |
| **`lunco-sandbox-server`** | `sandbox-server` | Headless launcher for the sandbox (no winit/egui) with the API + networking host. Its own crate purely so it can default to headless. |
| **`lunco-modelica`** | `lunica`, `lunica_worker`, `msl_indexer` | The Modelica workbench app + its wasm worker and MSL index builder. |

> Other binaries: `build_msl_assets` (`lunco-assets`), `net_smoke` (`lunco-networking`).

---

## Detailed Crate Responsibilities

Below, selected crates whose responsibilities benefit from extra detail. (Crates not listed here are adequately described by the tables above.)

### Core Foundation

**`lunco-core`**
The bedrock of the simulation. Defines `DigitalPort`/`PhysicalPort` primitives for software/hardware interaction, the typed `Mutation<P>` command substrate (replacing the old string-based `CommandMessage`), `SimTick`, and the `ComponentGraph` canonical data structure for all 2D diagram visualizations (Modelica, FSW, SysML).

**`lunco-time`**
The unified mission-time spine (architecture doc 19). Owns `MissionClock`/`TimeTransport`/`WorldTime` (the world animation clock that also gates physics via `Time<Virtual>`), the `TimeDomain` clock tree (`Playback`, `TimeBinding`, `ResolvedDomains`) with the `AnimationPreview` domain + `ControlAnimation` transport, and the `scales` projection layer (UTC↔TAI↔TT↔TDB, sidereal) over `celestial-time`. **All time-scale/JD nuance lives here; consumers delegate.**

**`lunco-doc`**
Foundation for structured, mutable artifacts (Modelica, USD, etc.) with built-in undo/redo logic. Defines the `DocumentHost` container and the atomic `DocumentOp` pattern for state mutation and inversion.

**`lunco-storage`**
I/O abstraction layer providing a unified `Storage` trait for reading and writing handles. Supports native FS and memory (for tests), with architectural stubs for future browser (OPFS/IndexedDB) and remote backends.

**`lunco-assets`**
Unified asset management system. Resolves shared cache locations across git worktrees, downloads external assets via `Assets.toml` with SHA-256 verification, and handles texture pre-processing (resize/convert).

**`lunco-cache`**
Generic resource cache with in-flight deduplication. Ensures that concurrent requests for expensive resources (like large USD stages or Modelica ASTs) collapse into a single background task, sharing the resulting parsed data.

**`lunco-theme`**
Centralized design tokens based on the Catppuccin palette. Provides semantic tokens for general UI (accent, success, error) and schematic-specific colors for diagram wires and badges, ensuring visual consistency across all panels.

**`lunco-settings`**
Centralised user-settings system. Persists one namespaced JSON file (`~/.lunco/settings.json`) with auto-save on change, giving subsystems a single place to read and write per-user preferences.

**`lunco-command-macro`**
Procedural macros for the typed command system. Provides the `#[Command]`, `#[on_command]`, and `register_commands!` macros used to simplify the creation and registration of simulation actions.

**`lunco-doc-bevy`**
Bevy ECS integration for the Document System. Provides lifecycle events (Opened, Changed, Saved, Closed), the `JournalResource` Bevy wrapper around the canonical Twin journal in `lunco-twin-journal`, and a `BevyJournalSink` for replaying remote-author entries through the same store. Lifecycle observers translate `EventOrigin` into `AuthorTag` and record `EntryKind::Lifecycle` entries directly into the canonical journal; structural ops are recorded by domain mutation paths.

**`lunco-twin-journal`**
Canonical, append-only, Twin-scoped record of every change. Immutable entries keyed by `(author, lamport)` (yrs-compatible), DAG parent links, optional `change_set` grouping for atomic undo, and `EntryKind::{Op, TextEdit, Snapshot, Lifecycle}`. Higher-level: `Stream` + `Composition`, `Branch`, `Marker`, and `UndoManager` with `UndoScope::{Document, Twin}`. Pure Rust, headless, no Bevy dep.

---

### Simulation Engine

**`lunco-celestial`**
Orbital mechanics and solar-system simulation spine. Handles body-fixed rotation, gravity vectors, and the Sphere of Influence (SOI) system for automatic coordinate frame transitions between bodies; the sun light is driven from ephemeris. Owns the `EphemerisResource` abstraction; the concrete high-fidelity provider lives in `lunco-celestial-ephemeris`.

**`lunco-celestial-ephemeris`**
Concrete high-fidelity ephemeris provider for `lunco-celestial`. The heavy half of the celestial split and the one place `celestial-time` is allowed: pulls in `celestial-ephemeris` (VSOP2013 + ELP/MPP02), `celestial-time`, and `celestial-core` (none of which build on Windows MSVC). Apps that need real planetary positions add `EphemerisPlugin`, which overwrites the default `EphemerisResource`.

**`lunco-environment`**
Position-dependent environmental state (gravity, atmosphere, radiation, etc.). Uses a provider-consumer pattern to compute local conditions for each entity based on its proximity to celestial bodies and their specific environment models.

**`lunco-terrain-core`**
Projection-agnostic terrain LOD spine. Provides quadtree-CDLOD tile selection, tile-grid math, and the `HeightSource` trait. Pure (std + serde only) with no bevy/avian/DEM/sphere dependency, so it is shared by both the planar DEM streamer (`lunco-terrain-surface`) and the cube-sphere planetary tiler (`lunco-terrain-globe`).

**`lunco-terrain-globe`**
Whole-body cube-sphere terrain tiling at orbital/planetary scale: quadtree-CDLOD globe, avian heightfield collision ring, and `big_space` anchoring. The "globe" projection of the terrain family; pairs with `lunco-terrain-surface` (local DEM ground) over the shared `lunco-terrain-core` LOD spine.

**`lunco-terrain-surface`**
Local high-detail DEM ground terrain at surface scale: heightfield colliders, CDLOD tile streaming, `big_space` per-tile anchoring, and the layered color pipeline. The "surface" projection of the terrain family; pairs with `lunco-terrain-globe` over the shared `lunco-terrain-core` LOD spine.

**`lunco-obstacle-field`**
Procedural crater + rock field generation for rover testing. Produces LOD-aware obstacle distributions usable as mobility test grounds.

**`lunco-cosim`**
Multi-engine simulation orchestrator. Wires named outputs from one engine (e.g., Modelica) to named inputs of another (e.g., Avian physics) via `SimConnection` components, following FMI/SSP causality. Owns the built-in `PortRegistry` backends: rigid-body state (position/velocity/attitude/rates + force/torque/mass-props), revolute/prismatic joint motors (`angle`/`displacement`), and USD-authored sensors (IMU, range, contact). Avian forces are applied through the typed-port spec table (`AvianGroup`/`AvianPort` + `PendingForces`), not a bespoke `AvianSim` struct.

**`lunco-experiments`**
Backend-agnostic experiment / batch-run registry. Models a single Fast Run as a first-class artifact (params, bounds, trajectory), decoupled from any one solver via the `ExperimentRunner` trait that another crate plugs in. `RunStatus` is `Pending → Queued → Running { t_current } → Done { wall_time_ms } | Failed { error, partial } | Cancelled`; `RunBounds` carries start/stop/interval; parallel runs schedule across a worker pool.

### Vessel Control & Hardware

**`lunco-mobility`**
Physics models for surface mobility and traction — the parameterized substrate (a vehicle is a USD file, not a Rust struct). Raycast wheel model with contact-plane traction (supports leaning single-track bikes), suspension (spring-damper), a data-driven `DriveMix` allocated by a named kernel from `lunco-core`'s `ControlKernelRegistry` (`skid`/`linear`), and a soft rocker-bogie `DifferentialCoupling`.

**`lunco-robotics`**
High-level vessel assembly and spawning logic. Orchestrates the composition of complex robots from constituent parts, linking chassis, wheels, software, and sensors into a cohesive simulation unit.

**`lunco-avatar`**
Human-interaction layer. Provides composable camera **rigs** (SpringArm, Orbit, FreeFlight, Surface) with smooth jitter-free transitions and coordinate-grid awareness for avatar-based exploration of celestial bodies. The rigs decide *how* a camera moves; *which* camera the viewport shows is owned by the reconciler in `lunco-usd-bevy` (they compose — possession changes the avatar camera's rig without changing the active view).

**`lunco-obc`**
On-Board Computer emulation. Acts as the signal-processing bridge (DAC/ADC) between digital Flight Software registers (`i16`) and physical hardware units (`f32`), emulating hardware quantization and scaling.

**`lunco-fsw`**
Decentralized Flight Software architecture. Manages vessel subsystems as independent ECS entities communicating via an asynchronous `CommandMessage` fabric, mapping semantic SysML names to hardware entities.

**`lunco-hardware`**
Physical actuator and sensor implementations. Bridges `PhysicalPort` values to the `avian3d` physics engine, providing concrete motor, brake, and sensor components that interact with the simulation world.

**`lunco-controller`**
Input mapping and translation. Converts raw human-interface device inputs (Keyboard, Gamepad, Mouse) into abstract `VesselIntent` actions and typed command events for consumption by Flight Software.

---

### USD Integration Layer

**`lunco-usd`**
High-level USD orchestrator (`UsdPlugins`) and engineering metadata bridge. Maps LunCo-specific metadata (`lunco:*` namespace) from USD stages to Bevy components, enriching 3D models with simulation-critical data like Ephemeris IDs.

**`lunco-usd-bevy`**
Core OpenUSD visual bridge. Maps USD prim hierarchies, shapes, and transforms into Bevy entities/components, decodes the full xform-op stack (`local_transform_at`), and drives authored `timeSamples` animation (`sample_usd_animation`). Composition/flattening lives here (`compose.rs`, `flatten_stage`) — there is no separate `lunco-usd-composer` crate. Also owns the **camera pipeline**: USD `def Camera` → inactive `Camera3d` (`camera.rs`), rover-mounted grid-direct camera followers (`camera_mount.rs`), and the **single-authority viewport-camera reconciler** + `SetActiveCamera`/`KeyC` switch that actuates `lunco_core::SceneViewport` (`camera_switch.rs`). See [`17-view-and-intent.md §6`](architecture/17-view-and-intent.md).

**`lunco-usd-avian`**
Physics bridge for OpenUSD (`UsdAvianPlugin`). Maps `UsdPhysics` schemas — rigid bodies + mass-properties, all collider shapes, and **all joints** (revolute/prismatic/fixed/spherical/distance, D6-reduced) with `UsdPhysicsDriveAPI` motor drive — to Avian3D. The single home for Avian joint construction (incl. the programmatic wheel hinge).

**`lunco-usd-sim`**
Specialized simulation metadata bridge. Intercepts complex industry-standard vehicle schemas (like NVIDIA PhysX Vehicles) and substitutes them with optimized LunCo simulation models (e.g., Raycast wheels).

**`lunco-materials`**
Material library for the USD pipeline. Provides one general self-describing `ShaderMaterial`: any `.wgsl` runs per-instance, its parameters reflected from the shader's `struct Material` and authored from USD `primvars` (e.g. `solar_panel.wgsl`, `blueprint.wgsl`, `regolith.wgsl`). No bespoke Rust material type per look.

---

### Networking & API

**`lunco-networking`**
Transparent multiplayer shim. Handles ECS replication, transport abstraction (UDP/WebSockets), and collaborative editing via a verified `AuthorizedCommand` flow and Lamport-ordered `EditLog` for history and undo.

**`lunco-api`**
Transport-agnostic API core. Exposes simulation state and command discovery via HTTP, mapping ULID-based stable entity IDs to process-local Bevy entities for external control and inspection.

**`lunco-telemetry`**
Reflection-based data extraction engine. Automatically samples and standardizes internal physics and software values for broadcast to external monitoring systems or Mission Control bridges (YAMCS/XTCE).

**`lunco-attributes`**
Distributed tuning registry. Allows external processes to mutate simulation state using string-based paths (e.g., `"vessel.rover1.suspension.k"`) that map 1:1 with SysML architectural models.

---

### Workbench & UI Tools

**`lunco-workbench`**
The engineering-IDE shell. Handles the docking engine (tabs, splits), perspective presets (Build, Simulate), and the Twin Browser, acting as the primary host for all other domain-specific UI panels.

**`lunco-ui`**
Reusable UI infrastructure. Provides the `WidgetSystem` for cached ECS widgets, the `CommandBuilder` for action-driven interaction, and `WorldPanel` for 3D in-scene UI elements attached to entities.

**`lunco-viz`**
Domain-agnostic visualization framework. Collects simulation data into a `SignalRegistry` and renders it via `Visualization` kinds (LinePlots, Gauges) into various view targets like 2D panels or the 3D viewport.

**`lunco-canvas`**
2D scene editor substrate. Provides the stateful viewport and tool foundation for diagramming and node-based editing, powering the Modelica diagram editor and other schematic-based tools.

**`lunco-sandbox-edit`**
In-scene editing toolkit for the 3D viewport. Implements click-to-place spawning, transform gizmos for manipulation, and inspector panels for real-time property editing during simulation assembly.

**`lunco-render`**
Shared render-look configuration. The single source of truth for lunar sun shadows, and the future home for exposure, anti-aliasing, and sky look settings, keeping render tuning out of individual app crates.

**`lunco-web`**
Shared web-frontend boot library for the wasm apps. Provides the streaming loader (`web/lunco-boot.{js,css}`) plus `WebReadyPlugin`, which signals the HTML loader once Bevy paints its first frame.

---

### Scripting & Modeling

**`lunco-modelica`**
Modelica language integration. Provides AST-based editing, compilation via Rumoca, and interactive diagramming, allowing complex industrial models to drive simulation entities and vessel subsystems.

**`lunco-scripting`**
Language-neutral world bridge for dynamic logic providers. The default (and only fully-wired) backend is **rhai** — browser-capable and enabled by the default `rhai` feature; build with `--no-default-features` for a script-free build. The bridge exposes ECS verbs and a native `ValueBuilder` (no JSON on the read path) over which each runtime is a thin binding. Python is an optional backend used for one-shot snippet evaluation only; Lua is a reserved (not yet implemented) backend id. rhai also funnels the `lunco-tools` registry into the engine via `lunco-tools-rhai`.

**`lunco-tools`**
Backend-agnostic tool registry. A *tool* is a named, reusable bundle of callable functions — a library of selection/behaviour policy a scenario can call as `name::fn(...)`. A tool's implementation is pluggable (rhai source, native Rust, or future runtimes); this crate is deliberately dependency-free and owns only the abstraction, the global registry, and discovery, so non-rhai consumers can enumerate/describe tools without pulling rhai in.

**`lunco-tools-rhai`**
rhai adapter for the `lunco-tools` registry. Provides the two concrete `Tool` impls scenarios use today — `RhaiTool` (rhai source) and `NativeRhaiTool` (native Rust functions) — and `refresh`, which binds every registered tool into a rhai `Engine` as a static module so it is callable as `name::fn(...)` from anywhere, including inside `on_tick`. Tools authored in other runtimes are exposed to rhai as a `NativeRhaiTool`.

---

### Applications

**`luncosim`**
The flagship windowed application and full lunar-mission simulator. Assembles celestial bodies + ephemeris, solar-system-scale `big_space`, an orbital camera (auto-focus Earth), and the whole FSW / hardware / mobility / robotics / avatar stack under the workbench. (cf. `sandbox` = ground-physics test bed, `lunica` = Modelica workbench.)

**`lunco-sandbox`**
The LunCo sandbox application — ground mobility + physics, loaded from USD (binary `sandbox`). A composition root rather than a UI host: `SandboxCorePlugin` (headless-safe sim/physics/cosim/USD/networking/API) plus an optional `SandboxUiPlugin` (egui workbench, windowed) or `SandboxHeadlessPlugin`. Assembles the USD scene, Avian physics, and the in-scene edit tools, and is the single shared entry point for both the `sandbox` GUI and `sandbox-server` headless binaries. (Historically this crate's README mis-titled it "lunco-client"; the package name is `lunco-sandbox`.) The full mission simulator is the separate `luncosim` crate.

**`lunco-sandbox-server`**
Headless launcher for the sandbox — the same app as `sandbox`, built without the GUI (no winit/egui) and with the API + networking host enabled. Exists as its own crate purely so it can default to headless (Cargo default features are per-package).
