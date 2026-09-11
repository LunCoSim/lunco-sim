# LunCoSim Crates Index

This document provides a comprehensive index of all crates in the LunCoSim workspace, categorized by their functional domain and architectural responsibility. It serves as a navigation guide for both developers and AI agents.

---

## 1. Workspace & Core Foundation
Low-level primitives, document/journal systems, time, and cross-cutting concerns (storage, assets, theming, settings).

| Crate | Responsibility |
| :--- | :--- |
| **`lunco-core`** | Core primitives (`Port`, the typed `Mutation<P>` command substrate, `SimTick`), f64 coordinate/frame helpers, the persistent BigSpace world shell, typed scene transitions, `SceneMountState` and the `SceneTeardown` schedule, the `SceneViewport` (active-camera binding), canonical diagram data types, shared human-readable entity labels, and shared terminal runtime faults/fixed-step coupling state. Core carries no vehicle-specific motion policy. |
| **`lunco-command-macro`** | Procedural macros for the typed command system (`#[Command]`, `#[on_command]`, `register_commands!`; re-exported by `lunco-core`). |
| **`lunco-workspace`** | Headless editor session management: open Twins, active documents, perspectives, recents, and generic active-Twin setting persistence (`SetTwinSetting` / `ResetTwinSetting`). |
| **`lunco-workspace-api`** | API adapter for Workspace-owned queries (`ListOpenDocuments`, `ListRecentFiles`, `ListTwin`), installable by windowed, headless, or offscreen hosts without making the data-only Workspace crate depend on the API layer. |
| **`lunco-twin`** | The simulation unit on disk: folder structure, `twin.toml` manifest parsing, generic scalar `[settings]`, and file indexing. |
| **`lunco-twin-journal`** | Canonical Twin-scoped op log: Lamport-ordered entries, DAG parents (for future merges), Streams + Composition, ChangeSets, Markers (named milestones), Branches, `UndoManager`. CRDT-shapable schema; in-memory backend today, yrs-swap-ready. |
| **`lunco-doc`** | Foundation for structured artifacts (Modelica, USD, SysML): process-wide live document handle allocation, the `DocumentHost` container and atomic `DocumentOp` pattern with built-in undo/redo. |
| **`lunco-doc-bevy`** | Bevy ECS integration for the Document System: lifecycle events, `JournalResource` (Bevy wrapper around the canonical Twin journal), `BevyJournalSink` for remote-replay, `EditorIntent` keybindings, `Presence` collab seed. |
| **`lunco-storage`** | I/O abstraction layer (`Storage` trait — Native FS, Memory, future WASM/Remote backends). The single write path; raw `std::fs` is disallowed. |
| **`lunco-assets`** | Unified asset management: cache resolution across worktrees, versioned downloads (`Assets.toml`, SHA-256), and texture processing. |
| **`lunco-hash`** | Hashing substrate: Fast tier (FNV-1a) for change/cache keys and CID tier (CIDv1 raw+sha2-256) for on-disk/on-wire content-addressing. Draws a firewall between ephemeral process keys and cross-peer persisted content. |
| **`lunco-precompute`** | Content-addressed precompute disk cache (`bake_or_load`): runs expensive pure functions once, persists results keyed by content hash (via `lunco-hash` + `lunco-storage`), and loads them on subsequent runs/peers. |
| **`lunco-settings`** | Centralised user-settings: one JSON file (`<OS config dir>/lunco/settings.json`), namespaced sections, auto-persist on change; also owns the shared `DownloadSettings` retry/backoff policy. |
| **`lunco-theme`** | Centralized design tokens (Catppuccin-based) for consistent UI across all panels and domains. |
| **`lunco-time`** | Unified mission-time spine (architecture doc 19): `MissionClock`/`TimeTransport`/`WorldTime`, the `TimeDomain` clock tree + animation transport, and the `scales` projection layer over `celestial-time`. |
| **`lunco-worker-transport`** | Generic Web Worker pool transport (wasm-only): spawn / lazy-grow, boot wire-id handshake, byte + Transferable-`ArrayBuffer` post, crash respawn. Payload-agnostic (the caller supplies decode/route callbacks); shared by the Modelica Fast-Run workers and the DEM bake worker so neither reimplements the plumbing. |
| **`lunco-status-core`** | Renderer-independent lifecycle, progress, and status infrastructure: `StatusBus`, scoped busy handles, tracked tasks, discrete diagnostics, and telemetry mirroring. Consumers such as the workbench status bar, busy widgets, and headless diagnostics read the same contract. |

---

## 2. Simulation Engine
The "Laws of Nature" — celestial mechanics, environmental state, terrain, obstacle fields, batch experiments, and co-simulation orchestration.

