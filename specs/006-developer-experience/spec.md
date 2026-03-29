# Feature Specification: 006-developer-experience

**Feature Branch**: `006-developer-experience`
**Created**: 2026-03-29
**Status**: Draft
**Input**: Unified Editor UI, Quake-style REPL, Dynamic Scripting (Lua/Python), Attribute Inspection, Remote API.
**Consolidates**: Former specs `006-unified-scripting-repl`, `007-unified-editor`, `008-dynamic-scripting`.

## Problem Statement
Building and testing a lunar digital twin requires high-frequency iteration. A single, cohesive "Developer Experience" system must provide: (1) an interactive Godot-like UI for visual inspection, (2) a live Lua REPL for immediate ECS manipulation, (3) dynamic scripting runtimes (Lua for real-time, Python for data science) that power FSW logic and hot-reload, (4) a headless backend mode for cloud deployment, and (5) a Remote Control API so that external tools have identical power to the in-engine UI.

These are one system — the scripting runtime powers the REPL, the REPL lives inside the editor, and the editor exposes everything via the remote API.

## User Stories

### Editor & UI

#### Story 1: Godot-like In-Game UI (Priority: P1)
As a scenario designer, I want a fully interactive UI pane inside the Bevy window for inspecting entities, dragging parts, and manipulating the environment.

**Acceptance Criteria:**
- The engine implements `bevy_egui` or a similar UI inspector layer.
- Users can click on physics entities to view their attributes (Transform, Sensors).
- **Celestial Lighting**: Users can scrub "Time of Day" / "Sun Position" to preview shadows and thermal states.

#### Story 2: Headless Backend Server (Priority: P0)
As a digital twin operator, I want to run the engine on a Linux VPS without a GPU/display attached.

**Acceptance Criteria:**
- The engine launches via `--headless` flag, disabling graphical window creation and UI rendering.
- Physics, FSW, and Modelica continue to operate deterministically.

#### Story 3: Remote Control API (Priority: P2)
As an external controller (Browser, MCP, Yamcs), I want to connect to the simulation via HTTP or WebSockets.

**Acceptance Criteria:**
- The editor exposes a headless REST or WebSocket API.
- Setting the sun position, pausing time, or querying entity state can be done via JSON payloads.
- The same commands available in the REPL are available via the API.

#### Story 4: Multi-Window Layout Manager (Priority: P2)
As a mission operator, I want to organize my workspace into dockable and floating windows (Telemetry, 3D View, Modelica Inspector).

**Acceptance Criteria:**
- The UI supports a "Workspace" layout with resizable/detachable panes.
- Layouts can be saved and reloaded between sessions.

#### Story 5: Global Notification & Master Caution (Priority: P1)
As a simulation user, I want immediate visual feedback for critical events (e.g., "Battery Critical", "Impact Detected").

**Acceptance Criteria:**
- The engine implements a `NotificationManager` for user-facing feedback.
- Critical engineering thresholds (from `014`) trigger "Master Caution" visual alerts and audio alarms (from `023`).

#### Story 6: Application Launcher / Mode Switcher (Priority: P2)
As a developer, I want to switch between operational modes (3D Simulation, Assembly Editor, Modelica Inspector) without restarting.

**Acceptance Criteria:**
- The editor provides a "Mode Switcher" interface.
- Switching modes dynamically enables/disables relevant Bevy plugin sets.

---

### REPL & Inspection

#### Story 7: Quake-Style Console & REPL (Priority: P1)
As a systems engineer, I want to press `~` to open a command console and execute live Lua snippets against the simulation.

**Acceptance Criteria:**
- The engine implements a Quake-style overlay console.
- Users can type Lua commands (e.g., `world.get_entity(42).transform.x = 100`) to modify live state.
- The REPL suggests autocompletions for ECS component names and common commands.
- Commands are routed through the same logic as the Remote Control API (Story 3).

#### Story 8: Attribute Inspection & Value Override (Priority: P1)
As a physics researcher, I want to click on a rover wheel and see its `TireFriction` in a side-panel, then type a new value and see the results instantly.

**Acceptance Criteria:**
- The editor provides a "Attribute Inspector" pane.
- Clicking an entity populates the pane with all public components and data fields.
- Changing a value immediately mutates the underlying Bevy component.

#### Story 9: Scene Scrubber & Time Control (Priority: P2)
As a scenario designer, I want to pause the simulation, scrub "Time of Day" to check shadows, then resume.

**Acceptance Criteria:**
- The REPL exposes global time commands (`time.pause()`, `time.set_scale(5.0)`).
- The "Scrubber" UI interacts with `018-astronomical-environment` to update solar position.

---

### Dynamic Scripting

#### Story 10: Lua Real-Time Logic (Priority: P1)
As a robotics control engineer, I want to write a custom sensor-processing loop in Lua and hot-reload it while the simulation runs.

**Acceptance Criteria:**
- The engine uses `bevy_mod_scripting` or similar to expose ECS to sandboxed `.lua` files.
- Scripts are dynamically hot-reloaded without stutter or recompilation.

**Lua as Scripting Glue**: Lua serves as the primary language for the REPL and internal logic glue, ensuring high-performance ECS access while remaining accessible to engineers.

#### Story 11: Python Data Science Hooks (Priority: P1)
As a machine learning engineer, I want to use Python (`numpy`, `tensorflow`) to query telemetry and issue autonomous commands.

**Acceptance Criteria:**
- The scripting bridge supports a Python backend (via `PyO3`).
- Python scripts run on a decoupled evaluation tick (e.g., 10 Hz) due to GIL constraints.

#### Story 12: Scripted Flight Software (FSW) (Priority: P2)
As an autonomy engineer, I want to write a custom "Mission FSW" in Lua that receives **Commands** and executes them by toggling **OBC Ports**.

**Acceptance Criteria:**
- The engine supports registering a Lua script as the `ActiveFSW` (Level 3).
- Scripts iterate over `Events<Command>` and write to `PinState` components.
- Scripts can implement "Safety Monitors" that override port states if sensor thresholds are breached.

#### Story 13: Timed Maneuver Plans (Priority: P2)
As an autonomy engineer, I want to write a Lua script executing a timed sequence of motor signals ("Maneuver Plan").

**Acceptance Criteria:**
- Scripts can implement a `TimedBuffer` for incoming signals.
- **Force-Curve Support**: Smooth interpolation between force values over time.
- **Actuator Metadata Aware**: Scripts can query `MaxTorque` before sending a signal.

## Implementation Notes
- **Lua Binding**: Engine leverages `bevy_mod_scripting` to expose ECS `World` to Lua runtime.
- **External Exposure**: REPL logic MUST be exposed via the Remote Control API so external Python/web tools achieve the same control.
- **Performance**: REPL and inspector overhead must be zero when console is closed.
- The scripting runtime powers the REPL; the REPL lives inside the editor; the editor exposes everything via the API. One system.

## Key Entities & Terminology
For a complete definition of all entities (OBC, FSW, PinState, etc.) and architectural terminology, refer to the authoritative **[Engineering Ontology](file:///home/rod/Documents/lunco/lunco-sim-bevy/specs/ontology.md)**.
