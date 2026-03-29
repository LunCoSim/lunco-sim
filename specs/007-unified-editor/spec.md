# Feature Specification: 007-unified-editor

**Feature Branch**: `007-unified-editor`
**Created**: 2026-03-29
**Status**: Draft
**Input**: Requirements for a Godot-like UI, Headless mode, and Omni-Control APIs.

## Problem Statement
The engine must support diverse operational environments: running as an interactive 3D desktop application (Godot-like) for test engineers, or running entirely headless in the cloud/server as a digital twin backend. Furthermore, controlling the environment (like changing the time of day) cannot be restricted to Rust code compilation. 

## User Scenarios

### User Story 1 - Godot-like In-Game UI (Priority: P1)
As a scenario designer, I want a fully interactive UI pane inside the Bevy window, so that I can inspect entities, drag parts around, and manipulate the environment dynamically.

**Acceptance Criteria:**
- The engine implements `bevy_egui` or a similar UI inspector layer.
- Users can click on physics entities to view their properties (Transform, Sensors).
- **Celestial Lighting**: Users can manually scrub the "Time of Day" or "Sun Position" parameter directly from the UI to instantly preview shadows and thermal states.

### User Story 2 - Headless Backend Server (Priority: P1)
As a digital twin operator, I want to run the engine on a Linux VPS without a GPU/display attached, so that it serves purely as a simulation backend.

**Acceptance Criteria:**
- The engine launches via CLI flags (e.g. `--headless`) which gracefully disables graphical window creation and UI rendering.
- The `avian` physics engine and universal control bridges continue to operate deterministically.

### User Story 3 - Remote Control API (Priority: P2)
As an external controller (Browser, MCP, Yamcs), I want to connect to the simulation via HTTP or WebSockets to execute the same powers the local Godot-like UI has.

**Acceptance Criteria:**
- The editor exposes a headless REST or WebSocket API.
- Setting the sun position, pausing time, or querying an entity's state can be achieved via a JSON payload from a web browser or MCP script.

### User Story 4 - Multi-Window Layout Manager (Priority: P2)
As a mission operator, I want to organize my workspace into dockable and floating windows (e.g., Telemetry, 3D View, Modelica Inspector), so that I can monitor complex systems simultaneously.

**Acceptance Criteria:**
- The UI system supports a "Workspace" layout with multiple panes that can be resized or detached.
- Layouts can be saved and reloaded between sessions.

### User Story 5 - Global Notification & Master Caution (Priority: P1)
As a simulation user, I want to see immediate visual and auditory feedback (toasts, popups, alarms) for critical events (e.g., "Connection Lost", "Battery Critical", "Impact Detected"), so that I don't miss important state changes.

**Acceptance Criteria:**
- The engine implements a `NotificationManager` for user-facing feedback.
- Critical engineering thresholds (from Spec 014) trigger "Master Caution" visual alerts and non-spatialized audio alarms (from Spec 023).
- *Note: This is distinct from the technical logging in Spec 002.*

### User Story 6 - Application Launcher Interface (Priority: P2)
As a developer or researcher, I want to switch between different operational modes (3D Simulation, Assembly Editor, Modelica Inspector) from within the main executable, without restarting.

**Acceptance Criteria:**
- The Unified Editor provides a "Mode Switcher" or "Launcher" interface.
- Switching modes dynamically enables/disables the relevant Bevy plugin sets for that mode.

### User Story 7 - In-Game Command Console (Priority: P1)
As an advanced user, I want a Quake-style command console (`~` key) to execute runtime commands, debug the ECS, and teleport entities.

**Acceptance Criteria:**
- The engine implements an interactive CLI (Command Line Interface) accessible in-game.
- Commands are routed through the same logic as the Remote Control API (`User Story 3`).
