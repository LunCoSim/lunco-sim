# Feature Specification: 020-mission-recording

**Feature Branch**: `020-mission-recording`
**Created**: 2026-03-29
**Status**: Draft
**Input**: Saving and replaying missions for post-mortem analysis.

## Problem Statement
Simulations run headless overnight. When a rover crashes due to an autonomous algorithm failure, developers need a way to review exactly what happened frame-by-frame. We need an industry-standard way to serialize Bevy ECS state and Telemetry to disk for later playback.

## User Scenarios

### User Story 1 - MCAP / ROSbag Export (Priority: P2)
As an autonomy engineer, I want the simulation to dump its state into an MCAP or ROSbag file, so that I can visualize the entire run in Foxglove Studio or Rviz.

**Acceptance Criteria:**
- The simulation records critical ECS events (Transforms, Sensor readings, Actuator commands) sequentially.
- The output file can be loaded into an external visualization tool.

### User Story 2 - In-Engine Playback (Priority: P3)
As a test engineer, I want to load a recorded mission file back into the engine so that I can visualize it offline without re-running the physics loop.

**Acceptance Criteria:**
- Bevy can ingest the recorded log and purely update visual Transforms historically, disabling Rapier/Avian physics processing.
