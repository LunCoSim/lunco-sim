# Feature Specification: OpenMCT Telemetry Streaming

## Problem Statement
To make the simulation act as a genuine digital twin, mission controllers need to view its data in real telemetry software. We must construct a telemetry broadcaster that streams out the 1.5kg Rover's data directly to OpenMCT.

## User Stories

### Story 1: Live Dashboard
As a mission controller
I want the simulation to stream rover statistics to OpenMCT
So that I can monitor physics and internal telemetry in a professional dashboard.

**Acceptance Criteria:**
- The Bevy app maintains a websocket or streaming connection to a local OpenMCT telemetry server.
- The rover's `position`, `velocity`, and `battery_level` (from the Modelica subsystem) are continuously transmitted.
- The telemetry updates visibly in the OpenMCT UI under 500ms of latency.
