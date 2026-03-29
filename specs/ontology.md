# LunCoSim Engineering Ontology

This document serves as the definitive source of truth for the architectural terminology and concepts used in the LunCoSim ecosystem. All specifications and code implementations MUST adhere to these definitions.

### Key Concepts

- **Space System**: The universal container for an independent, controllable entity in the simulation (e.g., Rover, Satellite, Ground Station, Base). Following CCSDS and XTCE standards, a Space System is a recursive hierarchy of Subsystems and structural Links.
- **Verifier**: A persistent, independent monitoring system that validates simulation state against analytical truth. Verifiers are the "Judges" of the digital twin, ensuring that physics and logic remain within verified engineering bounds. Following SysML v2, Verifiers execute **Verification Cases** against mission requirements.
- **Attribute**: A measurable, persistent data field belonging to a Link or Port (e.g., `Mass`, `Voltage`, `MaxTorque`). This term is used for 1:1 alignment with SysML v2 and USD.
- **CommandRegistry**: A self-describing component attached to a **Space System** or **Link** that defines its available **CommandMessages**. Inspired by **XTCE MetaCommands** and **NASA FPrime Commands**, it contains documentation, parameter types, and validation ranges for AI discovery.
- **CommandMessage**: The universal "Instruction" packet sent to an entity. Following **CCSDS Telecommand** standards, it contains a target ID, a command name, and typed arguments.

### Terminology Rationale

To build a high-fidelity digital twin of a lunar city, we use terms that are globally recognized across the disparate fields of aerospace, robotics, and systems engineering.

- **Space System (vs. Vessel/Vehicle)**: Following **XTCE (XML Telemetric and Command Exchange)** and **CCSDS** standards, "Space System" is a recursive container. This allows us to treat a single 3D-printed brick, a rover, a ground station, and the entire lunar city as the same class of object. It ensures that our simulation is "Mission Control Ready" out-of-the-box.
- **Verifier (vs. Test/Assertion)**: In computer science, an **Verifier** is an independent mechanism for determining whether a system has passed a test. In LunCoSim, Verifiers represent the "Ground Truth" (Analytical Physics) that monitors the simulation (Engine Physics) to detect drift, ensuring mathematical integrity.
- **Attribute (vs. Property)**: We use "Attribute" for 1:1 alignment with **SysML v2** and **Pixar's USD**. Prims have attributes; parts have attributes. This avoids the programming ambiguity of "Properties" (getter/setter functions).
- **Port (vs. Pin/Connector)**: "Port" is the universal term used by **SysML v2**, **NASA FPrime**, and **ROS**. It defines a semantic interface point. While Modelica uses "Connector," we use "Connection" for the link (the Wire) to maintain consistency with SysML v2 and FPrime.
- **Link & Joint (vs. Part/Bone)**: Adopting the **URDF** and **USD Physics** terminology ensures that any roboticist or CAD engineer can immediately map their kinematic chains into our coordinate frame tree.
- **CommandRegistry**: The "Brain-Interface" of a Space System. Instead of a fixed, hardcoded list of actions, each entity describes its own capabilities. This is the "secret sauce" for AI-native simulation: it allows an LLM or an MCP agent to look at a new, unknown rover and immediately "know" how to drive it by reading its live documentation. It creates a single, unified channel where human inputs (WASD), automated scripts, and AI agents all speak the same language to the simulation.

---

## 1. Architectural Principles

### Plugin First (Modular Core)
Every feature, from high-level flight software to low-level physics propagators, MUST be implemented as a modular **Bevy Plugin**. The core engine is a skeletal orchestrator; all "meat" is pluggable, allowing for a lean, headless-first architecture.

### Hot-Swappable (Runtime Logic Injection)
The 5-layer architecture MUST support runtime replacement of any Level 2, 3, or 4 component.
- **OBC Swap**: Replacing a basic wheel driver with an advanced differential steering driver.
- **FSW Swap**: Switching from internal Lua logic to an external Fprime/ROS bridge.
- **Controller Swap**: Changing from a Tank-drive mapping to a Character-movement mapping for the same vessel.

### Simulator as Mechanism, Avatar as Agency
The simulation core (Level 1 & 2) is a "dumb" physical reactor. Intelligence and interaction (Level 3, 4, & 5) are delegated to pluggable agents. The **Avatar** is the only entry point for human interaction, ensuring 100% decoupling between the physical plant and the pilot.

---

## 2. The 5-Layer Control Model

LunCoSim uses a layered approach to separate human intent from computer logic and physical execution.

| Layer | Name | Responsibility | Input | Output |
| :--- | :--- | :--- | :--- | :--- |
| **5** | **Action** | **Human Intent**: Platform-agnostic representation of what the user wants to do (e.g., `Move Forward`). | User Input (WASD, Joystick) | Command Map |
| **4** | **Controller**| **Pilot Mapping (Boring)**: A thin translator that maps generic Actions into vessel-specific Commands. It carries no steering or control logic. | Actions | Commands |
| **3** | **FSW (Flight Software)** | **The Brain**: Stateful logic that executes commands and manages autonomous behavior. | Commands (Controller, CLI, MCP) | I/O Requests (Pins) |
| **2** | **OBC (Onboard Computer)** | **Hardware Emulator**: Stateful digital twin of an SBC/MCU (Pins, Registers, Power). | I/O Requests | Electrical Signals |
| **1** | **Plant** | **The Mechanism**: Physical actuators, sensors, and rigidbodies (Physics). | Electrical Signals | Force/Torque/State |

