# LunCoSim Crates Index

This document provides a comprehensive index of all crates in the LunCoSim workspace, categorized by their functional domain and architectural responsibility. It serves as a navigation guide for both developers and AI agents.

---

## 1. Workspace & Core Foundation
Low-level primitives, document systems, and cross-cutting concerns (storage, assets, theming).

| Crate | Responsibility |
| :--- | :--- |
| **`lunco-core`** | Core primitives (`DigitalPort`, `PhysicalPort`, `CommandMessage`), coordinate systems, and canonical diagram data types. |
| **`lunco-workspace`** | Headless editor session management: open Twins, active documents, perspectives, and recents. |
| **`lunco-twin`** | The simulation unit on disk: folder structure, `twin.toml` manifest parsing, and file indexing. |
| **`lunco-doc`** | Foundation for structured artifacts (Modelica, USD, SysML) with built-in undo/redo logic. |
| **`lunco-storage`** | I/O abstraction layer (Native FS, Memory, and future WASM/Remote backends). |
| **`lunco-assets`** | Unified asset management: cache resolution, versioned downloads, and texture processing. |
| **`lunco-cache`** | Generic resource cache with in-flight deduplication for resolved URIs and parsed artifacts. |
| **`lunco-theme`** | Centralized design tokens (Catppuccin-based) for consistent UI across all panels and domains. |
| **`lunco-command-macro`** | Procedural macros for the typed command system (re-exported by `lunco-core`). |
| **`lunco-doc-bevy`** | Bevy ECS integration for the Document System: lifecycle events, `JournalResource` (Bevy wrapper around the canonical Twin journal), `BevyJournalSink` for remote-replay, `EditorIntent` keybindings, `Presence` collab seed. |
| **`lunco-twin-journal`** | Canonical Twin-scoped op log: Lamport-ordered entries, DAG parents (for future merges), Streams + Composition, ChangeSets, Markers (named milestones), Branches. CRDT-shapable schema; in-memory backend today, yrs-swap-ready. |

---

## 2. Simulation Engine
The "Laws of Nature"—celestial mechanics, environmental state, terrain, and co-simulation orchestration.

| Crate | Responsibility |
| :--- | :--- |
| **`lunco-celestial`** | High-precision orbital mechanics (Ephemeris), gravity, and Sphere of Influence (SOI) transitions. |
| **`lunco-environment`** | Per-entity position-dependent environment state (atmosphere, radiation, local gravity). |
| **`lunco-terrain`** | Procedural QuadSphere terrain generation, LOD subdivision, and heightmap-based collision. |
| **`lunco-cosim`** | Multi-engine orchestration (Modelica, FMU, GMAT, Avian) via explicit input/output wiring. |

---

## 3. Vessel Control & Hardware
The "Brains and Brawn"—Flight Software (FSW), On-Board Computer (OBC), mobility physics, and robotics assembly.

| Crate | Responsibility |
| :--- | :--- |
| **`lunco-mobility`** | Physics models for planetary rovers using high-performance raycast-based wheel/suspension logic. |
| **`lunco-robotics`** | High-level assembly logic and rover structural definitions. |
| **`lunco-avatar`** | Human-interaction layer: composable camera behaviors (SpringArm, Orbit) and control intents. |
| **`lunco-obc`** | Hardware interface emulation (ADC/DAC) between digital FSW registers and physical units. |
| **`lunco-fsw`** | Decentralized Flight Software architecture for coordinating vessel subsystems (GNC, Power, etc.). |
| **`lunco-hardware`** | Concrete implementations of physical actuators and sensors bridging ports to the physics engine. |
| **`lunco-controller`** | Translation of raw user input (Keyboard/Gamepad) into typed commands for FSW. |

---

## 4. USD Integration Layer
Modular bridge between OpenUSD and Bevy, covering visuals, physics, and simulation metadata.

| Crate | Responsibility |
| :--- | :--- |
| **`lunco-usd`** | High-level USD orchestrator and mapper for LunCo-specific engineering metadata (`lunco:*`). |
| **`lunco-usd-bevy`** | Core visual bridge: maps USD hierarchy, shapes, and transforms to Bevy entities/components. |
| **`lunco-usd-avian`** | Physics bridge: maps `USDPhysics` schemas (RigidBody, Colliders) to Avian3D components. |
| **`lunco-usd-sim`** | Intercepts specialized simulation schemas (e.g., PhysX Vehicles) and maps them to LunCo models. |
| **`lunco-usd-composer`** | Handles USD asset path resolution and stage flattening for complex multi-file assets. |
| **`lunco-materials`** | Self-contained procedural material plugins (SolarPanel, Blueprint) for the USD rendering pipeline. |

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
The editor shell, visualization framework, generic 2D canvas, and sandbox editing tools.

