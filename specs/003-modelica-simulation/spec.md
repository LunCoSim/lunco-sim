# Feature Specification: Modelica Subsystem Simulation

## Problem Statement
The rover's internal physical state, such as power generation from solar panels and battery depletion from movement, needs to be rigorously simulated based on formulas. We need to integrate Modelica FMI (Functional Mock-up Interface) to run these dynamic calculations in sync with the Bevy frame loop.

## User Stories

### Story 1: Subsystem Execution
As a subsystem engineer
I want the rover's battery level to be calculated by a Modelica FMU simulation in real-time
So that environmental factors reliably affect rover behavior.

**Acceptance Criteria:**
- The engine uses an FMU to define the solar power physics.
- The rover has an internal `.battery_level` that changes dynamically based on the FMU output (e.g., depletes when driving, recharges when stationary in the sun).
