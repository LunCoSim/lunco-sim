# Feature Specification: 001-vessel-control-architecture

**Feature Branch**: `001-vessel-control-architecture`
**Created**: 2026-03-29
**Status**: Draft
**Input**: Integrated Physical Plant, Universal Signal Bridge, and Input Remapping.

## Problem Statement
To serve as a high-fidelity digital twin, LunCoSim must treat all simulated entities (rovers, satellites, factories) as **Physical Plants** that are strictly decoupled from their controlling logic. We need a unified architecture that handles:
1. **Physical Actuators & Sensors**: "Dumb" components that perform work or gather data.
2. **Universal Signal Bridge**: A routing layer for connecting internal (Rust/Lua) or external (Fprime/ROS/HIL) controllers.
3. **Semantic Input Mapping**: A flexible system to map physical hardware (Keyboards, Joysticks) to simulation actions.
4. **Double-Precision Physics**: A mathematical foundation capable of solar-system-scale simulation without precision loss.

## User Scenarios

### User Story 1 - Physical Plant & Double-Precision (Priority: P1)
As a developer, I want to spawn a 1.5kg rover that is physically "dumb"—it responds to torque signals and gathers IMU data natively in `f64` (double precision).

**Acceptance Criteria:**
- Rover uses `avian3d` (or custom f64) Rigidbody with dynamic mass/inertia.
- **Camera-Relative Rendering**: Engine binds Camera near `(0,0,0)` to prevent visual jitter in `f32` GPU space while using `f64` for world transforms.
- `WheelActuator` and `ImuSensor` components are present and respond only to raw numerical signals.

---

## Architecture: The Hierarchical Control Pipeline

To support both high-level user intent and granular motor control, the pipeline is expanded into a hierarchical system:

1.  **Input/Script Layer**: Captures raw events or pre-planned sequences (e.g., WASD, a Lua script, or a timed `CommandSequence`).
2. **Coordination Layer (The Brain)**: *Optional* "Controller" plugins that translate high-level intents (e.g., "Drive Forward") into individual actuator signals.
    - **Future Capability (Closed-Loop/PID)**: The architecture supports implementing PID (Proportional-Integral-Derivative) logic to dynamically adjust **Actuator** signals based on **Sensor** feedback, but this is NOT required for the initial MVP.
3. **Signal Layer (The Bus)**:

    - **Tag-Based Addressing**: Signals can be sent to a specific `ActuatorID`, a `GroupTag` (e.g., `Wheels_Left`), or the entire `Vessel`.
    - **Priority Override**: Low-level signals (e.g., "Force Motor #4 to 100%") can have a higher priority in the `CommandMux` than high-level coordination signals, allowing for "Agile" troubleshooting and complex maneuvers.
4.  **Actuator Layer (The Hardware)**: "Dumb" components with **Metadata** (e.g., `MotorType`, `MaxTorque`, `EfficiencyCurve`).

### User Story 5 - Command Sequences & Planned Maneuvers (Priority: P2)
As an autonomy engineer, I want to send a JSON-formatted "Maneuver Plan" to a rover, where it executes a timed sequence of motor forces across different wheels to navigate a complex slope.

**Acceptance Criteria:**
- The `CommandMux` supports a `TimedBuffer` for incoming signals.
- **Actuator Metadata Aware**: Controllers can query an actuator's `MaxTorque` before sending a signal to ensure realistic physical behavior.
- **Granular Override**: An operator can "pin" a specific wheel to 0.0 torque via the UI, even while the high-level "Drive" command is active.

### User Story 2 - Semantic Action Mapping & Continuous Input (Priority: P1)
As a user, I want to map my keyboard (WASD) or a Joystick to semantic actions, and have the rover respond continuously as long as I hold the button.

**Acceptance Criteria:**
- The engine uses an input abstraction layer (e.g., `leafwing-input-manager`).
- **Stateful Input**: Holding 'W' generates a continuous `Pressed` state in the `ActionState` component.
- **Signal Translation**: A bridge system maps the `Pressed` state of an action to a constant stream of signals sent to the vessel's `CommandMux` every `FixedUpdate` tick.
- Users can define bindings (e.g., `W` -> `Throttle: 1.0`) and remap them via a UI config.


---

### User Story 3 - Universal Signal Bridge (SIL/HIL) (Priority: P2)
As a flight software engineer, I want to drive the virtual rover using an external **Fprime** instance or a physical **ESP32** microcontroller.

**Acceptance Criteria:**
- **Signal Bridge**: A modular I/O layer routes signals between Bevy and external sources (UDP/Serial).
- **Transparency**: The Controller logic cannot distinguish between a virtual "Plant" and a real physical rover if the bridge protocol matches.
- **Lockstep Sync**: Support for synchronization modes to ensure physics stays in sync with external controllers.

---

### User Story 4 - Command Multiplexing (Priority: P2)
As an operator, I want to manually override an autonomous script by simply pressing a key, where my input takes priority over the script.

**Acceptance Criteria:**
- Every vessel has a `CommandMux` (Multiplexer).
- Signals from "Manual Override" have a higher priority than "Autopilot" or "Remote Fprime."
- The `Mux` resolves conflicts and sends the winning signal to the `Actuators`.

## Requirements

### Functional Requirements
- **FR-001**: **Actuator Components & Addressing**: All physical work MUST be handled by components that respond only to raw numerical signals. Every actuator MUST be uniquely addressable via the `CommandMux` to allow for independent control of individual wheels, thrusters, or joints.
- **FR-002**: **Sensor Components**: All telemetry MUST be captured by components that make raw data available for export.
- **FR-003**: **Propagator Swapping**: The architecture MUST allow swapping an `AvianRigidBody` for a `KeplerianPropagator` (On-Rails) seamlessly.
- **FR-004**: **Plugin-First & Hot-Swappable**: The core engine must be a shell; all vessel types, controllers, and actuators are modular plugins. The architecture MUST support swapping implementations (e.g., "Simple Physics Motor" vs. "High-Fidelity Modelica Motor") at runtime or via configuration without breaking the signal pipeline.
- **FR-005**: **Coordinate Agnostic**: The system must handle transformations between Bevy's Y-up and industry-standard Z-up (ROS).
- **FR-006**: **Interface-Driven Pipeline**: Each layer of the control pipeline (Input, Coordination, Signal, Actuator) MUST communicate via standardized interfaces/traits, allowing "drop-in" replacements of different algorithms (e.g., swapping a basic GNC for a Neural Network controller).

### Key Entities
- **Plant (Vessel)**: The collection of Actuators, Sensors, and Physics.
- **Controller**: The logic (Internal Rust/Lua or External SIL/HIL).
- **Bridge**: The I/O layer between Plant and Controller.
- **CommandMux**: The priority-based signal resolver.

## Success Criteria
- **SC-001**: **Latency**: Signal Bridge adds <5ms of overhead.
- **SC-002**: **Precision**: No "Physics Jitter" at Lunar distances (approx 384,000 km from origin).
- **SC-003**: **Modularity**: Swapping an internal controller for an external one requires zero changes to the vessel's physical components.
