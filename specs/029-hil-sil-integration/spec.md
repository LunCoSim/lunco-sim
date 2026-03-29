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
1. **Command Injection**: When Fprime sends a `GROUND_COMMAND`, the engine routes it **directly to the FSW Command Bus (Level 4)**, bypassing the human Avatar/Controller.
2. **Device Driver Emulation**: When Fprime writes to its virtual hardware drivers, the engine maps those IO calls **directly to the OBC Port Emulator (Level 2)**.
3. **Telemetry Fidelity**: The rover's low-level sensors (Level 1) export raw readings that Fprime interprets as if it were on real hardware.

---

### User Story 2 - Hardware-In-The-Loop (HIL) (Priority: P3)
As a hardware engineer, I want to connect a physical microcontroller (e.g., ESP32) to the simulation.

**Acceptance Scenarios**:
1. **Given** a serial connection to a physical MCU, **When** the simulation exports sensor data, **Then** the MCU reacts by sending electrical equivalents (PWM/GPIO states) **directly to the OBC Port Map (Level 2)**, which then drives the Level 1 physical motors.

## Requirements

### Functional Requirements
- **FR-001**: **Transport Layer Abstraction**: The engine MUST implement a generic transport layer capable of routing actuator/sensor signals over various protocols, including but not limited to UDP, TCP, WebSockets, and physical Serial ports.
- **FR-002**: **Specific Protocol Implementations**: Out of the box, the engine MUST implement at least a UDP/TCP network socket layer for SIL testing (e.g. Fprime, cFS) and a Serial port interface to receive commands from physical hardware (e.g. ESP32).
- **FR-003**: **Time Synchronization (Lockstep)**: The transport layer MUST provide network synchronization modes to ensure the external software controller and Bevy physics ticks stay in strict lockstep, pausing Bevy if the external controller lags.
- **FR-004**: **Protocol Decoding**: Implement protocol parsers (e.g., Fprime framing, Mavlink) specifically tailored for space software protocols.

### Key Entities
- **UDPBridgeNode**: A system handling UDP sockets sending/receiving the generic actuator/sensor streams.
- **SerialBridgeNode**: A system handling USB/Serial port bindings.
- **LockstepOrchestrator**: Subsystem that pauses Bevy execution if the SIL controller misses a time tick deadline.

## Success Criteria
- **SC-001**: **Latency**: Bridge network communication adds less than 5ms of latency to the control loop.
- **SC-002**: **Execution Integrity**: Bevy can run in locked simulation time synced perfectly to an external Fprime container with zero dropped frames.
