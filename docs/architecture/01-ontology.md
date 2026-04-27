# LunCoSim Engineering Ontology

This document serves as the definitive source of truth for the architectural terminology and concepts used in the LunCoSim ecosystem. All specifications and code implementations MUST adhere to these definitions.

### Key Concepts

- **Space System**: The universal container for an independent, controllable entity in the simulation (e.g., Rover, Satellite, Ground Station, Base). Following CCSDS and XTCE standards, a Space System is a recursive hierarchy of Subsystems and structural Links.
- **Verifier**: A persistent, independent monitoring system that validates simulation state against analytical truth. Verifiers are the "Judges" of the digital twin, ensuring that physics and logic remain within verified engineering bounds. Following SysML v2, Verifiers execute **Verification Cases** against mission requirements.
- **Attribute**: A measurable, persistent property of a physical model or structural link (e.g., `SuspensionStiffness`, `Mass`, `WheelRadius`). In our ECS, these are internal component fields exposed via **Bevy Reflection**. They are used for 1:1 alignment with SysML v2 and provide an "Engineering Backdoor" for real-time simulation calibration (Digital Twin tuning) without affecting FSW logic.
- **CommandMessage**: The universal "Instruction" packet used for transport and communication. Inspired by **XTCE Telecommands (TC)**, it is a discrete, serializable data structure containing a unique `u64` ID, a target `Entity`, a source `Entity`, a command `name`, and high-precision **`f64`** arguments stored in a **`SmallVec<[f64; 4]>`** for stack-optimized performance. It serves as a "Dumb Transport" layer where spatial context is handled locally by the executor. The abstract instruction itself is referred to as a "Command".
- **CommandResponse**: The **Feedback Loop** for the command fabric. Every `CommandMessage` triggers a response event containing an `ACK` (Accepted), `NACK` (Rejected), `InProgress`, or `Completed` status. This matches real-world Mission Control handshaking, ensuring that the USER or an AI Agent has definitive confirmation of execution truth.
- **Parameter (TM)**: A dynamic, observable value representing the "Live State" of a system (e.g., `BatteryVoltage`, `CurrentSpeed`). Following **XTCE and YAMCS** standards, Parameters are sampled telemetry channels that form the continuous data stream monitored by ground stations.
- **Action**: A stateful, long-running execution of a **Command**. While a Command is a discrete event (a "pulse"), an Action has a lifecycle (`Started`, `Running`, `Completed`, `Cancelled`) and can be **Preempted** by manual USER input. Inspired by **ROS Actions**, they are used for tasks like orbital transitions or automated docking.
- **ControlStream**: A continuous, lossy, latest-sample-wins data channel for high-rate inputs that do not fit the discrete `CommandMessage` / long-running `Action` contract — e.g. joystick axes driving a vessel, live parameter scrubs, presence cursors. Modeled after **ROS 2 Topics** (`cmd_vel`-style, best-effort QoS) and NASA F Prime's setpoint + rate-group pattern: producers publish at any rate up to a per-stream cap, consumers see only the most recent sample, no acks, no replay. A Level 4 **Controller** / Level 3 **FSW** system on the Twin reads the latest sample at its own fixed rate and closes the control loop locally — clients publish *what they want*, never actuator outputs. Each stream declares a safe-fallback policy (`hold_last(timeout)`, `decay_to_zero(timeout)`, `fail_safe(default)`) so a network blip or producer pause degrades gracefully — same watchdog pattern as ROS 2 cmd_vel listeners. Distinct from `CommandMessage` (discrete, ordered, reliable, ack'd) and `Action` (long-running, lifecycle, preemptable); together these three form the Twin's complete write surface.
- **ViewPoint**: The logical "Eye" of an entity. It defines a position (`DVec3`), orientation (`Quat`), and field-of-view (`f32`) in the simulation's triple-precision space. It is decoupled from rendering; both humans and headless bots use ViewPoints to interact with the world spatially.
- **CameraDevice**: A physicalised component representing a sensing hardware unit. A CameraDevice carries a **ViewPoint** and may optionally possess a physical **Collider** to prevent terrain clipping and inherit vibrations from its parent vessel.
- **UserIntent**: The semantic mapping of raw inputs (Keyboard, Mouse, Gamepad) into abstract simulation goals (e.g., `MoveForward`, `LookAtTarget`). It serves as Level 5 of the control model, ensuring that the same physical key can trigger different actions depending on the context (e.g., free-fly vs. rover possession).
- **CommandRegistry**: A self-describing component attached to a **Space System** or **Link** that defines its available abstract **Commands** and how they are represented as `CommandMessage`s. Inspired by **XTCE MetaCommands** and **NASA FPrime Command definitions**, it contains documentation, parameter types, and validation ranges for AI discovery.
- **TelemetryEvent**: A discrete, timestamped occurrence in the simulation (e.g., "Airlock Opened", "Engine Cutoff"). Following the **YAMCS** standard, Events provide semantic context to the raw telemetry stream, carrying a severity level (Info, Warning, Critical) and a message.