---

## 3. Core Entities

### Avatar
The user's physical representation in the simulation. It provides **Agency** (Camera management, Mouse/Keyboard capture). An Avatar interacts with the world by **Possessing** a Space System and attaching a **Space SystemController**.

### Space System
A high-level container entity (Rover, Satellite, Space Station). A Space System is composed of a **Physical Plant** and an **OBC Emulator**.

### Controller
The "Pilot's Translator." It is a thin, logically "boring" bridge between the Avatar's generic Actions and the Space System's specific Flight Software commands. It does NOT handle wheel mixing, steering, or system logic; those are delegated to the FSW.

### FSW (Flight Software)
The logic running on the OBC. It can be "Integrated" (Basic driving logic) or "Professional" (External HIL/SIL tools). It MUST be hot-swappable.

### OBC (Onboard Computer)
The hardware emulator layer. Maintains the state of **Pins** (GPIO, PWM, Analog) and acts as the electrical interface to the hardware. **Hot-swappable** to allow for different hardware emulations.

### Physical Plant
The "Mechanism" layer. Focused on high-fidelity `f64` space.
- **Actuator**: Translates `OBC Pin` signals into physical work (Torque, Force). MUST expose metadata: `MaxTorque`, `MinTravel`, `Addressing`.
- **Sensor**: The "Input Source." Translates physical states (IMU, Encoder, Spectrometer) into **`OBC Pin` Input States**.
    - **Telemetry Flow**: Level 1 (Sensor) -> Level 2 (OBC Input Pins) -> Level 3 (FSW Logic).
    - **Authority**: The FSW is responsible for reading these raw pins, optionally performing sensor fusion/filtering, and packaging the data for the **Sensor-to-Dashboard Pipeline (011)**.

---

## 4. Communication Concepts

### Action Bus
The stream of high-level intents generated by the Avatar (Level 5).

### Command Bus
The universal instruction stream (Level 5/4/3/2) sent to any **Space System** or **Link**.
- **Dynamic Registry**: Every controllable entity carries a **`CommandRegistry`** (XTCE-compliant) that describes its capabilities.
- **Self-Describing**: Commands include built-in documentation and parameter metadata for **AI/MCP Discovery**.
- **Hierarchy**: High-level commands (e.g., `MOVE_TO`) are "decomposed" by the FSW into low-level commands (e.g., `SET_TORQUE`) sent to child links.

### Port
The universal interface for data and power flow between architectural layers.
- **Physical Port (Level 1)**: Located on Actuators/Sensors. Uses **`f32`** for high-fidelity physical units (Torque, Force, AngularVelocity).
- **Digital Port (Level 2)**: Located on the OBC. Uses **`i16`** (-32768 to 32767) to emulate hardware bit-depth (e.g., 8-bit -128 to 127, or 16-bit) and bidirectional signals.
- **Logic Port (Level 3/Internal)**: Logical endpoints within the FSW hardware map.
- **Compatibility**: Maps 1:1 to SysML `Proxy Ports`, Modelica `Connectors`, and ROS `Hardware Interfaces`.

### Wire (Connector)
The logical and electrical "link" between two **Ports**. A Wire is a Bevy entity that facilitates the high-speed transfer of `PortState` between Level 1 (Plant) and Level 2 (OBC).

### Port Mapping (Wiring)
The configuration defining which OBC Ports are connected to which Physical Plant Ports. 
- **Explicit Mapping**: Hardcoded or `.ron` defined links used for the Stage 1 Baseline Rover.
- **Heuristic Mapping**: Dynamic discovery of ports based on semantic tags (e.g., "drive", "left", "motor") for modular robot building.

---

## 5. Precision Architecture (f64/f32 Split)

### The Problem
Bevy's `Transform` uses `f32`. Planetary-scale simulation requires `f64`. These are fundamentally separated.

### The Solution: Dual-Component with Floating Origin

Every entity has BOTH a high-precision truth position and a render-ready Bevy Transform:

| Layer | Precision | Used By |
| :--- | :--- | :--- |
| **Simulation Truth** | `f64` (`HighPrecisionPosition`) | Physics (avian), OBC, FSW, Modelica, Networking (server) |
| **Floating Origin** | `big_space` crate (128-bit integer grids) | Origin rebasing as camera moves |
| **Render Transform** | `f32` (Bevy `Transform`) | Renderer, UI, Audio. Always near-origin, no precision loss |

**Rules:**
- Physics engine (avian) operates on `f64` components.
- A `SyncTransformSystem` runs after physics, converting f64 → f32 relative to the floating origin.
- `big_space` handles origin rebasing automatically as the camera moves.

