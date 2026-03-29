# Feature Specification: 008-modelica-simulation

## Problem Statement
The rover's internal physical state (power, thermal, etc.) requires rigorous mathematical modeling. We will integrate **Modelica** models as "Virtual Sensors/Actuators" that run in sync with the Bevy frame loop. This acts as the translation layer between the 3D graphics engine and the 1D mathematical domain.

## User Stories

### Story 1: Multi-Domain Physics Translation (Priority: P1)
As a systems engineer, I want the 3D environmental conditions (e.g., raycasted sunlight on solar panels) to feed dynamically into the 1D Modelica equations, so that the rover's battery accurately reflects its location on the moon.

**Acceptance Criteria:**
- The Bevy engine executes environmental queries (e.g., raycasting to the sun from the rover) and calculates a scalar Illumination value.
- This scalar data is piped through the `rumoca` runtime as an input to the Modelica thermal and power equations.

---

### Story 2: Subsystem Closure (The "Virtual Plant")
As a subsystem engineer, I want the rover's battery level to be calculated by a Modelica simulation that reads the current `MotorActuator` power draw and outputs the `BatteryLevel` sensor data.

**Acceptance Criteria:**
- A Modelica model is integrated via `rumoca` (Rust-Modelica runtime).
- The model reads signals from the Bevy `CommandMux` (Actuators) and feeds its results into the `Sensor` components.
- Simulation uses `FixedUpdate` to ensure the Modelica math is deterministic and accurate.
