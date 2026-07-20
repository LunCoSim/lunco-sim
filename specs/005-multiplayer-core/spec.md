# Feature Specification: 005-multiplayer-core

**Feature Branch**: `005-multiplayer-core`
**Created**: 2026-03-29
**Status**: Implemented (historical)
**Input**: Server-authoritative networking, basic entity sync, observer presence.

## Problem Statement
Collaborative and concurrent engineering is a **core differentiator** of LunCoSim. Multiple engineers must be able to view and interact with the same simulation world simultaneously. Given the non-deterministic nature of floating-point physics, the networking system MUST employ a **Server-Authoritative Architecture with Client-Side Prediction** to ensure physical determinism and robust state reconciliation.

This spec covers the foundational networking layer: connecting, syncing entities, and basic single-operator control. Advanced authority delegation and RBAC (multi-user concurrent subsystem control) are handled in `009-authority-rbac`.

> **Note on Scope:** Multiplayer networking focuses on syncing and distributed control of users in a shared world. This is distinct from simulating network degradation (`019`), which modifies internal physics/signals and runs independently (e.g., in solo operations).

> **Architectural Decision (Simple vs Accurate):** We embrace a simplified approach for clients. We prioritize the *interactive experience*—allowing several rovers controlled by different entities to work together seamlessly—over perfect client-side accuracy. The server performs all precise, heavy-duty math, while connected users receive a "rough simulation" sufficient for operation and visual sync.

## Architecture

### Server-Client f64/f32 Split
The server is the **sole authority** for physics and maintains all entity positions in `f64` precision. Clients are rendering-only consumers:

```
Server (f64 Truth)
├── Runs avian physics, Modelica, FSW in f64
├── Knows each client's camera position (f64)
├── Computes per-client relative f32 positions
└── Sends f32 deltas to each client

Client (f32 Render-Only / Rough Simulation)
├── Receives f32 positions relative to own floating origin
├── Applies directly to Bevy Transform (zero conversion)
├── Client-side prediction uses f32 (accepting rough accuracy for speed)
└── NO f64 computation — saves CPU and bandwidth, prioritizing interactivity
```

**Bandwidth advantage:** f32 positions are half the bytes of f64. For 1000 synced entities at 60Hz, this saves ~144KB/s per client.

## User Stories

### Story 1: Server Connection & World Sync (Priority: P0)
As a participant, I want to connect to a running simulation server and see the current state of the world, so that I can join an ongoing mission.

**Acceptance Criteria:**
- The engine supports a dedicated server mode (`cargo run -- --server`) and client mode (`cargo run -- --connect <ip>`).
- Upon connection, the server sends a full world snapshot to the new client.
- The client renders all synced entities with their current positions and states.

### Story 2: Entity State Synchronization (Priority: P0)
As a mission participant, I want to see the real-time positions, orientations, and sensor data of all entities in the shared world.

**Acceptance Criteria:**
- `Transform`, `Sensor`, and `Actuator` component data is synchronized from server to all clients.
- **Port State Sync**: All `Port` (f64) values are synchronized to provide real-time hardware telemetry (e.g., motor torque, battery voltage) across all clients.
- Latency in updates is minimized via state interpolation on the client side.
- Client prediction is reconciled against server state; see FR-003 for what is shipped
  by default and what is opt-in.

### Story 3: Single-Operator Control (Priority: P0)
As a rover operator, I want to take control of a space system and have my inputs reflected in the shared world, while other users observe.

**Acceptance Criteria:**
- A `ControlAuthority` component tracks which player (if any) currently controls each space system.
- Only the player with authority can send Commands to that space system's FSW.
- When Player A takes control, all other clients are notified that the space system is "under manual control."
- Control is exclusive per-space system at this level (concurrent subsystem access is spec `009`).

### Story 4: Observer Presence (Priority: P1)
As a mission participant, I want to see the presence and activity of other observers in the world.

**Acceptance Criteria:**
- The location and orientation of other users' cameras (Avatars) are synchronized.
- Visual markers or presence indicators (e.g., "Engineer B is observing Rover A") are displayed.
- The state of which entities are currently under which user's control is broadcast globally.

### Story 5: User Profiles (Priority: P1)
As a mission participant, I want to have a user profile so that my identity and basic preferences persist across sessions.

**Acceptance Criteria:**
- Network architecture supports authenticating and synchronizing user profiles.
- Participant identities are broadcast to all peers.
- Profile data is minimal at this stage: display name, avatar color, session preferences.

## Requirements

### Functional Requirements
- **FR-001**: **Server-Authoritative Physics**: the server is the ONLY authority on physics (avian, Modelica, FSW): its state always wins, and no client result is ever adopted by the host. A client MAY run a *local, disposable* copy of avian for the bodies it predicts (the vessel it drives, and free dynamic bodies near it) — that copy exists only to hide latency and is continuously corrected against, or discarded in favour of, the server's state. It is never authoritative and never leaves the client.
- **FR-002**: **f32 Client Transport**: entity positions cross the wire as a big_space `(cell: i64, remainder: i32 @ 1 mm)` pair, which the client composes back to an absolute f64 world position; the render path is f32 relative to the floating origin.
- **FR-003**: **Client-Side Prediction and Reconciliation.** The client predicts the effect of its own input on the vessel it drives, and the shipped reconcile is **state-sync + smoothing**, NOT rollback: on each snapshot that acks a new input `seq`, the client compares *what it predicted at that seq* against *authority at that same seq* (so its legitimate latency lead cancels), and — only if that diverges past a dead-zone — eases the error into the present pose over a few acks (`lunco_core::reconcile_decision`), or hard-snaps past a gross-desync threshold. There is no re-simulation on this path, and none is required: the coupled cosim/Modelica forces a client cannot reproduce make bit-exact re-simulation of the general case impossible anyway (which is what the `NotPredictable` marker concedes).
  **Deterministic rollback (input replay) IS built, and is opt-in** (`LUNCO_ROLLBACK=1`): the client keeps the real actuation per input `seq` (`lunco_core::InputFrame`) and the whole articulated assembly's physics state per seq, rewinds the assembly onto the acked authoritative state, and re-simulates every unacked input through a dedicated `RollbackReplay` schedule + avian's `PhysicsSchedule`. It is validated headlessly by the `rollback_probe` bin (a public-state-only restore + input replay reconverges to sub-mm). It is OFF by default so it cannot regress the shipped path until it is chosen.
  **Do not state "corrections are reconciled via rollback" without qualification** — that sentence stood here while the shipped path did no re-simulation at all, and it is the reason this requirement is now written in this much detail.
- **FR-004**: **Bandwidth Efficiency**: The sync protocol MUST support delta compression (only send changed components) and configurable update rates per entity priority.

### Key Entities
- **GameServer**: The authoritative simulation host. Runs physics, FSW, and Modelica.
- **GameClient**: A rendering-only consumer. Receives f32 state, renders, and sends inputs.
- **ControlAuthority**: Component tracking which player has exclusive control of a space system.
- **PlayerProfile**: Minimal identity and session data.