### Multiplayer f64/f32 Split
- **Server**: Maintains all entity positions in `f64`. Runs all physics, FSW, Modelica.
- **Server → Client**: Computes f32 positions relative to each client's floating origin and transmits f32 only.
- **Client**: Receives f32, applies directly to Bevy `Transform`. **Zero f64 computation on clients.** Saves CPU and halves bandwidth.

---

## 6. Physics Mode (Entity-Level Propagation)

Each entity's physics can be propagated differently depending on its spatial context. This is foundational to `004-time-and-integrators` and used by `022-fmu-gmat-integration`.

### PhysicsMode Enum

| Mode | Description | Physics Engine | Used When |
| :--- | :--- | :--- | :--- |
| `FullPhysics` | Avian RigidBody active. Thrust, collision, contacts. | avian (f64) | Entity is near player or on a surface |
| `HybridBlend { blend_factor }` | Smooth interpolation between analytical and physics (3-5 sec). | Both, weighted | Transitioning between modes |
| `OnRails` | No RigidBody. Position from orbit equation / GMAT propagator. | Analytical only | Entity is distant, orbiting, or time-warping |

**Transition triggers:** Altitude boundary, proximity to active player, time-warp activation.
**The HybridBlend zone** eliminates KSP-style "jitter pop" by smoothly cross-fading between propagators.

---

## 7. Units & Naming Conventions

### SI Units (Mandatory)
All simulation parameters MUST use SI units:
- **Length**: meters (m)
- **Mass**: kilograms (kg)
- **Time**: seconds (s)
- **Force**: Newtons (N)
- **Pressure**: Pascals (Pa)
- **Temperature**: Kelvin (K)
- **Angles**: radians (rad)
- **Electrical**: Volts (V), Amps (A), Watts (W)

### Entity Naming Convention
SysML `part` names map directly to Bevy entity `Name` components using dot-delimited paths:
- SysML: `rover_v2::chassis::left_front_wheel`
- Bevy Entity Name: `"rover_v2.chassis.left_front_wheel"`

This dot-delimited path is the **canonical identifier** used across:
- Telemetry keys in OpenMCT
- CLI/REPL commands (`set rover_v2.chassis.left_front_wheel.friction 0.8`)
- Scenario Verifier rules (`REQUIRE rover_v2.battery.level > 0.05`)
- Log messages and tracing spans

---

## 8. Simulation Tick Rate

Tick rate is configurable per-session via a `SimulationConfig` resource:

| Mode | Tick Rate | Use Case |
| :--- | :--- | :--- |
| **Game** | 60 Hz | Interactive play, tutorials, assembly |
| **Robotics** | 100–1000 Hz | HIL/SIL, control loop testing |
| **Fast-Forward** | Uncapped (CPU-bound) | Monte Carlo, ML training, orbital propagation |
| **Lockstep** | External clock | Fprime/ROS sync |

---

## 9. Standard Industry Mapping

To ensure interoperability with aerospace and robotics ecosystems, LunCoSim adheres to a 1:1 conceptual mapping with industry-standard modeling languages and simulation formats.

| LunCoSim Concept | SysML v2 | URDF | USD / Isaac | Modelica | **NASA F'** | **XTCE / CCSDS** | **Physical Hardware** |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| **Link** (f64) | `part` | `<link>` | `Xform` | `model` | **Component** | **Aggregate** | **Structural Link** |
| **Joint** (Constraint) | `connection` | `<joint>` | `PhysicsJoint` | `Joint` | N/A | N/A | **Movable Joint** |
| **Port** (Interface) | **`port`** | `transmission` | `PhysicsPort` | `connector` | **Port** | **Entry** | **Socket / Pinout** |
| **Wire** (Signal) | **`connection`** | ROS Topic | `PhysicsAPI` | `connect()` | **Connection** | **Sequence** | **Wire / Harness** |
| **Command** | `action` | ROS Action | `PhysicsAPI` | `action` | **Command** | **Telecommand** | **Instruction** |
| **Space System** | `part` | `<robot>` | `Articulation` | `model` | **Topology** | **SpaceSystem** | **Vehicle / Station** |
| **Verifier** (Verifier) | `requirement` | N/A | `SceneCheck` | `assert()` | **Test Comp** | **Check** | **Validation Rig** |
| **Attribute** | `attribute` | `<inertial>` | `MassAPI` | `parameter` | **Telemetry** | **Parameter** | **Spec Sheet** |


### Coordinate Frame Tree (CFT) Alignment
- **URDF Compatibility**: LunCoSim's **Joint** origin defines the parent-to-child `f64` offset, mirroring the URDF joint-centric hierarchy.
- **USD/Isaac Sim Compatibility**: Every **Link** is a primary transformable prim, mirroring the prim-centric hierarchy used in Omniverse.
- **SysML v2 Compatibility**: Semantic naming (dot-delimited paths) ensures that SysML `part` hierarchies map 1:1 to Bevy ECS parent-child structures.