| Crate | Responsibility |
| :--- | :--- |
| **`lunco-celestial`** | Orbital mechanics, canonical body catalog/NAIF identities, typed analytical frame transforms, named BigSpace frame projection, gravity, body rotation, and automatic SOI/frame transitions; sun-light driven from ephemeris. |
| **`lunco-celestial-ephemeris`** | Concrete high-fidelity ephemeris provider for `lunco-celestial` (VSOP2013 + ELP/MPP02 via `celestial-ephemeris`); the heavy, non-Windows-MSVC half of the celestial split and the one place `celestial-time` is allowed. |
| **`lunco-environment`** | Per-entity position-dependent environment state (atmosphere, radiation, local gravity). |
| **`lunco-terrain-core`** | Projection-agnostic terrain LOD spine: quadtree-CDLOD selection, tile-grid math, and the `HeightSource` trait. Pure (std + serde), shared by both the planar DEM streamer and the cube-sphere planetary tiler. |
| **`lunco-terrain-globe`** | Whole-body cube-sphere terrain tiling (orbital/planetary scale): quadtree-CDLOD globe, avian heightfield collision ring, `big_space` anchoring; the "globe" projection of the terrain family over the shared `lunco-terrain-core` LOD spine. |
| **`lunco-terrain-surface`** | Local high-detail DEM ground terrain (surface scale): heightfield colliders, CDLOD tile streaming, `big_space` per-tile anchoring, and the layered color pipeline; the "surface" projection of the terrain family. |
| **`lunco-terrain-bake`** | Pure (bevy/avian-free) DEM bake pipeline shared verbatim by the native async task and the wasm Web Worker: GeoTIFF decode → crop/resample → crater stamp → `HeightGrid`. Owns the `dem_worker` companion binary + its main-thread client (over `lunco-worker-transport`), moving the ~40 MB decode + crater stamp off the page's main thread on web (coarse-then-full progressive). |
| **`lunco-geotiff`** | The **geo** half of a GeoTIFF: GeoKey/tie-point/pixel-scale read and write, shared by the writer (`lunco-assets`) and the reader (`lunco-terrain-bake`). A raster states its own extent and projection; nothing restates it in a sidecar. See `docs/architecture/57-dem-georeferencing.md`. |
| **`lunco-physics`** | The physics **readiness, backend-admission, and solver-configuration owner** — `avian_backend` is the single numeric/shape contract for Avian's f64-to-f32 points, AABBs, and compound-child structure; lifecycle bridges decide when to admit it. It decides whether the world is safe to integrate and installs the single cross-platform Avian substep contract. A DEM still baking or a collider ring not yet paged in suspends integration without touching the user's transport clock, so a `Dynamic` body cannot free-fall through a collider that does not exist yet. |
| **`lunco-obstacle-field`** | Procedural crater + rock field generation (with LOD) for rover testing. |
| **`lunco-experiments`** | Backend-agnostic experiment / batch-run registry: models a single Fast Run as a first-class artifact (params, bounds, trajectory) with `RunStatus` (`Pending`/`Queued`/`Running`/`Done`/`Failed`/`Cancelled`) and `RunBounds`; the sim backend plugs in via the `ExperimentRunner` trait, parallel runs schedule across a worker pool. |
| **`lunco-cosim`** | Multi-engine orchestration (Modelica, FMU, GMAT, Avian) via explicit causal input/output wiring (`SimConnection`), following FMI/SSP. Dynamic feedback is exchanged once per fixed step; acausal islands require a typed backend partition and are not guessed from graph cycles. Port backends (incl. the `piloted` control-authority sensor derived from possession) resolve every exposed value by name. |

---

## 3. Vessel Control & Hardware
The "Brains and Brawn" — Flight Software (FSW), On-Board Computer (OBC), mobility physics, and robotics assembly.

| Crate | Responsibility |
| :--- | :--- |
| **`lunco-mobility`** | Parameterized surface-vehicle physics: contact-plane raycast wheels (incl. leaning bikes), suspension, drive mixing, rocker-bogie differential. |
| **`lunco-avatar`** | Human-interaction layer: composable camera **rigs** (SpringArm, Orbit, FreeFlight, Surface) and control intents. (Camera *selection* / viewport lives in `lunco-usd-bevy-camera` + `lunco-core::SceneViewport`.) |
| **`lunco-hardware`** | Concrete physical actuators and sensors bridging `Port` values to the `avian3d` physics engine. |
| **`lunco-controller`** | Owns the persisted `InputBindingsSettings` keymap and translates resolved raw user input (Keyboard/Gamepad/Mouse) into typed `UserIntent` actions for FSW. Yields a vessel to its owning session (spec 034), so the human never fights an autopilot. |
| **`lunco-autopilot`** | Headless autonomous driver as a first-class actor: an `AiAgent` session that possesses + drives a vessel via `SetPorts` (spec 034). Multi-actor (each vessel → one owning session, human or autopilot). Behaviour is a `lunco-behavior` tree authored as DATA (`BehaviorSpec`, rhai/JSON — hot-swappable via `SetAutopilotBehavior`) with Rust nav-math leaves. No avatar/UI dep. |

---

## 4. USD Integration Layer
Modular bridge between OpenUSD and Bevy, covering visuals, physics, simulation metadata, and materials.

