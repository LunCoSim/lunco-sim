# Feature Specification: 004-time-and-integrators

**Feature Branch**: `004-time-and-integrators`
**Created**: 2026-03-29
**Status**: Draft
**Input**: Time decoupling requirements, Lockstep synchronization, and Pluggable Mathematical Integrators (Solvers).

## Problem Statement
Bevy's default `Time` resource is bound to the system clock (real time) and uses implicit global stepping for its systems. To serve as a digital twin and testing platform, the simulation must control its own time passage (`dt`) dynamically. It must be able to run "Faster than Real-time" for ML/Orbital mechanics, or in "Lockstep" for debugging Fprime/ROS nodes. Because time (`dt`) is the fundamental variable for physics simulations, the engine must also support **pluggable mathematical solvers/integrators** allowing users to dictate *how* state progresses over time.

## User Scenarios

### User Story 1 - Fast-Forward Headless Execution (Priority: P1)
As a machine learning or astrodynamics engineer, I want the simulation to run as fast as my CPU allows without dropping physics ticks, so I can simulate thousands of lunar days in minutes.

**Acceptance Criteria:**
- The engine ignores the 60hz VSync cap rendering limit.
- The `Time::delta()` passed to systems is a fixed numeric constant rather than a measurement of wall-clock time.

### User Story 2 - Lockstep Time Sync (Priority: P1)
As a flight software engineer testing SIL nodes (`029`), I want the Bevy simulation to wait for my Fprime FSW to process the last sensor frame before stepping forward.

**Acceptance Criteria:**
- The engine pauses its primary `FixedUpdate` loop until an external "Step" command is received.
- External software receives the simulated timestamp to ensure strict deterministic log alignment.

### User Story 3 - Pluggable Solver & Integrator Architecture (Priority: P1)
As a systems modeler, I want to assign different mathematical integrators (e.g., RK4, Fast-Euler, CVODE) to different entities depending on their need for precision vs performance.

**Acceptance Criteria:**
- The engine implements a global generic `trait Integrator` governing how positional or scalar states update over `dt`.
- External plugins (like Modelica `014` or GMAT `022`) implement this trait, enabling them to hot-swap out local physics bounds on an entity mid-flight.
