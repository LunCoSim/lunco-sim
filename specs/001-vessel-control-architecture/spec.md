# Feature Specification: 001-unified-control-interaction

**Feature Branch**: `001-unified-control-interaction`
**Created**: 2026-03-29
**Status**: Active
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
| **5** | **Action** | **`ActionState<SpaceSystemAction>`** | Human intent (e.g., `MoveForward`). Managed by Leafwing. |
| **4** | **Controller**| **`SpaceSystemController` System** | Reads `ActionState`, emits **`Events<Command>`**. Mapping: `Action::W` -> `CMD_DRIVE(1.0)`. |
| **3** | **FSW** | **`FlightSoftware` Plugin** | Reads `Events<Command>`, translates to **`OBC.Port`** values via a **Hardware Map**. |
| **2** | **OBC** | **`OBC_Emulator` Entity** | Collection of **`Port`** child entities (Digital/Analog Signals). |
| **1** | **Plant** | **`Actuator / Sensor`** | Physics entities with **`Port`** components (Physical Units). |

---

## Implementation Patterns: Direct-Reference Port & Wire Architecture

To ensure high-performance (1000Hz+) and robustness, the engine uses a direct entity-reference system for hardware signal flow.

### 1. The Port Component
Lightweight components attached to both digital and physical interfaces.
```rust
struct DigitalPort {
    pub raw_value: i16, // -32768 to 32767 mapping to full physical bounds
}

struct PhysicalPort {
    pub value: f32, // SI Units (Nm, N, rad/s)
}
```

### 2. The Wire Component (Signal Link)
The `Wire` component scales signals between digital and physical domains.
```rust
struct Wire {
    pub source: Entity, 
    pub target: Entity, 
    pub scale: f32, // Signal Gain (e.g., Max_Torque / 128.0)
}
```

### 3. Symmetrical Signal Propagation
- **Forward Path (FSW -> OBC -> Plant)**: FSW writes `-255` for "Full Reverse" into the OBC. `Wire` scales this to `-MaxTorque`.
- **Reverse Path (Plant -> OBC -> FSW)**: Sensor writes `9.81` into its physical port. `Wire` scales this to its 16-bit digital representation (`i16`) for FSW retrieval.

### 4. FSW Hardware Map (Level 3 Logic)
The FSW populates a map of OBC Port Entity IDs during instantiation.
- **Example**: `RoverFSW.drive_left = Entity(OBC_Port_5)`.
- **Runtime Drive**: `ports.get_mut(fsw.drive_left).raw_value = -32768; // Full reverse command`

---

## Technical Validation Scenarios


### User Story 0 - Stage 1 Baseline (MVP) (Priority: P0)
As a developer, I want to validate the 5-layer architecture with a concrete "Stage 1" rover experience that works in the browser.

**Environment Setup:**
- **Terrain**: A static 1x1km flat plane.
- **Physics Feature**: A single physical **ramp** sufficient for launching the rover into the air.
- **Lighting**: A single directional light source representing the Sun.

**Space System Configuration (The Baseline Rover):**
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

**Baseline Rover Variants (The 4 Quadrants):**
To ensure architectural robustness across different simulation fidelity needs, the engine supports 4 baseline rover configurations:

| Variant | Physics Method | Steering Method | Use Case |
|---|---|---|---|
| **R-S** | Raycast | Skid (Differential) | High-performance, fast testing, web-stable. |
| **R-A** | Raycast | Ackermann (Turning) | Precision racing/navigation, stable at speed. |
| **J-S** | Joint-based | Skid (Differential) | High-fidelity suspension/bumps, no steering complexity. |
| **J-A** | Joint-based | Ackermann (Turning) | Maximum fidelity, 1:1 engineering twin. |