### Terminology Rationale
...
- **AttributeRegistry**: A centralized, thread-safe Reflection server. While `Attributes` define the individual data properties of components, the `AttributeRegistry` maps semantic external strings (e.g. `sim.rover.motor_l.torque_limit`) directly to live ECS Component memory pointers. This allows UI tools, CLI interfaces, and MCP LLMs to dynamically read or write internal engineering state in real-time without needing compiled generic logic.
- **CommandMessage (vs. Direct Function Call / Abstract Command)**: We use "CommandMessage" to signify a structured, transportable packet of instructions, distinct from a direct function call or a high-level abstract "Command." This adheres to standards like **XTCE/CCSDS Telecommands**, enabling better decoupling, serialization, and AI discoverability via the **CommandRegistry**. It separates the *instruction concept* from its *data representation and transport*.

### Terminology Rationale

To build a high-fidelity digital twin of a lunar city, we use terms that are globally recognized across the disparate fields of aerospace, robotics, and systems engineering.

- **Space System (vs. Vessel/Vehicle)**: Following **XTCE (XML Telemetric and Command Exchange)** and **CCSDS** standards, "Space System" is a recursive container. This allows us to treat a single 3D-printed brick, a rover, a ground station, and the entire lunar city as the same class of object. It ensures that our simulation is "Mission Control Ready" out-of-the-box.
- **Verifier (vs. Test/Assertion)**: In computer science, an **Verifier** is an independent mechanism for determining whether a system has passed a test. In LunCoSim, Verifiers represent the "Ground Truth" (Analytical Physics) that monitors the simulation (Engine Physics) to detect drift, ensuring mathematical integrity.
- **Attribute (vs. Property)**: We use "Attribute" for 1:1 alignment with **SysML v2** and **Pixar's USD**. Prims have attributes; parts have attributes. This avoids the programming ambiguity of "Properties" (getter/setter functions).
- **Port (vs. Pin/Connector)**: "Port" is the universal term used by **SysML v2**, **NASA FPrime**, and **ROS**. It defines a semantic interface point. While Modelica uses "Connector," we use **Port** for the interface point and **Connection** for the link — consistent with SysML v2, FPrime, and FMI/SSP.
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

## 2. Coordinate Frames & Orientation Standards

To ensure consistency across heterogeneous plugins (Physics, Rendering, FSW), LunCoSim adheres to a strict canonical orientation in Bevy 3D space.

### Bevy Standard (Simulation Space)
- **Up Vector**: **$+Y$** (0.0, 1.0, 0.0)
- **Forward Vector**: **$-Z$** (0.0, 0.0, -1.0)
- **Right Vector**: **$+X$** (1.0, 0.0, 0.0)