| Crate | Responsibility |
| :--- | :--- |
| **`lunco-usd-core`** | Headless USD document, authoring, operation, schema, unit-conversion, and asset-closure substrate. No runtime, physics, rendering, or UI. |
| **`lunco-usd`** | High-level runtime USD orchestrator (`UsdPlugins`) and mapper for LunCo-specific engineering metadata (`lunco:*`). |
| **`lunco-usd-geometry`** | Render-free NURBS evaluators, trimmed-domain tessellation, and rotation-minimizing curve-sweep mesh data. Isolates heavy numeric geometry dependencies from the USD stage loader. |
| **`lunco-usd-bevy-core`** | Headless composed-USD reader/view, stage composition, prepared stage assets, canonical live-stage ownership, authored-layer readers, instance identity, send-safe projection plans, program/variant resolution, material binding, transform decoding, and unit conversion. Uses Bevy's asset/ECS substrate but has no mesh, light, camera, renderer, window, or UI projection. |
| **`lunco-usd-bevy-scene`** | Render-free Bevy scene contract shared by visual and domain projections: `UsdPrimPath`, scene/revision lifecycle markers, preview/ancestry ownership, and canonical USD primitive/mesh geometry readers. It depends on the core reader and has no visual adapter or renderer dependency. |
| **`lunco-usd-bevy-camera`** | Render-free USD camera adapter: standard `UsdGeomCamera` projection intent, mounted/cinematic camera pose, camera-track selection, and the single-authority viewport-camera reconciler. It depends on the core reader and scene contract, not on visual projection. |
| **`lunco-usd-bevy`** | Visual Bevy adapter (`UsdBevyPlugin`): projects USD hierarchy, shapes, transforms, materials, and `timeSamples` animation into Bevy entities/components on top of `lunco-usd-bevy-core`. Owns mesh/lathe/curve projection and lighting; installs the independent camera adapter but does not own camera mechanisms. |
| **`lunco-usd-avian`** | Physics bridge (`UsdAvianPlugin`): maps `UsdPhysics` schemas (RigidBody, Colliders, all joint kinds + drive API) to Avian3D — the single home for joint construction. |
| **`lunco-usd-sim`** | Simulation-schema bridge (`UsdSimPlugin`): intercepts specialized vehicle/cosim schemas (e.g., PhysX Vehicles) and maps them to LunCo models. Full USD→Bevy→Avian→simulation projection tests live here; direct Avian bridge mechanics stay in `lunco-usd-avian`. |
| **`lunco-usd-terrain`** | Terrain bridge: projects authored terrain prims into `lunco-terrain-surface`'s `DemTerrainRequest` + composable `TerrainLayerStack` (craters / rocks / edits), and carries hand edits back as journaled, undoable USD ops on the document's **runtime** layer. Standard `UsdShade` owns terrain material intent. |
| **`lunco-scene-commands`** | The scene/document **command layer**: every runtime mutation — spawn, move, delete, set-property, shader edit — authored as journaled USD ops on the open document's runtime layer. One path for all four callers (rhai, HTTP API, peer over the wire, editor gizmo); an edit that bypasses it escapes save, journal, undo and replication. Validation is a separate production dependency so command changes do not rebuild the validator. |
| **`lunco-scene-validation`** | Production asset, loaded-stage, and Twin pre-flight: `ValidateAsset`, `ValidateTwin`, live `RunLint`, USD lint-fact aggregation, and Twin namespace inspection. It owns composition/parse/lint integration while `lunco-scene-commands` owns scene mutation and catalog commands. |
| **`lunco-materials`** | Shader appearance **intent**, render-free: `ShaderLook` (`.wgsl` path + open `dyn_params` + texture layers), the WGSL-reflected param schema, the CDLOD vertex attribute. Names no material type. |

---

## 5. Networking & API
External communication, ECS replication, telemetry extraction, and distributed attributes.

| Crate | Responsibility |
| :--- | :--- |
| **`lunco-networking`** | Multiplayer layer: transport-agnostic replication, authentication, and collaborative edit logs. Host-authoritative planes broadcast on connect + change: the **journal plane** (convergent op-log merge), the **scenario plane** (CID asset manifest + scenario sync), the **scripted-policy plane** (rhai merge/authorize/drive-kernel hooks distributed so every peer runs the identical one), and per-peer AOI snapshot routing. |
| **`lunco-api`** | Transport-agnostic API core: introspection-based command discovery and ULID entity registry. |
| **`lunco-telemetry`** | Telemetry channels: per-channel rate + deadband, bound to a `TimeDomain` (so pause/warp come free), retained in `lunco-signal`'s ring buffer, plus the OpenMCT-shaped query surface (catalog / history / recording). |
| **`lunco-signal`** | The signal DATA model — `SignalRegistry`, `SignalRef`, `ScalarHistory`, and the backend-neutral `SimRegistry`/`SimStream` snapshot publication path. **Render-free by construction**: split out of `lunco-viz` (which links bevy_egui → bevy_render) so a headless run can retain history without a GPU stack. `lunco-viz` re-exports the signal registry. |

---

## 6. Workbench & UI Tools
The editor shell, visualization framework, generic 2D canvas, in-scene/luncosim editing tools, render look, and web boot.

| Crate | Responsibility |
| :--- | :--- |
| **`lunco-workbench-core`** | Renderer-independent workbench contracts: `Panel`/`PanelCtx`, instance tabs, perspective layout plans, menu contributions, and the published `WorkbenchSnapshot`. It uses the Bevy ECS substrate and egui types but does not pull `bevy_render`, `bevy_egui`, `egui_dock`, storage, or window/render services. |
| **`lunco-workbench`** | The concrete IDE-like shell: `egui_dock` layout materialization, `bevy_egui` rendering, panel registration, persistence, viewport integration, built-in browser panels, and shell-only commands/widgets. It publishes `WorkbenchSnapshot`, consumes `lunco-workbench-core`, and renders status data supplied by `lunco-status-core`. |
| **`lunco-ui`** | Reusable UI infrastructure: cached widgets, 3D world panels, command builders. |
| **`lunco-viz`** | Domain-agnostic visualization: `SignalRegistry`, LinePlots, and future 3D/Rerun bridges. |
| **`lunco-canvas`** | Stateful 2D scene editor substrate for diagrams and annotation overlays. |
| **`lunco-luncosim-edit`** | In-scene editing tools: spawn systems, transform gizmos, and inspector panels. |
| **`lunco-render`** | Appearance **intent**, render-free: `PbrLook`, `SceneCamera`, `WorldLabel`, sun/shadow look. Names `Mesh3d`, never `MeshMaterial3d`. |
| **`lunco-render-bevy`** | The **only** crate that names `bevy_pbr`. Binds the intent (`PbrLook`/`ShaderLook`/`SceneCamera`/`WorldLabel`) to real materials & cameras; owns `ShaderMaterial`. Headless never adds it. |
| **`lunco-web`** | Shared web frontend for wasm apps: streaming loader, `WebReadyPlugin`, and the HTML/CSS/Rhai tool host routed through `lunco_rhai`. |