| Crate | Responsibility |
| :--- | :--- |
| **`lunco-workbench`** | The IDE-like frame: docking engine, perspective presets, and panel registration. |
| **`lunco-ui`** | Reusable UI infrastructure: cached widgets, 3D world panels, and command builders. |
| **`lunco-viz`** | Domain-agnostic visualization: SignalRegistry, LinePlots, and future 3D/Rerun bridges. |
| **`lunco-canvas`** | Stateful 2D scene editor substrate for diagrams and annotation overlays. |
| **`lunco-sandbox-edit`** | In-scene editing tools: spawn systems, transform gizmos, and inspector panels. |

---

## 7. Scripting & Modeling
Logic engines for dynamic simulation behavior and industrial modeling.

| Crate | Responsibility |
| :--- | :--- |
| **`lunco-modelica`** | Modelica integration: AST-based editing, compilation via Rumoca, and diagram visualization. |
| **`lunco-scripting`** | Reflected memory bridge for Python and Lua as first-class logic providers. |

---

## 8. Applications
Primary entry points and simulation assembly targets.

| Crate | Responsibility |
| :--- | :--- |
| **`lunco-client`** | The main simulation client assembling all plugins into a cohesive application. |

---

## Detailed Crate Responsibilities

### 1. Workspace & Core Foundation

**`lunco-core`**
The bedrock of the simulation. Defines `DigitalPort` and `PhysicalPort` architectural primitives for software/hardware interaction. It also owns the `CommandMessage` system and the `ComponentGraph` which serves as the canonical data structure for all 2D diagram visualizations (Modelica, FSW, SysML).

**`lunco-workspace`**
Manages the editor session ("what's open right now"). Tracks open Twins, active documents, perspectives, and recent files. It acts as a headless analog to a VS Code workspace, providing session-level metadata without depending on ECS or UI.

**`lunco-twin`**
Defines the simulation unit on disk: a folder with a `twin.toml` manifest. Handles file-system indexing, recursive sub-twin discovery, and document membership rules (nearest-neighbor ownership) using storage handles.

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

**`lunco-command-macro`**
Procedural macros for the typed command system. Provides the `#[Command]`, `#[on_command]`, and `register_commands!` macros used to simplify the creation and registration of simulation actions.

**`lunco-doc-bevy`**
Bevy ECS integration for the Document System. Provides lifecycle events (Opened, Changed, Saved, Closed), the `JournalResource` Bevy wrapper around the canonical Twin journal in `lunco-twin-journal`, and a `BevyJournalSink` for replaying remote-author entries through the same store. Lifecycle observers translate `EventOrigin` into `AuthorTag` and record `EntryKind::Lifecycle` entries directly into the canonical journal; structural ops are recorded by domain mutation paths.

**`lunco-twin-journal`**
Canonical, append-only, Twin-scoped record of every change. Entries are immutable, identified by `(author, lamport)` pairs (yrs-compatible). Carries DAG parent links (multi-parent for merges), optional `change_set` grouping for atomic undo, and `EntryKind::{Op, TextEdit, Snapshot, Lifecycle}` payloads. Higher-level abstractions: `Stream` + `Composition` (Sequential / Layered / LastWriteWins; only Sequential implemented in foundation), `Branch` (mutable named ref), `Marker` (named milestone — Onshape Version / git tag / SysML v2 named Version). `JournalSink` trait for write-side abstraction; `UndoManager` with `UndoScope::{Document, Twin}` for per-doc and Workspace-Undo. SysML v2 API conformance is a downstream adapter; the in-memory schema is the substrate. Pure Rust, headless, no Bevy dep.

---

### 2. Simulation Engine

**`lunco-celestial`**
High-precision orbital mechanics and solar system simulation. Handles planetary ephemeris, body-fixed rotation, gravity vectors, and the Sphere of Influence (SOI) system for automatic coordinate frame transitions between bodies.

**`lunco-environment`**
Position-dependent environmental state (gravity, atmosphere, radiation, etc.). Uses a provider-consumer pattern to compute local conditions for each entity based on its proximity to celestial bodies and their specific environment models.

**`lunco-terrain`**
Procedural QuadSphere terrain generation and collision. Implements cube-to-sphere projection, LOD subdivision, and heightmap-based collision for planetary surfaces, ensuring deterministic terrain across networked clients.

