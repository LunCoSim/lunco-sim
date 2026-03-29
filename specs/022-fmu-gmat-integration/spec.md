# Feature Specification: 022-fmu-gmat-basilisk-integration

**Feature Branch**: `022-fmu-gmat-basilisk-integration`
**Created**: 2026-03-29
**Status**: Draft
**Input**: Server-side integration with FMUs (Functional Mock-up Units) via FMI, and astrodynamics tools like GMAT and Basilisk.
**Depends on**: `004-time-and-integrators` (PhysicsMode state machine)

## Problem Statement
While Modelica (`014`) handles our *internal* 1D physics transparently and the Astronomical Environment (`018`) handles our native celestial mechanics, real-world mission planning often relies on compiled, proprietary black-box hardware models supplied by vendors (FMI standard) or macroscopic celestial mechanics solvers (GMAT / Basilisk). 

**This is for Co-Simulation & External Validation:** This specification handles the integration with external engineering tools. These tools act as a "Rigorous Cross-Check" and heavy co-simulation layer, ensuring the engine's built-in physics align with validated aerospace standards (FMUs and GMAT) during mission-critical sequences.

This spec builds on the **PhysicsMode state machine** established in `004-time-and-integrators`, providing concrete implementations of the `OnRails` and `HybridBlend` modes using GMAT/Basilisk as the analytical propagator.

## User Scenarios

### User Story 1 - FMI Standard Co-Simulation (Black-Box Execution) (Priority: P2)
As a systems engineer, I want the Bevy server to execute an external manufacturer's FMU binary (Functional Mock-up Unit) as part of the simulation tick, so that proprietary hardware logic runs without exposing its source code.

**Acceptance Criteria:**
- The engine can load `.fmu` packages and provide inputs while reading outputs natively inside Bevy's execution pipeline.
- To maintain deterministic constraints, this heavy execution happens *exclusively on the Headless Server*. Clients do not run the FMU; they receive updates via the multiplayer prediction loop.

### User Story 2 - ECS Astrodynamics Handoff with Hybrid Blend (Priority: P1)
As an orbital trajectory engineer, I want the spacecraft to seamlessly transition from bouncing on local regolith to a smooth Keplerian planetary orbit calculated by GMAT, avoiding 3D game physics drift.

**Acceptance Criteria:**
- The engine uses the `PhysicsMode` component from `004`:
  - **Detection**: A spatial state machine checks crossing boundaries (e.g., Altitude > threshold).
  - **HybridBlend**: Upon boundary crossing, the entity enters `HybridBlend` mode for 3-5 seconds. Both the avian physics forces and the GMAT analytical propagator are active, with outputs weighted by `blend_factor`.
  - **OnRails**: After blending completes, the avian `RigidBody` component is removed. Position is now driven purely by GMAT/Basilisk Keplerian/astrodynamics models.
  - **Return**: When re-entering the physics bubble (e.g., landing approach), the process reverses: `OnRails → HybridBlend → FullPhysics`.
- The transition is smooth and jitter-free.

### User Story 3 - Astrodynamics Ephemeris Sync (Priority: P3)
As a mission planner, I want the positions of orbital relays and celestial bodies in my scenario to be driven by GMAT or Basilisk, so that my lunar line-of-sight communication dropouts reflect authentic orbital mechanics.

**Acceptance Criteria:**
- The server subscribes to live GMAT/Basilisk ephemeris data (e.g., via TCP/IP wrapper or direct bridge).
- Bevy Entities representing satellites update their Transforms in the planetary coordinate frame strictly based on this external solver.
- These entities permanently remain in `OnRails` PhysicsMode.
