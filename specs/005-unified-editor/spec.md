# Feature Specification: 005-unified-editor

**Feature Branch**: `005-unified-editor`
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