---

## 7. Scripting & Modeling
Logic engines for dynamic simulation behavior, the tool registry, and industrial modeling.

| Crate | Responsibility |
| :--- | :--- |
| **`lunco-modelica-core`** | Headless Modelica domain runtime: document editing, Rumoca compilation, simulation sessions, worker transport, MSL indexing, and API-facing commands/queries. It has no workbench, egui, tutorial, or UI dependency. |
| **`lunco-modelica-ui`** | Modelica workbench UI and `lunica` application facade. It adapts core state to workbench contexts and owns Modelica panels, diagrams, plots, onboarding, and editor presentation; it has no tutorial catalog or lifecycle. |
| **`lunco-modelica-ast`** | Pure Modelica source boundary: BOM normalization, strict/recovering Rumoca parse wrappers, AST interface/component extraction, shared expression/description display projections, and Modelica lint facts. It has no Bevy, UI, worker, storage, or solver ownership; authored lint policy remains in `assets/scripting/policy/lint_modelica.rhai`. |
| **`lunco-scripting`** | Runtime-agnostic, language-neutral world bridge with **rhai** as the default (browser-capable) backend; Python is an optional one-shot-eval backend, Lua a reserved (unimplemented) backend id; logic providers cover scenarios and sequencing. Rhai can read/write generic active-Twin settings and read named engine exposures without per-setting bindings. |
| **`lunco-tools`** | Backend-agnostic, dependency-free tool trait + registry: a *tool* is a named, reusable bundle of callable functions whose implementation is pluggable (rhai/native/future). Owns the bevy-free `Tool` trait (discovery + `as_any` downcast) + global registry + discovery. Behaviour-tree execution lives in `lunco-tools-bevy`. |
| **`lunco-tools-rhai`** | rhai adapter binding for the `lunco-tools` registry: `RhaiTool` (source) + `NativeRhaiTool` (native Rust), and `bind_registered_tools`, which binds every registered tool into a rhai `Engine` as a static module callable as `name::fn(...)`. |
| **`lunco-tools-bevy`** | Bevy dispatch adapter for `lunco-tools` — the behaviour-tree execution half. Defines a bevy-aware `ExecutableTool` supertrait + `ClosureTool` (a closure that triggers its typed command directly via `&mut World`, no JSON/reflect). Observes `ToolFired`, downcasts to `ExecutableTool`, runs it. Instruments register via `register_closure_tool`. |
| **`lunco-hooks`** | Language-agnostic hook registry: a *hook* is a named, deterministic-flagged decision point (`HookValue` in/out) whose implementation is pluggable. Backs first-class policies — journal **merge** order, RBAC **authorize** gate, and authored actuation policies — as data, not Rust branches. Dependency-free (no rhai/bevy). |
| **`lunco-hooks-rhai`** | rhai backend for `lunco-hooks`: compiles a rhai `source` + `entry` fn and registers it under a hook id (`register_rhai_hook`), so any hook point can be authored in rhai and hot-replaced. |
| **`lunco-lint`** | Universal lint substrate: `LintFinding`/`LintReport` and `run_lint(domain, facts)`, which asks the `lint.<domain>` hook what is wrong with a domain's FACTS. Rules are authored (`assets/scripting/policy/lint_<domain>.rhai`), never compiled here — this crate names no domain and knows nothing about USD, rhai or Modelica. Nothing lints on load; `RunLint` and `ValidateAsset` in `lunco-scene-validation` are the two entry points. See `docs/architecture/lint-substrate.md`. |
| **`lunco-behavior`** | Dependency-free behaviour-tree kernel (mechanism, no bevy/avian/rhai): `Ctx`-driven tick-tree — composites (`Sequence`/`Selector`/`Parallel`), reactive composites (`ReactiveSequence`/`ReactiveSelector`, guards re-checked every tick), loops (`Repeat`/`Retry`), and decorators (`Invert`/`Force`). Consumed by `lunco-autopilot`, which authors trees as data (`BehaviorSpec`) and adds clock/pose leaves. Node catalogue: [docs/behaviour-trees.md](./behaviour-trees.md). |

---

## 8. Applications
Primary entry points and simulation assembly targets.

| Crate | Binary | Responsibility |
| :--- | :--- | :--- |
| **`lunco-luncosim-exposures`** | — | Headless-safe runtime exposure projection plugin. Resolves authoritative ECS/domain state and authored telemetry into the shared `EngineExposures` registry for HTML, egui, API, telemetry, and remote consumers; it has no renderer or UI dependency. |
| **`lunco-luncosim`** | `luncosim` | Headless-safe ground-physics composition root (USD + Avian + cosim + networking/API) plus the production authored-scene test runner. Windowed status, camera, terrain-shadow, environment-presentation, and offscreen-recording bridges live in `lunco-luncosim-ui`; the application still composes both through one core plugin. |
| **`lunco-luncosim-server`** | `luncosim-server` | Headless launcher for LunCoSim (no winit/egui) with the API + networking host. Its own crate purely so it can default to headless. |
| **`lunco-modelica-ui`** | `lunica` | The Modelica workbench application and UI facade. |
| **`lunco-modelica-core`** | `lunica_worker`, `modelica_run`, `modelica_tester`, `msl_indexer`, `msl_parse_bench` | Headless Modelica worker and CLI/indexing tools; none link the workbench UI. |

