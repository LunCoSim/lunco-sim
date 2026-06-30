# LunCoSim Crates Index

This document provides a comprehensive index of all crates in the LunCoSim workspace, categorized by their functional domain and architectural responsibility. It serves as a navigation guide for both developers and AI agents.

---

## 1. Workspace & Core Foundation
Low-level primitives, document/journal systems, time, and cross-cutting concerns (storage, assets, theming, settings).

| Crate | Responsibility |
| :--- | :--- |
| **`lunco-core`** | Core primitives (`DigitalPort`, `PhysicalPort`, the typed `Mutation<P>` command substrate, `SimTick`), coordinate systems, and canonical diagram data types. |
| **`lunco-command-macro`** | Procedural macros for the typed command system (`#[Command]`, `#[on_command]`, `register_commands!`; re-exported by `lunco-core`). |
| **`lunco-workspace`** | Headless editor session management: open Twins, active documents, perspectives, and recents. |
| **`lunco-twin`** | The simulation unit on disk: folder structure, `twin.toml` manifest parsing, and file indexing. |
| **`lunco-twin-journal`** | Canonical Twin-scoped op log: Lamport-ordered entries, DAG parents (for future merges), Streams + Composition, ChangeSets, Markers, Branches, `UndoManager`. CRDT-shapable schema; in-memory backend today, yrs-swap-ready. |
| **`lunco-doc`** | Foundation for structured artifacts (Modelica, USD, SysML): the `DocumentHost` container and atomic `DocumentOp` pattern with built-in undo/redo. |
| **`lunco-doc-bevy`** | Bevy ECS integration for the Document System: lifecycle events, `JournalResource` (Bevy wrapper around the canonical Twin journal), `BevyJournalSink` for remote-replay, `EditorIntent` keybindings. |
| **`lunco-storage`** | I/O abstraction layer (`Storage` trait — Native FS, Memory, future WASM/Remote backends). The single write path; raw `std::fs` is disallowed. |
| **`lunco-assets`** | Unified asset management: cache resolution across worktrees, versioned downloads (`Assets.toml`, SHA-256), and texture processing. |
| **`lunco-cache`** | Generic resource cache with in-flight deduplication for resolved URIs and parsed artifacts. |
| **`lunco-settings`** | Centralised user-settings: one JSON file (`~/.lunco/settings.json`), namespaced sections, auto-persist on change. |
| **`lunco-theme`** | Centralized design tokens (Catppuccin-based) for consistent UI across all panels and domains. |
| **`lunco-time`** | Unified mission-time spine (architecture doc 19): `MissionClock`/`TimeTransport`/`WorldTime`, the `TimeDomain` clock tree + animation transport, and the `scales` projection layer over `celestial-time`. |

---

## 2. Simulation Engine
The "Laws of Nature" — celestial mechanics, environmental state, terrain, obstacle fields, experiments, and co-simulation orchestration.

| Crate | Responsibility |
| :--- | :--- |
| **`lunco-celestial`** | Orbital mechanics, gravity, body rotation, and Sphere of Influence (SOI) transitions; sun-light driven from ephemeris. |
| **`lunco-celestial-ephemeris`** | High-precision ephemeris provider (the one place `celestial-time` is allowed). |
| **`lunco-environment`** | Per-entity position-dependent environment state (atmosphere, radiation, local gravity). |
| **`lunco-terrain-core`** | Shared terrain primitives and types used by the globe/surface crates. |
| **`lunco-terrain-globe`** | Streaming planetary terrain: quadtree-CDLOD globe, avian heightfield collision ring, big_space anchoring. |
| **`lunco-terrain-surface`** | Surface/regolith terrain detail and the layered color pipeline. |
| **`lunco-obstacle-field`** | Procedural crater + rock field generation (LOD) for rover testing. |
| **`lunco-experiments`** | Experiment/run model: `RunStatus` (`Pending`/`Queued`/`Running`/`Done`/`Failed`/`Cancelled`), `RunBounds`, parallel run scheduling. |
| **`lunco-cosim`** | Multi-engine orchestration (Modelica, FMU, Avian) via explicit input/output wiring (`SimConnection`), following FMI/SSP causality. |

---

## 3. Vessel Control & Hardware
The "Brains and Brawn" — Flight Software (FSW), On-Board Computer (OBC), mobility physics, and robotics assembly.

| Crate | Responsibility |
| :--- | :--- |
| **`lunco-mobility`** | Physics models for planetary rovers using high-performance raycast-based wheel/suspension logic. |
| **`lunco-robotics`** | High-level assembly logic and rover structural definitions (`assembler`). |
| **`lunco-avatar`** | Human-interaction layer: composable camera behaviors (SpringArm, Orbit) and control intents. |
| **`lunco-obc`** | Hardware interface emulation (ADC/DAC) between digital FSW registers and physical units. |
| **`lunco-fsw`** | Decentralized Flight Software architecture for coordinating vessel subsystems (GNC, Power, etc.). |
| **`lunco-hardware`** | Concrete physical actuators and sensors bridging `PhysicalPort` values to the `avian3d` physics engine. |
| **`lunco-controller`** | Translation of raw user input (Keyboard/Gamepad) into typed `VesselIntent` actions for FSW. |

---

## 4. USD Integration Layer
Modular bridge between OpenUSD and Bevy, covering visuals, physics, simulation metadata, and materials.

