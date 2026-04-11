# LunCoSim AI Agent Guidelines

This document provides specific instructions and context for AI agents (Claude, Gemini, Antigravity, etc.) working on the LunCoSim codebase. Adherence to these guidelines is mandatory for maintaining simulation integrity and modularity.

## 1. Project Context
LunCoSim is a digital twin of the solar system built with the Bevy engine. It follows a modular, hotswappable plugin architecture and mandates Test-Driven Development (TDD).

## 2. Core Technologies
- **Bevy Engine**: We are using **Bevy 0.18**.
- **Physics**: Avian3D (0.6.1)
- **Large-scale space**: big_space (0.12.0)
- **Input Management**: leafwing-input-manager (0.20.0)

## 3. The Tunability Mandate
As per Article X of the Project Constitution, **hardcoded magic numbers are forbidden**. 

*   **Visuals**: Colors, line widths, fade ranges, and subdivisions must be stored in Bevy `Resources` (for global settings) or `Components` (for entity-specific settings).
*   **Physics**: Gravity constants, SOI thresholds, and orbital sampling rates must be exposed as configurable parameters.
*   **UI**: Padding, margins, and transition speeds should be defined in a theme resource.

## 4. Key Constraints
- **Hotswappable Plugins**: Everything must be a plugin.
- **TDD-First**: Write tests before feature code.
- **Headless-First**: Simulation core must run without a GPU.
- **SysML v2**: Used for high-level system models and "source of truth".
- **Double Precision (f64)**: For all spatial math, physics, ephemeris calculations, and physical properties (mass, dimensions, forces, spring constants, axes), use `f64` or `DVec3`. Single precision (`f32`) is only acceptable for final rendering offsets, UI-level logic, or non-physics signals.
- **Non-Blocking UI (Responsive Mandate)**: Performance-intensive tasks (mesh generation, large-scale ephemeris lookups, physics collider building) MUST be offloaded to `AsyncComputeTaskPool`. Synchronous execution of heavy math in the main thread is forbidden to prevent UI stuttering.

## 4.1. Four-Layer Plugin Architecture

LunCoSim follows a standard simulation software pattern with independent plugin layers. Every feature you implement must fit into one of these layers:

```
Layer 4: UIPlugins (optional)     — bevy_workbench, lunco-ui, domain ui/ panels
Layer 3: SimulationPlugins (opt)  — Rendering, Cameras, Lighting, 3D viewport, Gizmos
Layer 2: DomainPlugins (always)   — Celestial, Avatar, Mobility, Robotics, OBC, FSW
Layer 1: SimCore (always)         — MinimalPlugins, ScheduleRunner, big_space, Avian3D
```

**Rules for agents**:
1. **Never mix layers in a single plugin**. A plugin is either domain logic (Layer 2) OR UI (Layer 4), never both.
2. **UI lives in `ui/` subdirectory**. Domain crates have `src/ui/mod.rs` that exports a `*UiPlugin`. UI code stays in `ui/`.
3. **UI never mutates state directly**. All UI interactions emit `CommandMessage` events. Observers in domain code handle the logic.
4. **Headless must work**. Removing Layer 3 and Layer 4 plugins must leave a functioning simulation. Tests use `MinimalPlugins` only.
5. **Domain plugins are self-contained**. `SandboxEditPlugin` provides logic (spawn, selection, undo). `SandboxEditUiPlugin` provides panels. They are independent.

**Example — correct layering**:
```rust
// crates/lunco-sandbox-edit/src/lib.rs     ← Layer 2: Domain logic
pub struct SandboxEditPlugin;  // spawn, selection, undo — NO UI

// crates/lunco-sandbox-edit/src/ui/mod.rs  ← Layer 4: UI
pub struct SandboxEditUiPlugin;  // registers panels with bevy_workbench
```

**Example — correct composition**:
```rust
// Full sim: all four layers
app.add_plugins(DefaultPlugins)           // Layer 1 + 3
   .add_plugins(LunCoAvatarPlugin)        // Layer 2
   .add_plugins(SandboxEditPlugin)        // Layer 2
   .add_plugins(WorkbenchPlugin)          // Layer 4
   .add_plugins(LuncoUiPlugin)            // Layer 4
   .add_plugins(SandboxEditUiPlugin)      // Layer 4

// Headless: only layers 1 + 2
app.add_plugins((MinimalPlugins, ScheduleRunnerPlugin::run_loop(...)))  // Layer 1
   .add_plugins(LunCoAvatarPlugin)        // Layer 2
   .add_plugins(SandboxEditPlugin)        // Layer 2
   // No Layer 3, no Layer 4
```

## 5. Implementation Patterns
### Dynamic Update Pattern
When adding a new tunable parameter:
1.  Define/Update a Bevy `Resource` to hold the data.
2.  Use that resource in your `System` queries.
3.  Ensure the system updates every frame or reacts to resource changes (`ResChanged`).

### Constitutional Hierarchy
Always verify your implementation plan against `.specify/memory/constitution.md`. If a feature request conflicts with the constitution (e.g., suggesting a non-plugin-based architecture), you must flag this to the user and prioritize constitutional integrity.

## 6. Tooling & Workflow
- **Search Tools**: Always skip the `target/` directory when using `grep` or other search tools to avoid searching generated artifacts.

## 7. Documentation Standards
- **MANDATORY Documentation**: All produced code MUST be documented using Rust's built-in doc comments (`///` for functions/structs/enums and `//!` for modules).
- **Maintenance Focus**: Comments should primarily aid in **system maintenance** for both **human developers and AI agents**.
- **The "Why" Over "How"**: Prioritize explaining the design intent, dependencies, and "why" a particular approach was chosen, rather than just restating what the code does. 
- **Conciseness**: Aim for "the right amount" of documentation—clear, helpful, and never redundant.
