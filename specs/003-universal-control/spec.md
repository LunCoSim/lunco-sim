# Feature Specification: 003-universal-control

**Feature Branch**: `003-universal-control`  
**Created**: 2026-03-29  
**Status**: Draft  
**Input**: Digital Twin / HIL / SIL Architecture. Simulation as a "Physical Plant" decoupled from controlling logic.

## Problem Statement
To serve as a high-fidelity digital twin for a lunar colony, the simulation must act as a **Physical Plant**. Control logic (Fprime, cFS, or custom scripts) must be decoupled from the physics engine to allow for Software-In-The-Loop (SIL) and Hardware-In-The-Loop (HIL) testing.

## User Scenarios & Testing

### User Story 1 - Software-In-The-Loop (SIL) (Priority: P1)
As a flight software engineer, I want to connect an external **Fprime** instance to the Bevy simulation, so that my real flight code can drive the virtual rover.

**Acceptance Scenarios**:
1. **Given** a rover in Bevy, **When** Fprime sends a `SET_MOTOR` UDP packet, **Then** the corresponding `MotorActuator` in Bevy applies torque.
2. **Given** the rover is moving, **When** the encoder sensor updates, **Then** the telemetry is sent back to Fprime via UDP.

---

### User Story 2 - Manual Override (Priority: P2)
As an operator, I want to manually drive a rover using a keyboard, while the simulation treats my inputs as if they were signals coming from an internal "Shadow Controller."

**Acceptance Scenarios**:
1. **Given** an Avatar possesses a rover, **When** WASD is pressed, **Then** the local "Input Bridge" translates these to raw actuator signals.

---

### User Story 3 - Hardware-In-The-Loop (HIL) (Priority: P3)
As a hardware engineer, I want to connect a physical microcontroller (e.g., ESP32) to the simulation, so that the physical hardware can "feel" the virtual environment.

**Acceptance Scenarios**:
1. **Given** a serial connection to a physical MCU, **When** the simulation exports sensor data, **Then** the MCU receives it and reacts in real-time.

## Requirements

### Functional Requirements
- **FR-001**: **Actuator Components**: All physical work (torque, thrust, light) MUST be handled by "dumb" Actuator components that respond only to raw numerical signals (e.g., `-1.0 to 1.0` or Voltage).
- **FR-002**: **Sensor Components**: All telemetry (IMU, Encoders, GPS, Power) MUST be captured by Sensor components that make data available for export.
- **FR-003**: **Signal Bridge**: The system MUST implement a modular Bridge that can route signals between Bevy and external sources (UDP, Shared Memory, or Internal Rust logic).
- **FR-004**: **Command Mux (Multiplexer)**: Every controllable entity MUST have a priority-based multiplexer to handle conflicting signals (e.g., Manual Override vs. Internal Autopilot).
- **FR-005**: **Future Compatibility**: The Bridge architecture SHOULD be designed to eventually support industry standards like ROS 2 / SpaceROS (see Feature 008).
- **FR-005**: **Time Synchronization**: The Bridge MUST support synchronization modes to ensure the external controller and Bevy physics stay in sync (critical for SIL).

### Key Entities
- **Plant (The Vessel)**: The collection of Actuators, Sensors, and Physics.
- **Controller (The Brain)**: The logic (internal or external) that processes sensors into commands.
- **Bridge**: The interface layer handling the I/O between Plant and Controller.

## Success Criteria
- **SC-001**: **Latency**: Bridge communication adds less than 5ms of latency to the control loop.
- **SC-002**: **Transparency**: A Controller cannot distinguish between a virtual "Plant" (Bevy) and a real physical rover if the bridge protocol matches.
- **SC-003**: **Modularity**: Swapping an internal Rust controller for an external Fprime instance requires zero changes to the Rover's physical components.
