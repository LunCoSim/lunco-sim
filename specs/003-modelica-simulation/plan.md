# Implementation Plan: Modelica Subsystem Simulation

## Technology Stack
- **Modelica Bridge:** FMI integration via the `fmi-rs` crate (Functional Mock-up Interface).
- **Engine:** Bevy ECS

## Architecture
- **FMU Loader Component:** A Bevy component storing the active FMI instance.
- **Subsystem Step System:** A Bevy update system that steps the FMU forward by `Time::delta_seconds()`, passing in Bevy state (like `speed`) as inputs to the FMU, and mapping the FMU outputs (like `battery_level`) back onto the Bevy components.