**`lunco-cosim`**
Multi-engine simulation orchestrator. Wires named outputs from one engine (e.g., Modelica) to named inputs of another (e.g., Avian physics) via `SimConnection` components, following FMI/SSP patterns for causality and propagation.

---

### 3. Vessel Control & Hardware

**`lunco-mobility`**
Physics models for surface mobility and traction. Implements high-performance raycast-based wheel models, suspension dynamics (spring-damper), and steering mixing (Skid/Ackermann) for realistic planetary rover simulation.

**`lunco-robotics`**
High-level vessel assembly and spawning logic. Orchestrates the composition of complex robots from constituent parts, linking chassis, wheels, software, and sensors into a cohesive simulation unit.

**`lunco-avatar`**
Human-interaction layer. Provides composable camera behaviors (SpringArm, Orbit, FreeFlight) with smooth jitter-free transitions and coordinate-grid awareness for avatar-based exploration of celestial bodies.

**`lunco-obc`**
On-Board Computer emulation. Acts as the signal-processing bridge (DAC/ADC) between digital Flight Software registers (`i16`) and physical hardware units (`f32`), emulating hardware quantization and scaling.

**`lunco-fsw`**
Decentralized Flight Software architecture. Manages vessel subsystems as independent ECS entities communicating via an asynchronous `CommandMessage` fabric, mapping semantic SysML names to hardware entities.

**`lunco-hardware`**
Physical actuator and sensor implementations. Bridges `PhysicalPort` values to the `avian3d` physics engine, providing concrete motor, brake, and sensor components that interact with the simulation world.

**`lunco-controller`**
Input mapping and translation. Converts raw human-interface device inputs (Keyboard, Gamepad, Mouse) into abstract `VesselIntent` actions and typed command events for consumption by Flight Software.

---

### 4. USD Integration Layer

**`lunco-usd`**
High-level USD orchestrator and engineering metadata bridge. Maps LunCo-specific metadata (`lunco:*` namespace) from USD stages to Bevy components, enriching 3D models with simulation-critical data like Ephemeris IDs.

**`lunco-usd-bevy`**
Core OpenUSD visual bridge. Maps USD prim hierarchies, shapes (Cubes, Spheres, Meshes), and transforms into Bevy entities and components for instant visual synchronization from USDA source files.

**`lunco-usd-avian`**
Physics bridge for OpenUSD. Automatically maps `USDPhysics` schemas (RigidBody, Colliders) to Avian3D components using high-performance ECS observers that react to USD prim paths.

**`lunco-usd-sim`**
Specialized simulation metadata bridge. Intercepts complex industry-standard vehicle schemas (like NVIDIA PhysX Vehicles) and substitutes them with optimized LunCo simulation models (e.g., Raycast wheels).

**`lunco-usd-composer`**
Handles USD asset path resolution and stage flattening. Resolves complex multi-file composition (references, sublayers) into a unified data map for the simulation stage loader, anchoring paths to the Bevy asset directory.

**`lunco-materials`**
Procedural material library for the USD pipeline. Provides self-contained plugins for specialized shaders (SolarPanel, Blueprint grid) that are automatically assigned to entities based on USD `primvars` metadata.

---

### 5. Networking & API

**`lunco-networking`**
Transparent multiplayer shim. Handles ECS replication, transport abstraction (UDP/WebSockets), and collaborative editing via a verified `AuthorizedCommand` flow and Lamport-ordered `EditLog` for history and undo.

**`lunco-api`**
Transport-agnostic API core. Exposes simulation state and command discovery via HTTP, mapping ULID-based stable entity IDs to process-local Bevy entities for external control and inspection.

**`lunco-telemetry`**
Reflection-based data extraction engine. Automatically samples and standardizes internal physics and software values for broadcast to external monitoring systems or Mission Control bridges (YAMCS/XTCE).

**`lunco-attributes`**
Distributed tuning registry. Allows external processes to mutate simulation state using string-based paths (e.g., `"vessel.rover1.suspension.k"`) that map 1:1 with SysML architectural models.

---

### 6. Workbench & UI Tools

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

---

### 7. Scripting & Modeling

**`lunco-modelica`**
Modelica language integration. Provides AST-based editing, compilation via Rumoca, and interactive diagramming, allowing complex industrial models to drive simulation entities and vessel subsystems.

**`lunco-scripting`**
Reflected memory bridge for Python and Lua. Enables dynamic logic providers to read and write simulation memory directly, supporting both deterministic physics loops and interactive REPL sessions.

---

### 8. Applications

**`lunco-client`**
Primary simulation entry point. Aggregates all domain plugins into cohesive application targets (Native/Web), orchestrating global configuration, scenario assembly, and environment integration.