*Rationale*: This selection ensures 1:1 parity with Bevy's internal defaults for `Transform::looking_at()`, `Camera3dBundle`, and standard GLTF asset orientation.

### Aerospace Mapping (Reference)
When communicating with external aerospace tools, the following implicit conversions apply:
- **ENU (East-North-Up)**: Bevy $+X$ (East), Bevy $-Z$ (North), Bevy $+Y$ (Up).
- **NED (North-East-Down)**: Bevy $-Z$ (North), Bevy $+X$ (East), Bevy $-Y$ (Down).

All "Forward/Reverse" logic in the **Controller (Level 4)** and **FSW (Level 3)** MUST resolve to the $-Z$ vector.

---

## 3. The 5-Layer Control Model

LunCoSim uses a layered approach to separate human intent from computer logic and physical execution.

| Layer | Name | Responsibility | Input | Output |
| :--- | :--- | :--- | :--- | :--- |
| **5** | **Intent** | **Human/AI Intent**: High-level goal (e.g., `MoveForward`). Functionally equivalent to Godot's "Input Actions". | Raw Input (WASD, Mouse) | `IntentState` |
| **4** | **Controller**| **Pilot Mapping**: Translates `IntentState` into specific `CommandMessages`. | `IntentState` | `CommandMessages` |
| **3** | **FSW** | **The Brain**: Stateless/Stateful logic that executes commands. | `CommandMessages` | `Port` Writes |
| **2** | **OBC** | **The Interface**: Holds `DigitalPorts` (i16) and registers. | `Port` Writes | `Connection` Signal |
| **1** | **Plant** | **The Mechanism**: Physical actuators, sensors, and rigidbodies. | `Connection` Signal | Force/Torque/State |


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

### ControlStream
A continuous, best-effort data channel orthogonal to the **Command Bus** and **Action Bus**. Used for high-frequency setpoints and presence data where discrete, ordered, reliable delivery is the wrong contract. Together, `CommandMessage` (discrete) + `Action` (long-running) + `ControlStream` (continuous) are the three sibling write channels of every Twin — directly mirroring the ROS 2 Service / Action / Topic trichotomy.
- **Semantics**: latest-sample-wins, no ack, no replay, per-stream max rate (debounce at the producer), bounded buffer (typically a 1-slot `last_sample` or small ring) at the consumer.
- **Typed channels**: each stream is keyed by `(twin_id, stream_id)` and carries a typed payload — e.g. `JoystickAxes`, `JointTarget`, `SimInputScrub`, `CursorPresence`.
- **Local controller closes the loop**: the Twin's Level 4 **Controller** / Level 3 **FSW** reads the latest sample at its own fixed rate (game/robotics/lockstep tick) and produces the actuator/parameter update. UI and remote clients only publish setpoints; they never drive actuators directly. This is what lets a Mars-rover-style ground console use the same Twin as a local-loop joystick session.
- **Safe-fallback policy per stream**: `hold_last(timeout)`, `decay_to_zero(timeout)`, `fail_safe(default)`. If the stream goes silent (network blip, producer stop, browser tab backgrounded), the on-board controller falls back to its declared safe behaviour without operator action.
- **Authority arbitration is a Command, not a stream concern**: `CommandMessage::AcquireStream { stream_id, role }` grants exclusive write to a stream; multiplayer "only one driver at a time, others read-only" resolves on the discrete Command Bus where ordering and ack matter.
- **Transport**: unreliable/unordered (UDP / WebRTC datachannel) for the network case; in-process channel for local. Distinct from the Command Bus transport (reliable/ordered, TCP/HTTP/gRPC).
- **Read-side dual**: `Parameter (TM)` and `TelemetryEvent` already cover the read direction — ControlStream is the symmetric continuous *write* channel that was previously missing from the ontology.

