# Agents Guidelines for LunCoSim

This file provides context and constraints for AI agents working on the LunCoSim project.

## Project Context
LunCoSim is a digital twin of the solar system built with the Bevy engine. It follows a modular, hotswappable plugin architecture and mandates TDD.

## Core Technologies
- **Bevy Engine**: We are using **Bevy 0.18**.
- **Physics**: Avian3D (0.6.1)
- **Large-scale space**: big_space (0.12.0)
- **Input Management**: leafwing-input-manager (0.20.0)

## Key Constraints
- **Hotswappable Plugins**: Everything must be a plugin.
- **TDD-First**: Write tests before feature code.
- **Headless-First**: Simulation core must run without a GPU.
- **SysML v2**: Used for high-level system models and "source of truth".
- **Physical Precision**: All physical properties (mass, dimensions, forces, spring constants, axes) MUST use `f64`. Control signals and non-physics data (e.g., UI, ports, commands) may use `f32`.

## Tooling & Workflow
- **Search Tools**: Always skip the `target/` directory when using `grep` or other search tools to avoid searching generated artifacts.
