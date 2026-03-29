# Feature Specification: 020-world-state-and-replay

**Feature Branch**: `020-world-state-and-replay`
**Created**: 2026-03-29
**Status**: Draft
**Input**: Unified ECS State Persistence, check-pointing, MCAP streaming, and replaying missions.

## Problem Statement
A digital twin of a lunar base must run for months of simulated time and be aggressively debugged post-mortem. We need a unified architectural standard to serialize the entire Bevy ECS world (live-checkpointing), record time-series telemetry to disk for external viewers (ROSbags/MCAP), and load those files back into the engine for visual playback.

## User Scenarios

### User Story 1 - Mid-Mission Checkpointing (Priority: P1)
As a mission operator, I want to save the current state of my lunar base (including internal Modelica states and ECS Transforms), so I can resume exactly where I left off.

**Acceptance Criteria**:
- The engine implements a `WorldSnapshot` system executing Bincode/MessagePack binary serialization on all components marked with a `Persistent` trait.
- Loading a save performs a "Warm Start" of external solvers (`rumoca`, `Cantera`) preventing numerical spikes during the first resumed frame, ensuring mathematical state continuity.

### User Story 2 - Headless Cloud Auto-Save (Priority: P1)
As a CI/CD operator running 10,000 parallel Monte Carlo simulations in a headless cluster, I want automatic checkpointing over simulated time to prevent data loss.

**Acceptance Criteria**:
- The engine supports a `PeriodicSave` resource triggerable via CLI flags (e.g., `--autosave-interval 3600`), dumping state to disk gracefully without interrupting the headless TDD oracle.

### User Story 3 - MCAP / ROSbag Export (Priority: P2)
As an autonomy engineer, I want the simulation to dump its historical state into an MCAP or ROSbag file, so I can visualize the entire run in Foxglove Studio or Rviz.

**Acceptance Criteria:**
- The engine can stream sequential `Transform`, `Sensor`, and `Actuator` updates natively to a `.mcap` file during the simulation run.
- Telemetry format adheres to standard robotics schemas.

### User Story 4 - In-Engine Playback (Priority: P3)
As a test engineer, I want to load a recorded MCAP mission file back into the Bevy engine.

**Acceptance Criteria:**
- Bevy ingests the recorded log and purely updates visual Transforms historically.
- The `avian` physics engine and Modelica solvers are bypassed when in `PlaybackMode`.
