# Feature Specification: 023-spatial-audio

**Feature Branch**: `023-spatial-audio`
**Created**: 2026-03-29
**Status**: Draft
**Input**: Basic spatial 3D sound effects and UI telemetry alarm klaxons.

## Problem Statement
A simulation feels lifeless without auditory feedback. Operators need to hear motors straining and telemetry alarms firing. This spec covers **basic, practical audio** — engine sounds, impact effects, and Master Caution alarms.

> **Note on Reduced Scope:** Advanced vacuum-aware structural acoustic propagation (sound traveling through rigidbody chains) is deferred to a future phase. For now, audio is simplified to basic spatial sounds with distance falloff and flat UI alarms.

## User Scenarios

### User Story 1 - Basic Spatial Engine Sounds (Priority: P2)
As a rover operator, I want to hear motor sounds and impact effects spatially in 3D.

**Acceptance Criteria:**
- The engine implements spatial audio (e.g., via `bevy_kira_audio` or `bevy_audio`).
- Actuators and collision events emit basic sound effects with distance-based falloff.
- A configurable "audio distance" limit prevents sounds from playing beyond a threshold.

### User Story 2 - UI Telemetry Alarms / Master Cautions (Priority: P1)
As a mission operator, I want loud, non-spatial Master Alarms when simulated hardware approaches critical limits.

**Acceptance Criteria:**
- The engine exposes a flat (2D non-spatialized) auditory UI channel.
- If `014-modelica-simulation` calculates a battery ≤10%, or torque ≥90% of `StressLimit`, repeating klaxon warnings fire.
- Alarms are managed by the `NotificationManager` from `006-developer-experience`.

### User Story 3 - Vacuum Sound Suppression (Priority: P3 — Future)
As an acoustics engineer, I want external vacuum sounds to be suppressed based on atmospheric environment flags.

**Acceptance Criteria:**
- **Deferred.** In future phases, a Sound Emitter should only transmit audio through atmospheric or structural media. For now, basic distance falloff is sufficient.
