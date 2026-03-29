# Feature Specification: 006-modelica-simulation

## Problem Statement
The rover's internal physical state (power, thermal, etc.) requires rigorous mathematical modeling. We will integrate **Modelica** models as "Virtual Sensors/Actuators" that run in sync with the Bevy frame loop.

## User Stories

### Story 1: Subsystem Closure (The "Virtual Plant")
As a subsystem engineer, I want the rover's battery level to be calculated by a Modelica simulation that reads the current `MotorActuator` power draw and outputs the `BatteryLevel` sensor data.

**Acceptance Criteria:**
- A Modelica model is integrated via `rumoca` (Rust-Modelica runtime).
- The model reads signals from the Bevy `CommandMux` (Actuators) and feeds its results into the `Sensor` components.
- Simulation uses `FixedUpdate` to ensure the Modelica math is deterministic and accurate.