### Port
The universal interface for data and power flow between architectural layers.
- **Physical Port (Level 1)**: Located on Actuators/Sensors. Uses **`f32`** for high-fidelity physical units (Torque, Force, AngularVelocity).
- **Digital Port (Level 2)**: Located on the OBC. Uses **`i16`** (-32768 to 32767) to emulate hardware bit-depth (e.g., 8-bit -128 to 127, or 16-bit) and bidirectional signals.
- **Logic Port (Level 3/Internal)**: Logical endpoints within the FSW hardware map.
- **Compatibility**: Maps 1:1 to SysML `Proxy Ports`, Modelica `Connectors`, and ROS `Hardware Interfaces`.

### Connection
The logical and electrical link between two **Ports**. A Connection is a Bevy entity (typically [`SimConnection`](../../crates/lunco-cosim/src/connection.rs)) that facilitates the transfer of `PortState` between ports — for example, between Level 1 (Plant) and Level 2 (OBC), or between two `SimComponent`s in a co-simulation graph.

*Historical note:* earlier drafts of this ontology and early code used the term "Wire" for the same concept. The canonical term is **Connection**, matching SysML v2, FMI/SSP, and Modelica's `connect()` statement. "Wire" may still appear in historical docs or legacy code; treat them as synonyms, prefer `Connection` in new work.

### Port Mapping (Wiring)
The configuration defining which OBC Ports are connected to which Physical Plant Ports. 
- **Explicit Mapping**: Hardcoded or `.ron` defined links used for the Stage 1 Baseline Rover.
- **Heuristic Mapping**: Dynamic discovery of ports based on semantic tags (e.g., "drive", "left", "motor") for modular robot building.

---

## 4a. Co-Simulation Concepts (`lunco-cosim`)

The co-simulation layer connects multiple simulation engines (Modelica, FMU, GMAT, Avian) as model instances with named inputs and outputs. See [`22-domain-cosim.md`](22-domain-cosim.md) for details.

### SimComponent
A Bevy component that wraps a simulation model instance (typically Modelica or FMU). Exposes named **inputs** and **outputs** as hashmaps. Status: `Idle | Running | Paused | Error`.

### SimConnection
A Bevy entity that links a source port on one component to a target port on another. Implements the FMI/SSP `<Connection>` pattern: `startElement.startConnector → endElement.endConnector` with an optional `scale` factor.

### SimPort
Metadata for a named interface point — `{ name, direction: In|Out|InOut, type: Force|Kinematic|Electrical|Thermal|Signal }`. Used by UI to list connectable endpoints and by connection validators to enforce type compatibility.

### AvianSim
A Bevy component that represents Avian physics as a co-simulation model. Inputs = forces (`force_x`, `force_y`, `force_z`). Outputs = state (`position_*`, `velocity_*`, `height`). Auto-added to any entity with a `RigidBody`.

---

## 4b. Environment Concepts (`lunco-environment`)

The environment layer computes per-entity physical state (gravity, atmosphere, radiation) from celestial-body providers. See [`23-domain-environment.md`](23-domain-environment.md).

### Provider (`GravityProvider`, `AtmosphereProvider`, ...)
A component on a celestial-body entity that defines **how** an environment quantity varies with position. Example: a `GravityProvider` wraps a `GravityModel` (point-mass, spherical harmonics, etc.) that can compute gravitational acceleration at any world position.

### Local\* component (`LocalGravity`, `LocalAtmosphere`, ...)
A cached, per-entity result of applying a provider at the entity's position. Computed each `FixedUpdate` by the environment systems. Read by Avian force application, cosim input injection, UI displays — anything that needs "what gravity does this entity feel right now."

### GravityBody
A link component on a non-body entity that identifies which celestial-body entity it is gravitationally bound to. Needed for `Gravity::Surface` mode. In Modelica terms: this is the ECS analog of `outer World`.

---

## 4c. Document System Concepts (`lunco-ui`)

The Document System is the canonical data model. Every structured artifact users edit (Modelica model, USD scene, SysML block, mission) is a Document. See [`10-document-system.md`](10-document-system.md).

