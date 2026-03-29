# Feature Specification: 009-authority-rbac

**Feature Branch**: `009-authority-rbac`
**Created**: 2026-03-29
**Status**: Draft
**Input**: Subsystem-level authority delegation, concurrent multi-user control, role-based access control.
**Depends on**: `003-multiplayer-core`

## Problem Statement
Basic multiplayer (`003`) provides single-operator vessel control. For professional concurrent engineering — where multiple specialists work on different subsystems of the same vessel simultaneously — we need fine-grained **authority delegation** and **role-based access control (RBAC)**.

This is what makes LunCoSim unique compared to traditional simulators: real collaborative engineering, not just "watching someone else drive."

## User Stories

### Story 1: Subsystem-Level Authority Delegation (Priority: P1)
As a vessel owner, I want to grant another engineer control over a specific subsystem (e.g., robotic arm) while I retain control of the drive system.

**Acceptance Criteria:**
- Each FSW module (Drive, Arm, Power, Comms, etc.) is independently authorizable.
- The vessel owner can grant/revoke subsystem access from the multiplayer UI.
- Multiple engineers can operate different subsystems of the same vessel concurrently.
- Example: Player A drives while Player B operates the robotic arm. Simultaneously.

### Story 2: Role-Based Access Control (Priority: P2)
As a mission commander, I want to assign roles (Owner, Operator, Observer, AI Agent) to participants, so that access is structured and auditable.

**Acceptance Criteria:**
- Authority types: `Owner | Operator | Observer | AI_Agent`.
- Owners have master authority over their vessels and can delegate subsystem access.
- Operators can command only the subsystems they are granted.
- Observers have read-only telemetry access — can watch but not command.
- AI Agents can be granted subsystem authority (e.g., autonomous power management).

### Story 3: Conflict Resolution (Priority: P2)
As a systems engineer, I want clear conflict resolution when two operators accidentally target the same actuator.

**Acceptance Criteria:**
- If two operators send conflicting commands to the same actuator, a configurable priority system resolves the conflict.
- Default priority: `Owner > Human Operator > AI Agent > Observer (blocked)`.
- Conflicts are logged and optionally surfaced as notifications via the Unified Editor (`006`).

### Story 4: Authority Audit Trail (Priority: P3)
As a mission commander, I want a log of who controlled what and when, for post-mission review.

**Acceptance Criteria:**
- All authority grants, revocations, and control actions are logged with timestamps.
- The audit trail is accessible via the REPL and exportable as part of the mission record (`020`).

## Key Entities
- **SubsystemAuthority**: Component mapping FSW modules to authorized players/agents.
- **AuthorityRole**: Enum defining access levels: `Owner | Operator | Observer | AI_Agent`.
- **ConflictResolver**: System handling simultaneous commands to the same actuator.
