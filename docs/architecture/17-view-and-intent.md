# 17 — LunCoSim View & Intent Architecture

> Status: Active · Audience: contributors on input, camera, and control systems
>
> **TL;DR.** A 5-layer control model that decouples raw input from physical
> execution (UserIntent → … → actuation), keeping the camera and intent
> systems modular and headless-safe.

**Status: design / proposal (not yet implemented).** The `ViewPoint` / `CameraDevice` components and the `lunco-camera` crate described below are the intended target architecture; they do not exist in the codebase yet. Camera behaviors today live in `lunco-avatar`.

This document provides a technical guide to the modular, action-oriented, and headless-safe camera and intent systems in LunCoSim.

---

## 1. The 5-Layer Control Model
LunCoSim decouples human interaction from physical execution using five distinct layers:

| Layer | Name | Responsibility | Logical Flow |
| :--- | :--- | :--- | :--- |
| **5** | **UserIntent** | **Semantic Mapping**: Raw inputs (WASD, Mouse) -> Abstract Goals (`MoveForward`, `LookAtTarget`). | Keyboard -> `Leafwing` -> `UserIntent` |
| **4** | **Controller** | **Translation**: Translates `UserIntent` into specific `CommandMessages` or `Actions` for a target entity. | `UserIntent` -> `Avatar` -> `CommandMessage` |
| **3** | **FSW / Subsystem**| **The Brain**: Decentralized observers that execute commands and emit `CommandResponse` ACKs. | `CommandMessage` -> `Subsystem Observer` -> `ACK` |
| **2** | **Logic / Device** | **Hardware Logic**: The individual components (e.g., `CameraDevice`, `ViewPoint`, `Motor`) responding to state changes. | `Subsystem` -> `Component Field` |
| **1** | **Plant / Physics**| **Mechanical Truth**: The `f64` spatial state and physical physics interaction. | `Component Field` -> `DVec3` / `Physics Impulse` |

---

## 2. Vision Components: ViewPoint vs. CameraDevice

> ⚠️ **Status note.** The 5-layer control model in §1 is real and implemented.
> The clean `ViewPoint` / `CameraDevice` component split below is **aspirational
> ontology — not yet in code.** There is no `ViewPoint` or `CameraDevice` type,
> and there is no `lunco-camera` crate / `LunCoCameraPlugin`. Today the camera
> lives in **`lunco-avatar`** (`LunCoAvatarPlugin`) as concrete camera-rig
> components — `SpringArmCamera`, `OrbitCamera`, `FreeFlightCamera`,
> `SurfaceCamera` — driving Bevy `Camera3d` + `big_space::FloatingOrigin`
> directly. Sun / shadow rendering lives in `lunco-render`.

### **ViewPoint (Logical)** — *planned*
The universal logical "eye."
- **Crate**: would live in `lunco-core` (Headless Safe). *Not yet implemented.*
- **Purpose**: Defines where an entity is looking and its FOV. Both bots and players read this component to perform spatial math (e.g., "Is the Earth in the center of my ViewPoint?").
- **Precision**: Uses `f64` for planetary-scale accuracy.

### **CameraDevice (Physical)** — *planned*
Representing a sensing hardware unit.
- **Crate**: would live in `lunco-core` (Hardware Marker). *Not yet implemented.*
- **Purpose**: Attaches a `ViewPoint` to a physical presence. It can optionally have a **Physical Collider** (via `avian`) to prevent terrain clipping.

### **Renderer / Blender (Visual)** — *today: `lunco-avatar`*
The rendering bridge.
- **Crate**: `lunco-avatar` (`LunCoAvatarPlugin`, client-only camera rigs). Sun/shadow in `lunco-render`.
- **Purpose**: Drives a Bevy `Camera3d` and its `FloatingOrigin`. Camera rigs (spring-arm, orbit, free-flight, surface-relative) handle smoothing between simulation truth and the rendered frame.

---

## 3. The Lifecycle: Command -> Action

### **CommandMessage** (The Pulse)
A discrete instruction packet.
- **Dumb Transport**: The envelope is lean (`id`, `target`, `source`, `name`, `args`). It does NOT handle spatial context; the receiving FSW handles internal coordinate mapping.
- **Performance**: Arguments use **`SmallVec<[f64; 4]>`** to stay on the stack for high-frequency ticks (WASD).
- **Feedback**: Every command triggers a **`CommandResponse`** (ACK/NACK) pulse for Mission Control confirmation.

### **ActiveAction** (The Process)
A long-running, stateful task with a lifecycle:
1. **Started**: A `CommandMessage` triggers an `ActiveAction`.
2. **Running**: A dedicated system updates `progress` and modifies the target component.
3. **Preemption**: Manual USER input (via `UserIntent`) immediately cancels active actions to ensure tactile control responsiveness.
4. **Result**: Upon completion, a final `CommandResponse` is emitted.

---

## 4. Input Preemption
To provide a natural "human" feel, manual user input always takes precedence over automated actions:
- If a `Look` or `Move` intent is detected, the `Avatar` automatically **Cancels** any active `CameraTransition` actions.
- Controls are "handed over" to the USER immediately, preventing fights between manual steering and automated transitions.

---

## 5. Headless Compatibility
The simulation core (`lunco-celestial`, `lunco-core`) has NO dependency on the camera rigs or Bevy's rendering systems.
- **Bots** can "see" and "look at" objects through the same `Action` / intent system (against the planned `ViewPoint`; today against the avatar/camera transform).
- **Server** instances run the full spatial logic without a GPU.
- **Clients** add **`LunCoAvatarPlugin`** (`lunco-avatar`) to provide the camera rigs and visual bridge; post-processing / lighting come from `lunco-render`.

---

## Technical Reference

- [**Application Guide**](../README.md#application-guide) — How to run the various binaries and tools.
- [**API Documentation**](12-api.md) — Detailed list of API endpoints, typed commands, and queries.
- [**Crates Index**](../crates-index.md) — Navigation guide for the workspace structure.
