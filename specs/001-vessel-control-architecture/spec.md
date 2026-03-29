# Feature Specification: 001-unified-control-interaction

**Feature Branch**: `001-unified-control-interaction`
**Created**: 2026-03-29
**Status**: Draft
**Input**: 5-Layer Action-to-Actuator Technical Architecture (Avian & Leafwing).

## Problem Statement
To serve as a high-fidelity digital twin, LunCoSim treats all simulated entities as **Physical Plants** strictly decoupled from their controlling logic. The human operator interacts with this world through an **Avatar** that possesses an entity-specific **Controller**, bridging the gap between human intent and robotic flight software.

### Core Principles
- **Plugin First**: Every feature (OBC, FSW, Propagator) MUST be implemented as a modular Bevy Plugin.
- **Hot-Swappable**: The architecture MUST support runtime replacement of any Level 2 (OBC), Level 3 (FSW), or Level 4 (Controller) component.
- **Decoupled Visual/Logical Part**: The simulation logic (OBC, FSW, Plant Physics) MUST be strictly decoupled from the visual representation (Meshes, Lights, Materials). This is the technical foundation for **Headless Mode**, allowing the reactor to function perfectly even when visual plugins are disabled.
- **Simulator as Mechanism**: The core engine is a "dumb" reactor; intelligence is pluggable.

---

## Technical Stack
- **Physics Engine**: **Avian3D** (configured for `f64` double-precision).
- **Input Action Manager**: **Leafwing Input Manager** (for Godot-style Action Mapping).
- **Rendering**: Bevy `PbrBundle` with **Camera-Relative Origin Shifting** to maintain `f32` GPU stability.

---

## The 5-Layer Control Architecture (Technical Mapping)

| Layer | Concept | Implementation (Bevy ECS) | Role |
| :--- | :--- | :--- | :--- |
| **5** | **Action** | **`ActionState<VesselAction>`** | Human intent (e.g., `MoveForward`). Managed by Leafwing. |
| **4** | **Controller**| **`VesselController` System** | Reads `ActionState`, emits **`Events<Command>`**. Mapping: `Action::W` -> `CMD_DRIVE(1.0)`. |
| **3** | **FSW** | **`FlightSoftware` Plugin** | Reads `Events<Command>`, calculates logic, and writes to **`PinState`**. Consumes sensor **`PinState`** for fusion. |
| **2** | **OBC** | **`OBC_Emulator` Entity** | Collection of **`PinState`** components (Output: PWM; Input: Digital/Analog Sensor Signals). |
| **1** | **Plant** | **`Actuator / Sensor`** | Physics-ready components. Sensors write to **`PinState`** (Input). Actuators read from **`PinState`** (Output). |

---

## Technical Validation Scenarios

### User Story 0 - Stage 1 Baseline (MVP) (Priority: P0)
As a developer, I want to validate the 5-layer architecture with a concrete "Stage 1" rover experience that works in the browser.

**Environment Setup:**
- **Terrain**: A static 1x1km flat plane.
- **Physics Feature**: A single physical **ramp** sufficient for launching the rover into the air.
- **Lighting**: A single directional light source representing the Sun.

**Vessel Configuration (The Baseline Rover):**
- **Body**: A primitive box collider.
- **Wheels**: 4 independent wheel colliders.
- **Actuators (6 Total)**:
    - **4x Drive Motors**: 1 per wheel (Forward/Backward torque).
    - **2x Steering Motors**: Independent steering on the 2 front wheels.

**Interaction Logic:**
- **Initial State**: Avatar starts unpossessed (Free-cam).
- **Possession Handover**: Avatar can "Enter" the rover to enable control.
- **Controls**:
    - **W/S**: Drive (Forward/Backward torque).
    - **A/D**: Steer (Left/Right front wheel rotation).
    - **Space**: **Brake** (Immediately stop wheel rotation).
    - **Idle**: **Inertia** (Wheels transition to free-rolling when no keys are pressed).