### Document
The canonical, persistent, serialized representation of a user-editable artifact. Lives in Tier 1 of the three-tier architecture. Examples: `ModelicaDocument` (wraps a `.mo` AST), `UsdDocument` (wraps a Stage), `MissionDocument` (wraps an event graph).

### DocumentOp (Op)
A typed, serializable, reversible mutation of a Document. Examples: `AddComponent`, `RemoveConnection`, `SetParameter`, `MoveComponent`. Every op carries its inverse (for undo). Op streams are the unit of collaboration (future) and replay.

### DocumentView
A panel that observes a Document and renders a projection of it. The same document may have many views — e.g., a `ModelicaDocument` is viewed by DiagramPanel (egui-snarl), CodeEditorPanel (text), ParameterInspectorPanel (form), PlotPanel (time series). Views emit ops; they never mutate the document directly.

### DocumentHost
Runtime plumbing that owns a Document, routes ops from views, records undo history, and broadcasts change notifications to other views.

---

## 4d. Workbench Concepts (`lunco-workbench`)

The UI application scaffold. See [`11-workbench.md`](11-workbench.md).

Three terms, three layers. Different tools overload "workspace" to mean
different things — LunCoSim splits them into three explicit concepts to
avoid the collision:

| Concept | Our term | Where |
|---|---|---|
| Editor shell + dock engine + panel registry | **Workbench** | `lunco-workbench` |
| Editor session: open Twins + documents + recents | **Workspace** | `lunco-workspace` (wrapped as `WorkspaceResource` in `lunco-workbench`) |
| Task-specific UI chrome preset | **Perspective** | `lunco-workbench` (trait) |
| A simulation unit on disk | **Twin** | `lunco-twin` |

### Panel
A dockable UI element in the workbench. A Panel typically implements `DocumentView<D>` for some Document type, or is a non-document tool (Scene Tree, Spawn Palette, Console, Twin Browser).

### Perspective
A named task-specific UI configuration. Each Perspective has its own default panel layout, toolbar set, and optionally a camera/view state. Standard LunCoSim Perspectives: **Build** (edit scenes and subsystems), **Simulate** (minimal chrome, maximize viewport), **Analyze** (Modelica/system model deep dive), **Plan** (mission timeline), **Observe** (presentation/cinema mode). Analogous to Eclipse Perspectives (same word) or Blender "Workspaces" (different word, same idea). *Renamed from the earlier `Workspace` trait when the bigger-scope **Workspace** concept landed — see §4e.*

### Activity
A primary navigation category displayed in a vertical strip at the far left (VS Code activity bar pattern). Examples: Scene, Subsystems, Assets, Console, Search. Selecting an Activity opens its browser in a slide-in panel.

### Viewport
The 3D world view — NOT a panel, NOT a tile. Structurally persistent as the central area of the workbench window. Docks are arranged around the Viewport, never on top of it. This is a first-class architectural primitive, distinct from panels.

### Command Palette
Keyboard-invoked (Ctrl+P / Cmd+P) universal search for actions, entities, parameters, and commands. Integrates with the `CommandRegistry` of each Space System for AI-discoverable actions.

---

## 4e. Session Concepts (`lunco-workspace`)

The **Workspace** is LunCoSim's editor-session type — what's open
*right now in this window*. It's the VS Code–Workspace analog:
multiple Twins from potentially different disk locations, every open
Document, the active Twin / Document / Perspective, recents, and
(future) hot-exit buffers.

Ships in a separate crate so headless CI, API-only servers, and
scripting can hold a Workspace without pulling in bevy or egui.

### Workspace
Root session type. Holds `twins: Vec<Twin>`, `documents:
Vec<DocumentEntry>`, `active_twin`, `active_document`,
`active_perspective`, `recents`.

**Twin is a view, not a container.** Documents always live in the
Workspace. A Twin doesn't own a list; it answers "does this document
belong to me?" by checking whether the doc's storage handle lies under
its folder or is context-pinned to it. This keeps Untitled scratch
docs, loose files, and Twin-owned files on one uniform surface.

