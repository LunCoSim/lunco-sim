# Feature Specification: 008-dynamic-scripting

**Feature Branch**: `008-dynamic-scripting`
**Created**: 2026-03-29
**Status**: Draft
**Input**: Multi-language runtime extensibility (Lua + Python) integrated natively into the Bevy ECS and Unified Editor.

## Problem Statement
A static simulator is inflexible for iterative research. We need a way to hot-swap logic, adjust physics parameters, and automate multi-hour mission cycles without recompiling the Rust engine. We will integrate a **Dynamic Scripting Layer** (using Lua as the primary runtime and Python for high-level dataset orchestration) that interfaces directly with the Bevy ECS.

**Lua as Scripting Glue**: Lua will serve as the primary language for the **Unified REPL** and internal logic glue, ensuring high-performance access to the ECS while remaining accessible to engineers.

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

### User Story 4 - Scripted Flight Software (FSW) (Priority: P2)
As an autonomous systems engineer, I want to write a custom "Mission FSW" in Lua that receives **Commands** from the Controller/CLI and executes them by toggling **OBC Pins**.

**Acceptance Criteria:**
- The engine supports registering a Lua script as the `ActiveFSW` (Level 3).
- The script iterates over the **`Events<Command>`** and writes to **`PinState`** components of the `OBC_Emulator`.
- Scripts can implement complex "Safety Monitors" that override pin states if sensor thresholds are breached.

### User Story 5 - Timed Maneuver Plans (Priority: P2)
As an autonomy engineer, I want to write a Lua script that executes a timed sequence of motor signals ("Maneuver Plan") by directly manipulating the OBC register state (`PinState`).

**Acceptance Criteria:**
- Scripts can implement a `TimedBuffer` for incoming signals.
- **Force-Curve Support**: The scripting API allows for smooth interpolation between force values over time.
- **Actuator Metadata Aware**: Scripts can query an actuator's `MaxTorque` before sending a signal to ensure realistic physical behavior.

---

## Key Entities & Terminology
For a complete definition of all entities (OBC, FSW, PinState, etc.) and architectural terminology, refer to the authoritative **[Engineering Ontology](file:///home/rod/Documents/lunco/lunco-sim-bevy/specs/ontology.md)**.
