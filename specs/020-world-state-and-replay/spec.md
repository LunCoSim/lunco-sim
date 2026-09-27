# Feature Specification: 020-world-state-and-replay

**Feature Branch**: `020-world-state-and-replay`
**Created**: 2026-03-29
**Status**: Partial.
- **Built:** the *document* journal (`lunco-twin-journal` + `lunco-doc-bevy`) — ops for
  `DomainKind::{Usd, Modelica, Script, Shader, Experiment, ObstacleField, ToolLibrary, Timeline}`,
  each with its inverse, undo/redo, cross-peer merge, `to_bytes` persistence, and **document-level**
  replay (journal → document → scene projection).
- **NOT built — US3 (Deterministic Replay):** there is no complete, durable whole-session input log or playback consumer. `#[Command]`s are not
  comprehensively journaled. HTTP, MCP, and Rhai transport calls use `api_command_dispatcher`, while
  UI and subsystem code can also trigger registered typed command events directly. The generated
  `CommandOccurred` projection carries API transport or Rhai execution origin; scenario Rhai origins
  include the stable actor `GlobalEntityId`, but this general projection does not retain typed
  parameters, target, admitted generation, effective tick, or per-tick sequence.
  External API, application-Rhai, and direct typed `SimulateIntentEdge` submissions, external
  held/released `SimulateIntent` changes for fixed-simulation targets, and raw-file runtime spawns
  share a bounded fixed-tick input queue. Admission assigns the committed scene generation,
  effective `SimTick`, and shared per-tick sequence before delivery; the owner validates generation
  and target again at commit. Edge acknowledgements, `CausalTrace`, and `intent.edge` retain edge
  correlation and admission; held-command acknowledgements and `intent.hold` retain the same
  held-input correlation and stamp. Raw-file spawn records also retain their producer, scene root,
  active frame, catalog entry, exact `f64` pose, correlation, and reserved root `GlobalEntityId`.
  Document-backed Twin spawns instead record `ApplyUsdOps` in the document journal.
  The fixed-step controller captures physical `ActionState<UserIntent>` as a by-value semantic
  frame. Admission requires the local input `SessionId`, target `GlobalEntityId`, and committed
  scene generation; it receives the current `SimTick` and a sequence from the shared per-tick
  allocator before control translation. Missing admission facts or duplicate target/session order
  keys hold the input with a structured runtime error instead of using world-local entity bits.
  When explicitly active, `SessionInputStream` retains bounded physical, semantic, and raw-spawn
  records with their typed producer identity, stable target, generation, tick, sequence, and payload.
  `SessionInputRecord::validate` checks canonical payload names, stable stamps, producer/payload
  pairing, and spawn-pose invariants before retention. A malformed record, capacity limit, or order
  violation stops capture visibly while preserving prior records. Typed commands start, stop, or
  clear capture, and `ReadSessionInputStream` returns the records through the typed API.
  Deterministic simulation-Rhai actions remain derived behavior, and local-embodiment input remains
  on the interaction cadence. Other typed command payloads remain outside capture. The capture is
  in memory only: it has no durable writer or playback consumer. The per-vessel `InputFrame` log
  retains latched `SetPorts` setpoints for opt-in owned-body prediction rollback; it is not a
  persistent whole-session log. Runtime actions such as `AcquireControl`, `DriveRover`, external
  `SetPorts`, terrain operations, and time control therefore cannot be reconstructed as a session
  from the current Twin journal and input capture. Reopening a Twin restores *document* state only.
  See [`docs/architecture/command-journal.md`](../../docs/architecture/command-journal.md) for the
  separate authored-document and session-input lifecycles.
- **NOT built — US1/US2/US4/US5:** no ECS `WorldSnapshot`, no `PeriodicSave`, no MCAP/ROSbag export or
  playback.
**Input**: Unified ECS State Persistence, check-pointing, deterministic replay, MCAP streaming, and replaying missions.

## Problem Statement
A digital twin of a lunar base must run for months of simulated time and be aggressively debugged post-mortem. We need a unified architectural standard to serialize the entire Bevy ECS world (live-checkpointing), record time-series telemetry to disk for external viewers (ROSbags/MCAP), provide **deterministic replay** for debugging, and load those files back into the engine for visual playback.

## User Scenarios

### User Story 1 - Mid-Mission Checkpointing (Priority: P1)
As a mission operator, I want to save the current state of my lunar base (including internal Modelica states and ECS Transforms), so I can resume exactly where I left off.

**Acceptance Criteria**:
- The engine implements a `WorldSnapshot` system executing bincode binary serialization on all components marked with a `Persistent` trait.
- Loading a save performs a "Warm Start" of external solvers (`rumoca`) preventing numerical spikes during the first resumed frame, ensuring mathematical state continuity.

### User Story 2 - Headless Cloud Auto-Save (Priority: P1)
As a CI/CD operator running 10,000 parallel Monte Carlo simulations in a headless cluster, I want automatic checkpointing over simulated time to prevent data loss.

**Acceptance Criteria**:
- The engine supports a `PeriodicSave` resource triggerable via CLI flags (e.g., `--autosave-interval 3600`), dumping state to disk gracefully without interrupting the headless TDD verifier.

### User Story 3 - Deterministic Replay (Priority: P1)
As a test engineer, I want the simulation to reproduce the same authoritative tick and input ordering from a recorded run, so that I can reliably reproduce and debug a run on a supported deterministic execution profile.

**Acceptance Criteria:**
- The simulation records external authoritative inputs with their source type, stable source and target identities, admitted scene generation, effective simulation tick, and stable per-tick sequence. The log also identifies the admitted Twin/source revisions, engine build, solver profile, and random seeds needed to interpret the run.
- Replay submits those inputs through the same typed owner boundaries in the same order. Deterministic Rhai, Modelica, and physics behavior is re-derived from the recorded inputs; derived commands and worker completion timing are not recorded as additional inputs.
- Exact state equality is claimed only for engine and domain profiles that declare deterministic numerical behavior. A profile that cannot promise bitwise equality, including adaptive numerical solvers or unsupported cross-platform execution, reports that limit instead of presenting bitwise equality as guaranteed.
- For a profile that declares deterministic numerical behavior, enforce fixed-timestep physics (`004`), deterministic system ordering, and seeded RNG for all stochastic processes (sensor noise, packet loss).
- Divergence detection: the replay engine can compare typed state digests at simulation-tick boundaries and report the first divergent tick and affected owner. A digest is not used as a substitute for the ordered input log.

### User Story 4 - MCAP / ROSbag Export (Priority: P2)
As an autonomy engineer, I want the simulation to dump its historical state into an MCAP or ROSbag file, so I can visualize the entire run in Foxglove Studio or Rviz.

**Acceptance Criteria:**
- The engine can stream sequential `Transform`, `Sensor`, and `Actuator` updates natively to a `.mcap` file during the simulation run.
- Telemetry format adheres to standard robotics schemas.

### User Story 5 - In-Engine Playback (Priority: P3)
As a test engineer, I want to load a recorded MCAP mission file back into the Bevy engine.

**Acceptance Criteria:**
- Bevy ingests the recorded log and purely updates visual Transforms historically.
- The `avian` physics engine and Modelica solvers are bypassed when in `PlaybackMode`.
