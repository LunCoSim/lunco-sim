# Feature Specification: 014-modelica-simulation

## Problem Statement
The rover's internal physical states (power, thermal, fluid flow) require rigorous, continuous 1D mathematical modeling. We will integrate **Modelica** models via the `rumoca` runtime as "Virtual Sensors/Actuators" that run synchronously inside the Bevy frame loop. 

**This is for Native Modeling:** Engineers can write Modelica code directly for the simulation to define custom subsystem logic. This is distinct from external FMI/FMU black boxes or pre-compiled chemistry kernels; Modelica here operates as a **native mathematical expansion** of Bevy’s physics.

## User Stories

### Story 1: Multi-Domain Physics Translation (Priority: P1)
As a systems engineer, I want the 3D environmental conditions (e.g., raycasted sunlight on solar panels) to feed dynamically into the 1D Modelica equations natively.

**Acceptance Criteria:**
- The Bevy engine executes environmental queries and calculates a scalar Illumination value.
- This scalar data is piped through the `rumoca` runtime natively within Bevy's `FixedUpdate` as an input to the Modelica thermal and power equations.

### Story 2: Subsystem Closure (The "Virtual Plant")
As a subsystem engineer, I want the rover's battery level to be calculated by a Modelica simulation that reads the current `MotorActuator` power draw and outputs the `BatteryLevel` sensor data.

**Acceptance Criteria:**
- The `rumoca` runtime executes natively in Bevy.
- The model reads signals from the Bevy `CommandMux` and feeds its mathematical results into the `Sensor` components.

### Story 3: Generic Physics Mutators (Hard Engineering Interop)
As a physics engineer, I want the internal state of the Modelica simulation to dynamically alter the structural properties of the outward Bevy object.

**Acceptance Criteria:**
- Modelica holds mutator authority over generic `avian` structural components (e.g., altering a tire's `Friction` if Modelica dictates it has frozen).
- This operates via a generic ECS Publisher/Subscriber framework.

### Story 4: Spatial Thermodynamics (Bi-Directional LOD Data Flow)
As a thermal systems engineer, I need my Modelica heat-rejection equations to automatically understand spatial occlusion dynamically.

**Acceptance Criteria:**
- Bevy computes a "Sky View-Factor" geometrically and streams this spatial occlusion coefficient dynamically back into `rumoca`.
- **Adjustable LOD:** The engine provides adjustable spatial-calculation modes to spare CPU cycles (e.g., `Low Quality`: Vector Dot-math / `Medium Quality`: 16-ray `avian` hemispherical casts / `Max Quality`: Delegated to GPU Compute Shaders). This controllable fidelity in both ways must be fundamentally integrated with the Modelica runtime.
