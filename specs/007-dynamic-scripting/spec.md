# Feature Specification: 007-dynamic-scripting

**Feature Branch**: `007-dynamic-scripting`
**Created**: 2026-03-29
**Status**: Draft
**Input**: Multi-language runtime extensibility (Lua + Python) integrated natively into the Bevy ECS and Unified Editor.

## Problem Statement
Requiring a Rust toolchain recompilation to change simple test behaviors severely limits the accessibility of the simulation for domain experts (like aerodynamicists or ML scientists). The engine must support an "Omniverse-style" runtime scripting layer where behaviors are loaded and evaluated completely on the fly without restarting the compiled binary.

## User Scenarios

### User Story 1 - Lua Real-time Logic (Priority: P1)
As a robotics control engineer, I want to write a custom sensor-processing loop in Lua, so that I can rapidly iterate on rover actuator math while the simulation is actively running.

**Acceptance Criteria:**
- The engine uses a scripting bridge (e.g., `mlua` or `bevy_mod_scripting`) to expose Bevy's ECS components and `FixedUpdate` loop to sandboxed `.lua` files.
- Scripts are dynamically hot-reloaded: saving changes to `scripts/custom_drive.lua` is immediately reflected in the simulation without stutter or recompilation.

### User Story 2 - Python Data Science Hooks (Priority: P1)
As a machine learning engineer, I want to use standard Python packages (`numpy`, `tensorflow`) to query the rover's telemetry and issue autonomous commands, so that my algorithm can drive the Rover inside Bevy.

**Acceptance Criteria:**
- The scripting bridge supports a Python backend (via `PyO3`).
- Because Python execution is heavier and constrained by the Global Interpreter Lock (GIL), Python scripts run on a decoupled evaluation tick (e.g., 10 Hz) querying sensor data and generating actuator outputs for the fast Lua/Rust physics loops to consume.

### User Story 3 - Visual Script Editor (Priority: P2)
As a scenario designer, I want to interactively type script commands into a UI panel, so that I can teleport entities or adjust sun angles dynamically without modifying files.

**Acceptance Criteria:**
- The Unified Editor (`006`) implements an interactive Scripting REPL (Read-Eval-Print Loop) panel.
- Users can execute live Lua commands directly against the active Bevy `World`.