**Technical Acceptance Criteria:**
- **Visual Accuracy**: Wheels MUST have a basic shader/texture allowing visual verification of rotation.
- **Speed Matching**: Wheel rotation speed MUST physically match the rover's velocity (including air-borne inertia).
- **Braking Realism**: Braking MUST apply a physical `BrakeForce` that slows the vessel according to mass/friction, rather than a hard teleport to zero velocity.
- **Architecture Validation**: MUST pass all **[General Testing Framework (000-TEST)](file:///home/rod/Documents/lunco/lunco-bevy/specs/000-testing-framework/spec.md)** compliance rules.
- **WASM Compliance**: The entire scenario MUST be runnable in a modern web browser.

---

## Testing & Validation Mandate

To ensure the 5-layer architecture is computationally verifiable, all development MUST adhere to the **[General Testing Framework (000-TEST)](file:///home/rod/Documents/lunco/lunco-bevy/specs/000-testing-framework/spec.md)**. This includes:

1. **Architecture Compliance**: No layer-skipping or prohibited dependency flows.
2. **Component-Level Logic Tests**: Mandatory unit tests for every Layer 2, 3, and 4 component (FR-010).
3. **Headless Verifier Validation**: Automated, GUI-less physical state verification (Movement, Braking, Stability).

---

## Implementation Patterns

### 2. Coordinate Interoperability (Aerospace Standards)
To ensure engine parity and reduce orientation errors, all development and simulation logic adheres to a project-wide coordinate standard.

- **Bevy Standard**: **$-Z$ is Forward**, **$+Y$ is Up**.
- **FSW Mapping**: The FSW-to-OBC driver handles the implicit rotation mapping when communicating with external ENU (+Y Forward) or NED (+X Forward) tools.

---

## 3. High-Fidelity Plant Baselines (Reference)

To ensure consistency across the 4 Baseline Rover variants, the following parameters are suggested as the **"Standard Heavy Plant"** baseline for testing:

- **Mass**: 1,000 kg (Chassis) + 100 kg/wheel.
- **Suspension Stiffness**: 80,000 N/m.
- **Suspension Damping**: 5,000 Ns/m.
- **Linear Damping**: 0.5.
- **Angular Damping**: 1.0.

### Visual Orientation Mandate
To prevent development orientation errors, all baseline vessels MUST possess **Distinct Front/Rear Visual Markers** (e.g. Red wheels at Front, Blue wheels at Rear) in development and verification builds.

---

## 4. Interaction Logic & Implementation Patterns

### 1. Stateful FSW Control
In accordance with the 5-layer model, human interaction (Momentary Keypress) is decoupled from hardware execution (Persistent State).
- **BRAKE_ROVER**: This command represents a **Stateful Toggle** managed by the **Level 3 (FSW)**. The FSW is responsible for maintaining the "Brake Active" state until a release command is received.
- **Center-on-Release**: The **Level 4 (Controller)** is responsible for emitting neutral control signals (e.g., zero drive/steering) when no human input is detected, facilitating passive self-centering.

### 2. Camera-Relative Rendering (Origin Shifting)
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

### User Story 1 - Avatar Possession & Orbit Interaction (Priority: P1)
As a user, I want my **Avatar** to be the primary interactive element in the world:
- By default, the Avatar operates as a free-camera, moving freely through the 3D space using the `WASDQE` keys.
- By holding the **Right Mouse Button**, I can drag to rotate the Avatar's view direction.
- **Possession & Orbit**: When I **Click on the rover**, the Avatar "possesses" the vessel. The camera switches to **Orbit Mode**, centering the rover.
- **Controllers**: While possessed, `WASD` commands move the rover; **Right Click + Drag** orbits the camera around the vessel.
- **Release**: By pressing **Backspace**, the Avatar terminates possession, releasing the rover and returning to free-camera mode.

### User Story 2 - CLI Command Overhaul (Priority: P1)
As a mission operator, I want to bypass the Avatar and Controller by sending a raw `CMD_REBOOT` directly to the FSW via the command-line interface (CLI).

### User Story 3 - Automated Success Verifiers (Priority: P1)
As a QA lead, I want to run 1,000 headless simulations of a rover landing to gather statistical success data without GPU overhead or windowing requirements.

### User Story 4 - SysML Attribute Telemetry Tweaking (Priority: P1)
As a mission operator, I want to view and modify dynamic component configurations (Attributes) in real-time via external CLI or MCP tools without executing procedural commands. By using an `AttributeRegistry` that parses SysML strings (e.g. `set("rover1.motor_l.max_torque", 95.5)`), I can seamlessly tweak hardware calibrations and visualize the telemetry instantly affecting physical outcomes.

---

## Requirements

### Functional Requirements
- **FR-001**: **Unified Action Bus**: Every Avatar executes an Action Bus (using `leafwing-input-manager` style mapping).
- **FR-002**: **Command Handover**: The Flight Software MUST expose a unified interface for receiving **Commands** from the Controller, CLI, or internal autonomous sequences.
- **FR-003**: **OBC Hardware Emulation**: The OBC MUST maintain a persistent state of its I/O registers, allowing telemetry probes to read real-time "Port Levels".
- **FR-005**: **f64 Physical Fidelity**: All physics calculations MUST be performed in **f64 (double precision)** using Avian3D.
- **FR-006**: **Actuator Metadata Awareness**: Every Level 1 component MUST expose `MaxTorque`, `MinTravel`, and `Addressing` for control validation.
- **FR-007**: **Headless Mode Compliance**: The core simulation MUST be capable of functioning without `RenderPlugin` or `WindowPlugin`.
- **FR-008**: **Actuator Multi-Mapping**: FSW MUST support simultaneous control of multiple actuator types (Drive vs. Steering) from a single user intent (e.g., A/D mapping and mixing).
- **FR-009**: **Brake Capability**: Actuators MUST support a `Brake` state that overrides current torque/speed to halt rotation.
- **FR-010**: **Testability Mandate**: Every Level 2, 3, and 4 component MUST be implementable in a mockable way, allowing for isolated unit testing of logic without the full physics engine.

### Key Entities & Terminology
For a complete definition of all entities (Avatar, Space System, Controller, OBC, etc.) and architectural terminology, refer to the authoritative **[Engineering Ontology](file:///home/rod/Documents/lunco/lunco-bevy/specs/ontology.md)**.
