# Feature Specification: 017-fmu-gmat-integration

**Feature Branch**: `017-fmu-gmat-integration`
**Created**: 2026-03-29
**Status**: Draft
**Input**: Server-side integration with FMUs (Functional Mock-up Units) via FMI, and astrodynamics tools like GMAT or Basilisk.

## Problem Statement
While Modelica handles internal 1D subsystem physics on the rover, complex mission dynamics (such as orbital trajectories of a relay satellite, or legacy proprietary hardware models from vendors) require connecting Bevy to specialized external solvers. Vendors often provide their black-box models as standard FMUs. Real-world mission planning relies heavily on tools like GMAT and Basilisk.

## User Scenarios

### User Story 1 - FMI Standard Co-Simulation (Priority: P2)
As a systems engineer, I want the Bevy server to execute an external manufacturer's FMU (Functional Mock-up Unit) as part of the simulation tick, so that proprietary hardware (like specialized thrusters) accurately models its behavior.

**Acceptance Criteria:**
- The engine leverages the FMI (Functional Mock-up Interface) standard to step the FMU and exchange numerical inputs/outputs with the universal `CommandMux`.
- To maintain deterministic constraints, this heavy execution happens *exclusively on the Headless Server*. Clients do not run the FMU; they merely receive the resulting physics/telemetry updates via the multiplayer prediction loop (`006`).

### User Story 2 - Astrodynamics Sync (Priority: P3)
As a mission planner, I want the positions of orbital relays and celestial bodies in my scenario to be driven by GMAT (General Mission Analysis Tool) or Basilisk, so that my lunar line-of-sight communication dropouts (`014`) reflect authentic orbital mechanics.

**Acceptance Criteria:**
- The sever subscribes to live GMAT/Basilisk ephemeris data (e.g., via TCP/IP wrapper or direct bridge).
- Bevy Entities representing satellites update their Transforms in the planetary coordinate frame strictly based on this external solver, serving as moving raycast targets.