> Other binaries: `build_msl_assets` (`lunco-assets`), `net_smoke` (`lunco-networking`), `dem_worker` (`lunco-terrain-bake`, the off-thread DEM bake Web Worker — staged next to the wasm by `build_web.sh`).

---

## Detailed Crate Responsibilities

Below, selected crates whose responsibilities benefit from extra detail. (Crates not listed here are adequately described by the tables above.)

### Core Foundation

**`lunco-core`**
The bedrock of the simulation. Defines the shared scalar port substrate (`PortRegistry`, `PortInfo`, and owner-supplied metadata) for software/hardware interaction, the typed `Mutation<P>` command substrate, `SimTick`, and the `ComponentGraph` canonical data structure for all 2D diagram visualizations (Modelica, FSW, SysML). Owns the canonical BigSpace world shell, arbitrary-grid f64 pose composition/conversion, atomic grid migration, and the `ActivePhysicsFrame` boundary; it does not assign celestial semantics.

**`lunco-time`**
The unified mission-time spine (architecture doc 19). Owns `MissionClock`/`TimeTransport`/`WorldTime` (the world animation clock that also gates physics via `Time<Virtual>`), the `TimeDomain` clock tree (`Playback`, `TimeBinding`, `ResolvedDomains`) with the `AnimationPreview` domain + `ControlAnimation` transport, and the `scales` projection layer (UTC↔TAI↔TT↔TDB, sidereal) over `celestial-time`. **All time-scale/JD nuance lives here; consumers delegate.**

**`lunco-doc`**
Foundation for structured, mutable artifacts (Modelica, USD, etc.) with built-in undo/redo logic. Defines the `DocumentHost` container and the atomic `DocumentOp` pattern for state mutation and inversion.

**`lunco-storage`**
I/O abstraction layer providing a unified `Storage` trait for reading, writing,
renaming, entry-kind inspection, and directory preparation through handles.
Supports native FS and memory (for tests), with the browser localStorage
backend and architectural stubs for future OPFS/IndexedDB and remote backends.

**`lunco-assets`**
Unified asset management system. Resolves shared cache locations across git worktrees, downloads external assets via `Assets.toml` with SHA-256 verification, and handles texture pre-processing (resize/convert).

**`lunco-hash`**
The workspace hashing substrate. Fast tier provides dependency-free, wasm-clean FNV-1a hashing for process-local change detection and caching keys. CID tier (via the `cid` feature) provides CIDv1 raw + SHA-256 content-addressing for files/blobs on disk and wire.

**`lunco-precompute`**
Content-addressed precompute disk cache. Provides the `bake_or_load` mechanism to run expensive pure functions once, persist the artifact keyed by its content hash (using `lunco-hash` and `lunco-storage`), and transparently load it on subsequent runs and networked peers. Serves as the home for terrain derived layers, horizon bakes, collider/mesh bakes, and USD stage compositions.

**`lunco-theme`**
Centralized design tokens based on the Catppuccin palette. Provides semantic tokens for general UI (accent, success, error) and schematic-specific colors for diagram wires and badges, ensuring visual consistency across all panels.

**`lunco-settings`**
Centralised user-settings system. Persists one namespaced JSON file at `<OS config dir>/lunco/settings.json` with auto-save on change, giving subsystems a single place to read and write per-user preferences. It also owns the application-wide `DownloadSettings` policy: total attempts, exponential retry delay, and delay cap.

**`lunco-command-macro`**
Procedural macros for the typed command system. Provides the `#[Command]`, `#[on_command]`, and `register_commands!` macros used to simplify the creation and registration of simulation actions.

**`lunco-doc-bevy`**
Bevy ECS integration for the Document System. Provides lifecycle events (Opened, Changed, Saved, Closed), the `JournalResource` Bevy wrapper around the canonical Twin journal in `lunco-twin-journal`, and a `BevyJournalSink` for replaying remote-author entries through the same store. Lifecycle observers translate `EventOrigin` into `AuthorTag` and record `EntryKind::Lifecycle` entries directly into the canonical journal; structural ops are recorded by domain mutation paths.

**`lunco-twin-journal`**
Canonical, append-only, Twin-scoped record of every change. Immutable entries keyed by `(author, lamport)` (yrs-compatible), DAG parent links, optional `change_set` grouping for atomic undo, and `EntryKind::{Op, TextEdit, Snapshot, Lifecycle}`. Higher-level: `Stream` + `Composition`, `Branch`, `Marker`, and `UndoManager` with `UndoScope::{Document, Twin}`. Pure Rust, headless, no Bevy dep.

**`lunco-worker-transport`**
The generic Web Worker pool transport (wasm-only; `#![cfg(target_arch = "wasm32")]`). wasm32 has no OS threads, so multi-second companion work (a Modelica compile, a DEM decode + crater stamp) would freeze the page; each pool member is a JS `Worker` running a *second* wasm instance with its own linear memory. `WorkerPool` owns only the payload-agnostic plumbing — spawn / lazy-grow, the boot wire-id handshake (stale-worker guard), byte + Transferable-`ArrayBuffer` post, and crash respawn — driven by caller-supplied `Callbacks` (`on_message`/`on_ready`/`on_error`/`on_wire_mismatch`). Message framing, readiness gating, and result routing stay with the caller. `lunco-modelica-core::worker_transport` composes it for the Fast-Run pool (MSL/run state on top); `lunco-terrain-bake::worker_client` composes it for the DEM bake — so the transport is written once and reused, not duplicated.

---

### Simulation Engine