| Crate | Responsibility |
| :--- | :--- |
| **`lunco-usd`** | High-level USD orchestrator (`UsdPlugins`) and mapper for LunCo-specific engineering metadata (`lunco:*`). |
| **`lunco-usd-bevy`** | Core visual bridge (`UsdBevyPlugin`): maps USD hierarchy, shapes, transforms, and `timeSamples` animation to Bevy. Owns composition/flattening (`compose.rs`, `flatten_stage`). |
| **`lunco-usd-avian`** | Physics bridge (`UsdAvianPlugin`): maps `UsdPhysics` schemas (RigidBody, Colliders, joints) to Avian3D. |
| **`lunco-usd-sim`** | Simulation-schema bridge (`UsdSimPlugin`): intercepts vehicle/cosim schemas and maps them to LunCo models. |
| **`lunco-materials`** | The one general self-describing `ShaderMaterial` (any `.wgsl` per-instance; params reflected from the shader's `struct Material`) for the USD rendering pipeline. |

---

## 5. Networking & API
External communication, ECS replication, telemetry extraction, and distributed attributes.

| Crate | Responsibility |
| :--- | :--- |
| **`lunco-networking`** | Multiplayer layer: transport-agnostic replication, authentication, and collaborative edit logs. |
| **`lunco-api`** | Transport-agnostic API core: introspection-based command discovery and ULID entity registry. |
| **`lunco-telemetry`** | Generic reflection-based data extraction engine for "No-Code" telemetry mirroring. |
| **`lunco-attributes`** | String-based distributed tuning registry for mapping SysML paths to raw ECS memory. |

---

## 6. Workbench & UI Tools
The editor shell, visualization framework, generic 2D canvas, in-scene tools, render look, and web boot.

| Crate | Responsibility |
| :--- | :--- |
| **`lunco-workbench`** | The IDE-like frame: docking engine, perspective presets, panel registration, and the reactive `Panel`/`PanelCtx` API. |
| **`lunco-ui`** | Reusable UI infrastructure: cached widgets, 3D world panels, command builders. |
| **`lunco-viz`** | Domain-agnostic visualization: `SignalRegistry`, LinePlots, and future 3D/Rerun bridges. |
| **`lunco-canvas`** | Stateful 2D scene editor substrate for diagrams and annotation overlays. |
| **`lunco-sandbox-edit`** | In-scene editing tools: spawn systems, transform gizmos, and inspector panels. |
| **`lunco-render`** | Shared render-look configuration: the single source of truth for lunar sun shadows (and future exposure/AA/sky look). |
| **`lunco-web`** | Shared web-frontend boot library for wasm apps: streaming loader + `WebReadyPlugin` (signals the HTML loader on first paint). |

---

## 7. Scripting & Modeling
Logic engines, the tool registry, and industrial modeling.

| Crate | Responsibility |
| :--- | :--- |
| **`lunco-modelica`** | Modelica integration: AST-based editing, compilation via Rumoca, diagram visualization, and the Modelica workbench + worker pool. |
| **`lunco-scripting`** | Runtime-agnostic world-bridge + rhai/Python logic providers (scenarios, sequencing). |
| **`lunco-tools`** | Backend-agnostic tool trait + registry (rhai-free). |
| **`lunco-tools-rhai`** | rhai adapter binding for the `lunco-tools` registry. |

---

## 8. Applications
Primary entry points and simulation assembly targets.

| Crate | Binary | Responsibility |
| :--- | :--- | :--- |
| **`luncosim`** | `luncosim` | The flagship windowed simulator: celestial bodies + ephemeris, solar-system-scale `big_space`, orbital camera, and the full FSW/hardware/mobility/robotics/avatar stack under the workbench. |
| **`lunco-sandbox`** | `sandbox`, `joint_minimal` | Ground-physics test bed: USD scene + Avian physics + the sandbox edit tools. |
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
Foundation for structured, mutable artifacts (Modelica, USD, etc.) with built-in undo/redo. Defines the `DocumentHost` container and the atomic `DocumentOp` pattern for state mutation and inversion.

**`lunco-twin-journal`**
Canonical, append-only, Twin-scoped record of every change. Immutable entries keyed by `(author, lamport)` (yrs-compatible), DAG parent links, optional `change_set` grouping for atomic undo, and `EntryKind::{Op, TextEdit, Snapshot, Lifecycle}`. Higher-level: `Stream` + `Composition`, `Branch`, `Marker`, and `UndoManager` with `UndoScope::{Document, Twin}`. Pure Rust, headless, no Bevy dep.

### Simulation Engine

**`lunco-cosim`**
Multi-engine orchestrator. Wires named outputs from one engine (e.g. Modelica) to named inputs of another (e.g. Avian physics) via `SimConnection` components, following FMI/SSP patterns. Avian forces are applied through the typed-port spec table (`AvianGroup`/`AvianPort` + `PendingForces`), not a bespoke `AvianSim` struct.

**`lunco-experiments`**
The experiment/run model. `RunStatus` is `Pending → Queued → Running { t_current } → Done { wall_time_ms } | Failed { error, partial } | Cancelled`; `RunBounds` carries start/stop/interval; parallel runs schedule across a worker pool.

### USD Integration Layer

**`lunco-usd-bevy`**
Core OpenUSD visual bridge. Maps USD prim hierarchies, shapes, and transforms into Bevy entities/components, decodes the full xform-op stack (`local_transform_at`), and drives authored `timeSamples` animation (`sample_usd_animation`). Composition/flattening lives here (`compose.rs`, `flatten_stage`) — there is no separate `lunco-usd-composer` crate.

### Applications

**`lunco-sandbox`**
The ground-physics test bed (binary `sandbox`). Assembles the USD scene, Avian physics, and the in-scene edit tools. (Historically this crate's README mis-titled it "lunco-client"; the package name is `lunco-sandbox`.) The full mission simulator is the separate `luncosim` crate; the headless variant is `lunco-sandbox-server`.
