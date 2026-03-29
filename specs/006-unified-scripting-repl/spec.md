# Feature Specification: 006-unified-scripting-repl

**Feature Branch**: `006-unified-scripting-repl`
**Created**: 2026-03-29
**Status**: Draft
**Input**: Integrated Lua REPL, property inspection, and live state overrides.

## Problem Statement
Building and testing a lunar digital twin requires high-frequency iteration. Developers and researchers need to inspect ECS component states, override physics parameters, and hot-swap logic without a mission restart or a full Rust recompilation. 

**Interactive Debugging over Telemetry**: While the Telemetry Bridge (`011`) handles mission-critical data visualization (OpenMCT, Grafana), this spec defines the **In-Engine Developer Interface**. It provides a "Live Workspace" where the entire simulation state is exposed to the user via a **Unified Lua REPL** for immediate, low-latency manipulation.

## User Scenarios

### User Story 1 - Quake-Style Console & REPL (Priority: P1)
As a systems engineer, I want to press a hotkey (e.g. `~`) to open a command console and execute live Lua snippets against the active simulation world.

**Acceptance Criteria:**
- The engine implements a Quake-style overlay console.
- Users can type Lua commands (e.g. `world.get_entity(42).transform.x = 100`) to modify live state.
- The REPL suggests autocompletions for ECS component names and common commands.

### User Story 2 - Property Inspection & Value Override (Priority: P1)
As a physics researcher, I want to click on a rover wheel and see its current `TireFriction` coefficient in a side-panel, allowing me to type a new value and see the results instantly.

**Acceptance Criteria:**
- The Unified Editor (`007`) provides a "Property Inspector" pane.
- Clicking an entity populates the pane with all its public components and their data fields.
- Changing a value in the inspector immediately mutates the underlying Bevy component.

### User Story 3 - Scene Scrubber & Time Control (Priority: P2)
As a scenario designer, I want to pause the simulation, scrub the "Time of Day" to check shadow positions, and then resume, all while the physics engine maintains its deterministic state.

**Acceptance Criteria:**
- The REPL exposes global commands for time-scale control (`time.pause()`, `time.set_scale(5.0)`).
- The "Scrubber" UI interacts with the Astronomical Environment (`018`) to update the solar position.

## Implementation Notes
- **Lua Binding**: The engine leverages `bevy_mod_scripting` or a similar bridge to expose the ECS `World` safely to the Lua runtime.
- **External Exposure**: The same REPL logic MUST be exposed via the **Remote Control API** (`007 Story 3`) so that external Python scripts or web tools can achieve the same level of granular control.
- **Performance**: The REPL and inspector overhead must be zero when the console is closed.
