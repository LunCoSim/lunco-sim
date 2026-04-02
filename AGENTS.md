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
.
