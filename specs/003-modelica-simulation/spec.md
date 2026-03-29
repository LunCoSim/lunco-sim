# Feature Specification: Modelica Subsystem Simulation

## Problem Statement
The rover's internal physical state, such as power generation from solar panels and battery depletion from movement, needs to be rigorously simulated based on formulas. We need to integrate **rumoca** (a native Rust Modelica compiler/runtime) to run these dynamic calculations in sync with the Bevy frame loop. Using native Rust ensures we can seamlessly compile the simulation to WASM later.

## User Stories

### Story 1: Subsystem Execution
As a subsystem engineer
I want the rover's battery level to be calculated by a Modelica simulation running via rumoca
So that environmental factors reliably affect rover behavior without relying on external C binaries.

**Acceptance Criteria:**
- The engine uses `rumoca` to define and execute the solar power physics directly in Rust.
- The rover has an internal `.battery_level` that changes dynamically based on the rumoca output (e.g., depletes when driving, recharges when stationary in the sun).
- The simulation uses fixed timesteps (`FixedUpdate`) to ensure the Modelica math is deterministic and accurate.
