# Feature Specification: 029-world-persistence

**Feature Branch**: `029-world-persistence`
**Created**: 2026-03-29
**Status**: Draft
**Input**: Unified State Persistence, Checkpointing, and ECS/Modelica/Chemistry snapshotting.

## Problem Statement
While `005-scenario-orchestration` handles the *initial* loading of a mission, and `020-mission-recording` handles *historical* playback, the engine lacks a way to save and resume a **Live Simulation State**. A digital twin of a lunar base must be able to run for months of simulated time, meaning we must be able to serialize the entire ECS world, the internal 1D math of Modelica solvers, and chemical species balances into a single "Checkpoint" file that can be reloaded with perfect mathematical continuity.

## User Stories

### User Story 1 - Mid-Mission Checkpointing (Priority: P1)
As a mission operator, I want to save the current state of my lunar base (battery levels, rover positions, oxygen pressures), so that I can resume the simulation exactly where I left off after a restart.

**Acceptance Criteria**:
- The engine implements a `WorldSnapshot` system.
- Upon a "Save" command, the engine serializes all components marked with a `Persistent` trait into a binary format (e.g., `bincode` or `MessagePack`).
- The saved file includes the current `SimulationTime` (Spec 004) to ensure temporal consistency.

---

### User Story 2 - Cross-Subsystem State Sync (Priority: P1)
As a systems engineer, I want the saved state to include the internal variables of the Modelica and Chemistry solvers, so that my thermal and power math doesn't "reset" to defaults when I load a save.

**Acceptance Criteria**:
- The `014-modelica-simulation` and `026-native-chemistry-simulation` plugins provide serialization hooks.
- Internal solver states (integrator history, mass fractions) are bundled into the same checkpoint file as the Bevy ECS data.
- **Warm Start**: Loading a save performs a "Warm Start" of the solvers, preventing numerical spikes during the first few frames of resumed execution.

---

### User Story 3 - Save-Game Versioning & Migration (Priority: P2)
As a developer, I want the engine to detect when a save file was created with an older version of the SysML model, so that I don't crash the simulation with incompatible entity hierarchies.

**Acceptance Criteria**:
- Checkpoint files include a `SysmlVersion` and `EngineVersion` header.
- The loader validates these headers against the current `013-sysml-integration` master specification.
- If a mismatch is detected, the engine provides a warning and attempts a best-effort migration of component data.

---

### User Story 4 - Headless Cloud Auto-Save (Priority: P1)
As a CI/CD operator running 10,000 parallel Monte Carlo simulations, I want the engine to automatically save a checkpoint every N simulated hours, so that I can recover from a server crash without losing the entire run.

**Acceptance Criteria**:
- The engine supports a `PeriodicSave` resource.
- CLI flags (e.g., `--autosave-interval 3600`) trigger background serialization to a designated directory.

## Requirements

### Functional Requirements
- **FR-001**: **The `Persistent` Component**: Any ECS component that needs to be saved MUST implement a standardized serialization trait.
- **FR-002**: **Deterministic Reloading**: Loading a save into the same engine version MUST result in a bit-perfect match of the physical and mathematical state.
- **FR-003**: **External Solver Hooks**: External plugins (Modelica, FMI, GMAT) MUST be given a pre-serialization and post-deserialization event to handle their internal memory buffers.
- **FR-004**: **Differential Saves (Future Scope)**: To save disk space, the engine should eventually support "Diff Saves" relative to the initial `005` Scenario file.

### Key Entities
- **PersistenceManager**: The central Bevy resource managing the save/load pipeline.
- **Checkpoint File**: A binary blob containing ECS, Solver, and Time state.
- **WorldSnapshot Hook**: A system event that triggers the collection of state from all plugins.

## Success Criteria
- **SC-001**: A user can save a 1,000-entity base and reload it in under 2 seconds.
- **SC-002**: Numerical Continuity: The first `FixedUpdate` tick after a load produces a delta error of <0.0001% compared to the frame before the save.
- **SC-003**: A save created in the Godot-like UI (Spec 007) can be loaded and resumed in a Headless CLI environment.