**`lunco-celestial`**
Orbital mechanics and solar-system simulation spine. Owns the canonical body catalog and named semantic reference frames, the typed f64 `FrameTree`, body-fixed rotation, gravity vectors, and automatic Sphere-of-Influence/frame migration. User-facing anchors/orbits declare physical intent; the crate resolves concrete BigSpace grids and performs the projection. Owns the `EphemerisResource` abstraction; the concrete high-fidelity provider lives in `lunco-celestial-ephemeris`.

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

**`lunco-terrain-bake`**
The pure (bevy/avian-free) DEM bake pipeline, factored out of `lunco-terrain-surface` so the SAME code runs on native and in a browser Web Worker: GeoTIFF decode → native crop → optional coarse-preview resample → crater stamp → `HeightGrid` (`bake_grid`/`finish_bake`), plus the serializable `DemBakeJob`/`StampSpec`. On native `lunco-terrain-surface` calls it inside an `AsyncComputeTaskPool` task; on wasm — where that pool runs on the page's main thread and the ~40 MB decode + crater stamp froze the tab — it dispatches to the `dem_worker` companion binary over `lunco-worker-transport`, which decodes once then streams a coarse preview (`COARSE_RES`) and then the full native grid back (coarse-then-full progressive). Only the avian collider + Bevy mesh derive stays in `lunco-terrain-surface`, where those types live.

**`lunco-obstacle-field`**
Procedural crater + rock field generation for rover testing. Produces LOD-aware obstacle distributions usable as mobility test grounds.

**`lunco-cosim`**
Multi-engine simulation orchestrator. Wires named outputs from one engine (e.g., Modelica) to named inputs of another (e.g., Avian physics) via `SimConnection` components, following FMI/SSP causality. Owns the built-in `PortRegistry` backends: rigid-body state (position/velocity/attitude/rates + force/torque/mass-props), revolute/prismatic joint motors (`angle`/`displacement`), and USD-authored sensors (IMU, range, contact), including their authoritative port metadata. Avian forces are applied through the typed-port spec table (`AvianGroup`/`AvianPort` + `PendingForces`), not a bespoke `AvianSim` struct.

**`lunco-experiments`**
Backend-agnostic experiment / batch-run registry. Models a single Fast Run as a first-class artifact (params, bounds, trajectory), decoupled from any one solver via the `ExperimentRunner` trait that another crate plugs in. `RunStatus` is `Pending → Queued → Running { t_current } → Done { wall_time_ms } | Failed { error, partial } | Cancelled`; `RunBounds` carries start/stop/interval; parallel runs schedule across a worker pool.

### Vessel Control & Hardware

**`lunco-mobility`**
Physics models for surface mobility and traction — the parameterized substrate (a vehicle is a USD file, not a Rust struct). Raycast wheel model with contact-plane traction (supports leaning single-track bikes), suspension (spring-damper), generic authored drive/heading output realization, and a soft rocker-bogie `DifferentialCoupling`.