**Technical Acceptance Criteria:**
- **Visual Accuracy**: Wheels MUST have a basic shader/texture allowing visual verification of rotation.
- **Speed Matching**: Wheel rotation speed MUST physically match the rover's velocity (including air-borne inertia).
- **Architecture Validation**: MUST pass all **[General Testing Framework (000-TEST)](file:///home/rod/Documents/lunco/lunco-sim-bevy/specs/000-testing-framework/spec.md)** compliance rules.
- **WASM Compliance**: The entire scenario MUST be runnable in a modern web browser.

---

## Testing & Validation Mandate

To ensure the 5-layer architecture is computationally verifiable, all development MUST adhere to the **[General Testing Framework (000-TEST)](file:///home/rod/Documents/lunco/lunco-sim-bevy/specs/000-testing-framework/spec.md)**. This includes:

1. **Architecture Compliance**: No layer-skipping or prohibited dependency flows.
2. **Component-Level Logic Tests**: Mandatory unit tests for every Layer 2, 3, and 4 component (FR-010).
3. **Headless Oracle Validation**: Automated, GUI-less physical state verification (Movement, Braking, Stability).

---

## Implementation Patterns

### 1. Camera-Relative Rendering (Origin Shifting)
To prevent visual jitter (floating-point error) when the camera is thousands of kilometers from the world origin:
- **CPU Space**: Entity transforms are stored in **`f64`** (Global Space).
- **GPU Space**: The engine maintains a **`Camera_Origin`**. All mesh transforms sent to the GPU are calculated as `Global_Pos - Camera_Origin`, keeping them near `(0,0,0)` in **`f32`** space.

### 2. Coordinate Interoperability (Aerospace Standards)
Bevy defaults to **Y-Up**. External aerospace software (Fprime, ROS) defaults to **Z-Up**.
- **Conversion Matrix**: The FSW-to-OBC driver handles the implicit rotation mapping:
  - `Bevy (+X, +Y, +Z)` -> `Aerospace (+X, +Z, -Y)`.
  - All "North/Up" semantic commands MUST be resolved at the **Controller/FSW** boundary.

---

## User Scenarios

### User Story 1 - Avatar Possession & Controller Mapping (Priority: P1)
As a user, I want my Avatar to fly through the world and 'Possess' a rover, whereby my `ActionState<VesselAction>::MoveForward` is automatically mapped by the rover's **VesselController** into valid drive commands for its flight software. (Note: The Controller is logically 'boring'—it only passes intent; the FSW handles the actual steering/logic).

### User Story 2 - CLI Command Overhaul (Priority: P1)
As a mission operator, I want to bypass the Avatar and Controller by sending a raw `CMD_REBOOT` directly to the FSW via the command-line interface (CLI).

### User Story 3 - Automated Success Oracles (Priority: P1)
As a QA lead, I want to run 1,000 headless simulations of a rover landing to gather statistical success data without GPU overhead or windowing requirements.

---

## Requirements

### Functional Requirements
- **FR-001**: **Unified Action Bus**: Every Avatar executes an Action Bus (using `leafwing-input-manager` style mapping).
- **FR-002**: **Command Handover**: The Flight Software MUST expose a unified interface for receiving **Commands** from the Controller, CLI, or internal autonomous sequences.
- **FR-003**: **OBC Hardware Emulation**: The OBC MUST maintain a persistent state of its I/O registers, allowing telemetry probes to read real-time "Pin Levels".
- **FR-005**: **f64 Physical Fidelity**: All physics calculations MUST be performed in **f64 (double precision)** using Avian3D.
- **FR-006**: **Actuator Metadata Awareness**: Every Level 1 component MUST expose `MaxTorque`, `MinTravel`, and `Addressing` for control validation.
- **FR-007**: **Headless Mode Compliance**: The core simulation MUST be capable of functioning without `RenderPlugin` or `WindowPlugin`.
- **FR-008**: **Actuator Multi-Mapping**: FSW MUST support simultaneous control of multiple actuator types (Drive vs. Steering) from a single user intent (e.g., A/D mapping and mixing).
- **FR-009**: **Brake Capability**: Actuators MUST support a `Brake` state that overrides current torque/speed to halt rotation.
- **FR-010**: **Testability Mandate**: Every Level 2, 3, and 4 component MUST be implementable in a mockable way, allowing for isolated unit testing of logic without the full physics engine.

### Key Entities & Terminology
For a complete definition of all entities (Avatar, Vessel, Controller, OBC, etc.) and architectural terminology, refer to the authoritative **[Engineering Ontology](file:///home/rod/Documents/lunco/lunco-sim-bevy/specs/ontology.md)**.
