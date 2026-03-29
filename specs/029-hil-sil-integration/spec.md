# Feature Specification: 029-hil-sil-integration

**Feature Branch**: `029-hil-sil-integration`  
**Created**: 2026-03-29  
**Status**: Draft  
**Input**: Digital Twin / HIL / SIL Network Implementations.

## Problem Statement
While the underlying control architecture (Spec 001) supports a generic, decoupled signal bridge (Command Mux, Actuators, Continuous/Discrete signals), the actual transport layers for external engineering validation tools have not been built.
To serve as a high-fidelity digital twin, the simulation must implement specific network and serial socket protocols to interface with Software-In-The-Loop (SIL) tools like **Fprime** and Hardware-In-The-Loop (HIL) boards like **ESP32**.

## User Scenarios & Testing

### User Story 1 - Software-In-The-Loop (SIL) (Priority: P1)
As a flight software engineer, I want to connect an external **Fprime** instance to the Bevy simulation over UDP, so my real flight code can drive the virtual rover.

**Acceptance Scenarios**:
1. **Given** a rover in Bevy, **When** Fprime sends a `SET_MOTOR` UDP packet, **Then** the proxy translates it and routes it into the Vessel's Mux.
2. **Given** the rover is moving, **When** the encoder sensor updates natively, **Then** the telemetry is packed into Fprime formats and sent back via UDP.

---

### User Story 2 - Hardware-In-The-Loop (HIL) (Priority: P3)
As a hardware engineer, I want to connect a physical microcontroller (e.g., ESP32) to the simulation.

**Acceptance Scenarios**:
1. **Given** a serial connection to a physical MCU, **When** the simulation exports sensor data through its generic Signal Bridge, **Then** the `SerialBridgeNode` streams it over USB/Serial and the MCU reacts in real-time.

---

### User Story 3 - ROS2 Subsystem Emulation (Priority: P2)
As a robotics engineer, I want the simulation to act as a native ROS2 node, exposing topics and services, so I can test my ROS2 logic stack without altering its network dependencies.

**Acceptance Scenarios**:
1. **Given** a running ROS2 network, **When** the spatial simulation is running, **Then** the engine natively emulates ROS2 compatibility (e.g., via native DDS or bridge protocols) and seamlessly publishes/subscribes to external ROS2 nodes.

## Requirements

### Functional Requirements
- **FR-001**: **Transport Layer Abstraction**: The engine MUST implement a generic transport layer capable of routing actuator/sensor signals over various protocols, including but not limited to UDP, TCP, WebSockets, and physical Serial ports.
- **FR-002**: **Specific Protocol Implementations**: Out of the box, the engine MUST implement at least a UDP/TCP network socket layer for SIL testing (e.g. Fprime, cFS) and a Serial port interface to receive commands from physical hardware (e.g. ESP32).
- **FR-003**: **Time Synchronization (Lockstep)**: The transport layer MUST provide network synchronization modes to ensure the external software controller and Bevy physics ticks stay in strict lockstep, pausing Bevy if the external controller lags.
- **FR-004**: **Protocol Decoding**: Implement protocol parsers (e.g., Fprime framing, Mavlink) specifically tailored for space software protocols.
- **FR-005**: **ROS2 / DDS Emulation**: The connectivity layer MUST natively emulate ROS2 node behavior (e.g., by integrating a native Rust DDS implementation or an equivalent protocol emulation bridge) so external ROS networks treat the engine as a standard dependency.

### Key Entities
- **UDPBridgeNode**: A system handling UDP sockets sending/receiving the generic actuator/sensor streams.
- **SerialBridgeNode**: A system handling USB/Serial port bindings.
- **LockstepOrchestrator**: Subsystem that pauses Bevy execution if the SIL controller misses a time tick deadline.

## Success Criteria
- **SC-001**: **Latency**: Bridge network communication adds less than 5ms of latency to the control loop.
- **SC-002**: **Execution Integrity**: Bevy can run in locked simulation time synced perfectly to an external Fprime container with zero dropped frames.