**`lunco-avatar`**
Human-interaction layer. Provides composable camera **rigs** (SpringArm, Orbit, FreeFlight, Surface) with smooth jitter-free transitions and coordinate-grid awareness for avatar-based exploration of celestial bodies. The rigs decide *how* a camera moves; *which* camera the viewport shows is owned by the reconciler in `lunco-usd-bevy-camera` (they compose — possession changes the avatar camera's rig without changing the active view).

**`lunco-hardware`**
Physical actuator and sensor implementations. Bridges `Port` values to the `avian3d` physics engine, providing concrete motor, brake, and sensor components that interact with the simulation world.

**`lunco-controller`**
Input mapping and translation. Owns the persisted `InputBindingsSettings` keymap and converts raw human-interface device inputs (Keyboard, Gamepad, Mouse) into abstract `UserIntent` actions and typed command events for consumption by Flight Software.

---

### USD Integration Layer

**`lunco-usd-core`**
Headless OpenUSD document, authoring, operation, schema, unit-conversion, and
asset-closure substrate. It has no runtime projection, physics, rendering, or
UI dependency.

**`lunco-usd`**
High-level, UI-free USD orchestrator (`UsdPlugins`) and engineering metadata bridge. Maps LunCo-specific metadata (`lunco:*` namespace) from USD stages to Bevy components, enriching 3D models with simulation-critical data like Ephemeris IDs. Document commands and composition are available to headless consumers; interactive presentation lives in `lunco-usd-ui`.

**`lunco-usd-geometry`**
Render-free reusable geometry substrate for USD projections: NURBS curves and
patches, trimmed-domain tessellation, and rotation-minimizing curve sweeps. Its
heavy numeric dependencies are isolated from the stage loader so evaluator
changes do not rebuild unrelated USD runtime code.

**`lunco-usd-bevy-core`**
Headless composed-USD substrate shared by visual, physics, and simulation
projections. It owns the `UsdRead`/`StageView` contract, resolver-backed
composition, `UsdStageAsset` loading, `CanonicalStage` live-stage ownership,
authored-layer readers, instance identity markers, `UsdStageProjectionPlan`,
program and variant resolution, standard material binding, canonical transform
decoding, stage units, and shared composed-value readers (`read_vec3_f64`,
strict primvar/boolean readers, and their time-sampled variants). Consumers
import those helpers from `lunco_usd_bevy_core::read`; OpenUSD types such as
`sdf::Path` are used directly rather than re-exported by a projection crate.
It deliberately contains no
visual mesh, light, camera, renderer, window, or UI projection, so changes to
those adapters do not rebuild this reader/composition package.

**`lunco-usd-bevy-scene`**
Render-free ECS contract between USD projection domains. It owns `UsdPrimPath`,
`UsdSceneProjected`, `UsdSceneRoot`, `UsdPreviewOnly`, `UsdAnimated`, and the
stage revision/ancestry helpers, plus the shared USD primitive and indexed-mesh
readers. Avian, terrain, and other headless projections depend on this package
without depending on the visual mesh/camera/light adapter; the visual crate
uses the same contract when it binds presentation components.

**`lunco-usd-ui`**
Interactive USD browser and preview presentation. Owns workbench sections, preview sessions/views, viewport queries, Save-As picker integration, and UI status/placeholder adapters while consuming the document and projection APIs from `lunco-usd`.

**`lunco-usd-bevy-camera`**
Render-free camera adapter built on `lunco-usd-bevy-core` and
`lunco-usd-bevy-scene`. It maps standard USD `def Camera` prims to camera
intent, handles rover-mounted and cinematic camera poses, and owns camera
selection plus the single-authority viewport reconciler. It contains no visual
projection and does not depend on the visual adapter.

**`lunco-usd-bevy`**
Visual OpenUSD bridge built on `lunco-usd-bevy-core`. It maps USD prim
hierarchies and visual facts into Bevy entities/components, projects meshes,
lights, render intent, and authored `timeSamples` animation. It installs the
camera adapter at the integration boundary; `lunco-render-bevy` supplies the
concrete render pipeline. See
[`17-view-and-intent.md §6`](architecture/17-view-and-intent.md).
Headless consumers import the owning `lunco-usd-bevy-core` modules directly;
this visual adapter is not a compatibility facade for the headless API.

**`lunco-usd-avian`**
Physics bridge for OpenUSD (`UsdAvianPlugin`). Maps `UsdPhysics` schemas — rigid bodies + mass-properties, all collider shapes, and **all joints** (revolute/prismatic/fixed/spherical/distance, D6-reduced) with `UsdPhysicsDriveAPI` motor drive — to Avian3D. The single home for Avian joint construction (incl. the programmatic wheel hinge).

**`lunco-usd-sim`**
Specialized simulation metadata bridge. Intercepts complex industry-standard vehicle schemas (like NVIDIA PhysX Vehicles) and substitutes them with optimized LunCo simulation models (e.g., Raycast wheels). Its integration tests cover the complete USD→Bevy→Avian→simulation seam; direct USD physics lowering remains in `lunco-usd-avian`.

**`lunco-materials`**
Shader appearance **intent** — **render-free**. Holds `ShaderLook` (a `.wgsl` path + an open `dyn_params` map + named texture layers), the WGSL-reflected `ParamSchema` (parameter names/ranges/defaults are parsed from each shader's own `struct Material` — **none are hardcoded in Rust**, so adding a parameter is editing a shader), and the CDLOD geomorph vertex attribute. It names **no** material type and no render pipeline, so a domain crate may depend on it without linking `bevy_render`. The concrete `ShaderMaterial` it describes lives in `lunco-render-bevy`. See [architecture/shader-layers-and-params.md](architecture/shader-layers-and-params.md).

---

### Networking & API

**`lunco-networking`**
Multiplayer transport adapter. Handles ECS replication, transport abstraction (UDP/WebSockets), and collaborative editing. Physics snapshots and camera/perspective state transfer f64 named-frame state; capture/apply automatically convert between each peer's private `ActivePhysicsFrame` and the semantic frame. No `CellCoord` is a public/wire reference-frame identity.

**`lunco-api`**
Transport-agnostic API core. Exposes simulation state and command discovery via HTTP, mapping ULID-based stable entity IDs to process-local Bevy entities for external control and inspection.

**`lunco-telemetry`**
Reflection-based data extraction engine. Automatically samples and standardizes internal physics and software values for broadcast to external monitoring systems or Mission Control bridges (YAMCS/XTCE).

---

### Workbench & UI Tools

**`lunco-workbench`**
The engineering-IDE shell. Handles the docking engine (tabs, splits),
perspective presets (Build, Simulate), Twin Browser, shared hierarchy-row
presentation (`tree::{branch, leaf}`), and picker/command adapters. It does
not own file bytes or backend I/O; those go through `lunco-storage`, while
Twin discovery stays in `lunco-workspace`/`lunco-twin`.

**`lunco-status-core`**
Renderer-independent status and lifecycle substrate. It owns the shared
`StatusBus`, scoped busy/progress handles, tracked async tasks, and telemetry
mirroring. The concrete workbench status bar and `lunco-ui` busy widgets are
consumers; headless hosts can publish and inspect the same status without
linking the shell.

**`lunco-ui`**
Reusable UI infrastructure. Provides the `WidgetSystem` for cached ECS widgets, support for typed commands, and `WorldPanel` for 3D in-scene UI elements attached to entities.

**`lunco-viz`**
Domain-agnostic visualization framework. Collects simulation data into a `SignalRegistry` and renders it via `Visualization` kinds (LinePlots, Gauges) into various view targets like 2D panels or the 3D viewport.

**`lunco-canvas`**
2D scene editor substrate. Provides the stateful viewport and tool foundation for diagramming and node-based editing, powering the Modelica diagram editor and other schematic-based tools.

**`lunco-luncosim-edit`**
In-scene editing toolkit for the 3D viewport. Implements click-to-place spawning, transform gizmos for manipulation, the universal Ports panel, and inspector panels for real-time property editing during simulation assembly.

**`lunco-render`**
Appearance **intent** and persisted Graphics quality policy — **render-free**. The vocabulary a domain crate uses to say what a thing should look like without naming a renderer: `PbrLook` (a plain surface as data — colour, roughness, metallic, emissive, alpha mode, texture channels), `SceneCamera`, `WorldLabel`, the sun/shadow look settings, and `RenderingQualitySettings` for shared camera, light, sky, terrain, shadow, and tessellation budgets. It names `Mesh3d` but **never `MeshMaterial3d`** — that one line is the whole rule.

**`lunco-render-bevy`**
The **only** crate that names `bevy_pbr`. Binds the intent above to real Bevy materials: `PbrLook` → `StandardMaterial`, `ShaderLook` → `ShaderMaterial` (the one general self-describing `AsBindGroup`, any `.wgsl` per-instance), plus `SceneCamera` → camera bundle, `WorldLabel` → billboard text, environment light and horizon shading. Headless simply never adds this plugin — which is why `--no-ui` links **no wgpu, no `bevy_render`, no `bevy_pbr`, no egui, no winit**. See [architecture/render-decoupling.md](architecture/render-decoupling.md).

**`lunco-web`**
Shared web frontend for the wasm apps. Provides the streaming loader (`web/lunco-boot.{js,css}`), `WebReadyPlugin`, which signals the HTML loader once Bevy paints its first frame, and `mountRhaiTool`, which mounts trusted HTML/CSS tool bundles whose actions execute through the existing Rhai bridge.

---

### Scripting & Modeling

**`lunco-modelica-core`**
Modelica language integration. Provides AST-based editing, compilation via Rumoca, and interactive diagramming, allowing complex industrial models to drive simulation entities and vessel subsystems. On wasm, compiles/Fast-Runs are dispatched off the main thread to the `lunica_worker` companion binary; its `worker_transport` composes the generic `lunco-worker-transport::WorkerPool` (spawn/handshake/post/respawn) and layers the Modelica-specific MSL-readiness and per-run routing on top.

**`lunco-scripting`**
Language-neutral world bridge for dynamic logic providers. The default (and only fully-wired) backend is **rhai** — browser-capable and enabled by the default `rhai` feature; build with `--no-default-features` for a script-free build. The bridge exposes ECS verbs and a native `ValueBuilder` (no JSON on the read path) over which each runtime is a thin binding. Python is an optional backend used for one-shot snippet evaluation only; Lua is a reserved (not yet implemented) backend id. rhai also funnels the `lunco-tools` registry into the engine via `lunco-tools-rhai`.

**`lunco-tools`**
Backend-agnostic, dependency-free tool registry. A *tool* is a named, reusable unit a scenario reaches as a **script-call library** (`name::fn(...)` from rhai). A tool's implementation is pluggable (rhai source, native Rust, or future runtimes). This crate owns only the bevy-free `Tool` trait (discovery metadata + `as_any` downcast hook), the global registry, and discovery — no bevy, no rhai, so the rhai-binding adapter (`lunco-tools-rhai`) stays slim. Behaviour-tree *execution* of a tool (the `run_tool` leaf) is a bevy-aware capability and lives in `lunco-tools-bevy`, not here.

**`lunco-tools-rhai`**
rhai adapter for the `lunco-tools` registry. Provides the two concrete `Tool` impls scenarios use today — `RhaiTool` (rhai source) and `NativeRhaiTool` (native Rust functions) — and `bind_registered_tools`, which binds every registered tool into a rhai `Engine` as a static module so it is callable as `name::fn(...)` from anywhere, including task closures and event/lifecycle hooks. Tools authored in other runtimes are exposed to rhai as a `NativeRhaiTool`.

**`lunco-tools-bevy`**
Bevy dispatch adapter for `lunco-tools` — the **behaviour-tree execution** half. Defines a bevy-aware `ExecutableTool` supertrait (separate from the bevy-free `Tool`, so `lunco-tools-rhai` doesn't pull bevy) + `ClosureTool` (the common-case instrument: a closure that triggers its typed command directly via `&mut World`, no JSON/reflect). Observes `lunco_core::tools::ToolFired` (emitted by `lunco-autopilot`'s `run_tool` leaf), looks the tool up in the registry, downcasts to `ExecutableTool`, and runs it. Instruments are registered declaratively via `register_closure_tool(name, sigs, |world, vessel, gid, args| { world.trigger(MyCommand{...}); Ok })` — the closure IS the tool definition; adding an instrument is one closure, no per-instrument Rust struct.

---

### Applications

**`lunco-luncosim`**
The LunCoSim application — ground mobility + physics, loaded from USD (binary `luncosim`). A composition root rather than a UI host: `LunCoSimCorePlugin` (headless-safe sim/physics/cosim/USD/networking/API) plus an optional `LunCoSimUiPlugin` from `lunco-luncosim-ui` (egui workbench, windowed) or `LunCoSimHeadlessPlugin`. Assembles the USD scene, Avian physics, and the in-scene edit tools, and is the single shared entry point for both the `luncosim` GUI and `luncosim-server` headless binaries.

**`lunco-luncosim-ui`**
Windowed LunCoSim presentation and packaging boundary: egui workbench, interactive editor composition, status/camera/terrain/environment bridges, GPU-backed offscreen recording, native window icon generation, and the UI-owned `window_icon_bytes()` API. The headless application core and `luncosim-server` do not compile its GUI/build-time graphics dependencies.

**`lunco-luncosim-exposures`**
Production integration crate for the renderer-independent runtime exposure projection. `RuntimeExposuresPlugin` registers the single shared path from authoritative ECS/domain state and authored telemetry to `lunco_core::exposure::EngineExposures`; HTML, egui, API, telemetry, and remote clients consume that registry. It owns no UI, renderer, or tutorial policy, so changing exposure derivation does not recompile the application composition root.

**`lunco-luncosim-server`**
Headless launcher for the luncosim — the same app as `luncosim`, built without the GUI (no winit/egui) and with the API + networking host enabled. Exists as its own crate purely so it can default to headless (Cargo default features are per-package).