### TwinId (`u64`)
Stable identifier the Workspace assigns on registration. `0` is the
"unassigned" sentinel; actual ids start at 1. Used over raw paths so
renaming a folder mid-session doesn't invalidate references.

### DocumentEntry
Workspace-level metadata for one open Document: `{ id, kind, origin,
context_twin, title }`. Does NOT hold the parsed source + ops + undo
stack — those live in domain registries (e.g.
`ModelicaDocumentRegistry`).

### Twin-Document association rule
The **deepest** registered Twin whose folder contains the document's
path wins (sub-Twins outrank their parent — the "nearest `twin.toml`"
rule, matching Cargo). For Untitled docs, the explicit `context_twin`
pin applies instead. Docs matching neither are **loose** — shown under
a "Loose" group in the Twin Browser.

### Recents
Bounded lists (10 Twin folders, 20 loose files), most-recent-first,
dedupe-on-push. Surfaced by the Welcome page.

### Not yet
- `.lunco-workspace` on-disk manifest.
- Hot-exit (serialising unsaved buffers across restarts).
- Named / shared Workspaces.

---

## 4f. Storage Concepts (`lunco-storage`)

I/O abstraction that sits under every crate that reads or writes a
Document. Keeps higher layers (`lunco-doc`, `lunco-twin`,
`lunco-workspace`) free of filesystem assumptions so the same
save/load flow compiles for native, browser, and remote-twin backends.

### Storage (trait)
Synchronous `read`, `write`, `exists`, `is_writable`, `pick_open`,
`pick_save`, `pick_folder`. Picker methods are sync (rfd on native);
the wasm backend will switch to async when it lands.

### StorageHandle
Opaque address into a backend: `File(PathBuf)` or `Memory(String)`
today; `Fsa(token)`, `Idb { db, key }`, `Opfs(String)`, `Http(url)`
are declared behind feature flags so downstream matches stay
exhaustive when those backends arrive.

### FileStorage
Native backend. `std::fs` for I/O, `rfd::FileDialog` for pickers,
in-process `Memory` map for tests.

### Where it fits
`Twin::root_handle()` returns a `StorageHandle`. `Twin::owns(&handle)`
is the Workspace's document-routing predicate. `ModelicaDocument`
save-to-disk (Ctrl+S and Ctrl+Shift+S) writes through
`FileStorage::write`. Future browser + remote backends plug in by
implementing the trait — no consumer-side rewrite.

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

**Transition triggers:** Altitude boundary, proximity to active USER, time-warp activation.
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
| **ControlStream** (continuous setpoint) | `flow port` | ROS 2 **Topic** (`cmd_vel`) | `PhysicsAttribute` | real-time `input` | **Setpoint + Rate Group** | **Parameter (TC)** | **Analog Stream** |
| **Space System** | `part` | `<robot>` | `Articulation` | `model` | **Topology** | **SpaceSystem** | **Vehicle / Station** |
| **Verifier** (Verifier) | `requirement` | N/A | `SceneCheck` | `assert()` | **Test Comp** | **Check** | **Validation Rig** |
| **Attribute** | `attribute` | `<inertial>` | `MassAPI` | `parameter` | **Telemetry** | **Parameter** | **Spec Sheet** |


### Coordinate Frame Tree (CFT) Alignment
- **URDF Compatibility**: LunCoSim's **Joint** origin defines the parent-to-child `f64` offset, mirroring the URDF joint-centric hierarchy.
- **USD/Isaac Sim Compatibility**: Every **Link** is a primary transformable prim, mirroring the prim-centric hierarchy used in Omniverse.
- **SysML v2 Compatibility**: Semantic naming (dot-delimited paths) ensures that SysML `part` hierarchies map 1:1 to Bevy ECS parent-child structures.
