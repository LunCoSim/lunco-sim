# Feature Specification: 003-multiplayer-core

**Feature Branch**: `003-multiplayer-core`
**Created**: 2026-03-29
**Status**: Draft
**Input**: Server-authoritative networking, basic entity sync, observer presence.

## Problem Statement
Collaborative and concurrent engineering is a **core differentiator** of LunCoSim. Multiple engineers must be able to view and interact with the same simulation world simultaneously. Given the non-deterministic nature of floating-point physics, the networking system MUST employ a **Server-Authoritative Architecture with Client-Side Prediction** to ensure physical determinism and robust state reconciliation.

This spec covers the foundational networking layer: connecting, syncing entities, and basic single-operator control. Advanced authority delegation and RBAC (multi-user concurrent subsystem control) are handled in `009-authority-rbac`.

> **Note on Scope:** Multiplayer networking focuses on syncing and distributed control of users in a shared world. This is distinct from simulating network degradation (`019`), which modifies internal physics/signals and runs independently (e.g., in solo operations).

## Architecture

### Server-Client f64/f32 Split
The server is the **sole authority** for physics and maintains all entity positions in `f64` precision. Clients are rendering-only consumers:

```
Server (f64 Truth)
├── Runs avian physics, Modelica, FSW in f64
├── Knows each client's camera position (f64)
├── Computes per-client relative f32 positions
└── Sends f32 deltas to each client

Client (f32 Render-Only)
├── Receives f32 positions relative to own floating origin
├── Applies directly to Bevy Transform (zero conversion)
├── Client-side prediction uses f32 (short-term, fine for <1s)
└── NO f64 computation — saves CPU and bandwidth
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
- Latency in updates is minimized via state interpolation on the client side.
- State prediction rollbacks are supported by tying into the `avian` ECS-native physics engine.

### Story 3: Single-Operator Control (Priority: P0)
As a rover operator, I want to take control of a vessel and have my inputs reflected in the shared world, while other users observe.

**Acceptance Criteria:**
- A `ControlAuthority` component tracks which player (if any) currently controls each vessel.
- Only the player with authority can send Commands to that vessel's FSW.
- When Player A takes control, all other clients are notified that the vessel is "under manual control."
- Control is exclusive per-vessel at this level (concurrent subsystem access is spec `009`).

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
- **FR-001**: **Server-Authoritative Physics**: ALL physics computation (avian, Modelica, FSW) MUST execute exclusively on the server. Clients MUST NOT run independent physics.
- **FR-002**: **f32 Client Transport**: The server MUST compute and transmit entity positions as f32 values relative to each client's floating origin. Clients perform zero f64 computation.
- **FR-003**: **Client-Side Prediction**: Clients MAY predict local input effects for up to 1 second using f32 arithmetic. Server state corrections are reconciled via rollback.
- **FR-004**: **Bandwidth Efficiency**: The sync protocol MUST support delta compression (only send changed components) and configurable update rates per entity priority.

### Key Entities
- **GameServer**: The authoritative simulation host. Runs physics, FSW, and Modelica.
- **GameClient**: A rendering-only consumer. Receives f32 state, renders, and sends inputs.
- **ControlAuthority**: Component tracking which player has exclusive control of a vessel.
- **PlayerProfile**: Minimal identity and session data.
