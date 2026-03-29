# Feature Specification: 003-multiplayer-networking

## Problem Statement
To enable collaborative missions, multiple players must be able to view and interact with the same "Physical Plants." The networking system must synchronize **Command Signals** and **Sensor Telemetry** rather than raw physics states or inputs.

## User Stories

### Story 1: Collaborative Telemetry
As a mission participant, I want to see the sensor data from a rover controlled by another user, so that we can coordinate our actions.

**Acceptance Criteria:**
- The `Sensor` component data is synchronized across the network.
- Latency in sensor updates is minimized via state interpolation.

### Story 2: Authority & Signal Sync
As a rover operator, I want to see the effect of my commands reflected in the shared environment, and I want other users to be aware when I have "Manual Override" authority.

**Acceptance Criteria:**
- The `ControlAuthority` component and its corresponding `CommandMux` state are synchronized.
- When Player A takes control, Player B is notified that "Manual Authority" is active.
