# 17 — LunCoSim View & Intent Architecture

> Status: Active · Audience: contributors on input, camera, and control systems
>
> **TL;DR.** A 5-layer control model that decouples raw input from physical
> execution (UserIntent → … → actuation), keeping the camera and intent
> systems modular and headless-safe.

**Status: partly implemented.** The `ViewPoint` / `CameraDevice` components and the `lunco-camera` crate described in §1–§5 remain the aspirational target ontology; they do not exist in the codebase yet. However, camera **selection** and the **viewport** are now real and follow a single-authority design — see **§6 (Implemented: Scene Viewport & Active Camera)**. Camera *rig behaviors* (spring-arm, orbit, free-flight, surface) still live in `lunco-avatar`.

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

## 6. Implemented: Scene Viewport & Active Camera (2026-07)

The camera-*selection* and viewport machinery below is **implemented** (distinct
from the aspirational `ViewPoint`/`CameraDevice` ontology in §2). It reuses Bevy
and USD standards rather than inventing bespoke types, and follows a strict
**single-authority** discipline: exactly one system writes window-camera state.

### 6.1 Cameras are standard USD + Bevy

- A scene camera is a standard USD **`def Camera`** (`UsdGeomCamera`) prim.
  `lunco-usd-bevy` (`camera.rs`) translates each to an **inactive** Bevy
  `Camera3d`: `focalLength` / `verticalAperture` → vertical FOV, `clippingRange`
  → near/far, `projection` token → perspective/orthographic. The optional
  `lunco:cameraLookAt` (double3, parent-local) aims the camera at a point.
- "Which camera renders" is Bevy's own **`Camera::is_active`** — there is no
  bespoke "active camera" marker.
- A *switchable* camera is any `Camera3d` with a window `RenderTarget`: every USD
  `def Camera`, plus whatever free/avatar camera a host adds. RTT
  (`Image`-target) cameras and the egui `Camera2d` are excluded.

### 6.2 The Viewport is the single source of truth

`lunco_core::SceneViewport` models the main window's 3D viewport (à la an
Omniverse Viewport, which owns an active `camera`):

| Field | Meaning | Written by |
| :--- | :--- | :--- |
| `active_camera: Option<Entity>` | which camera renders | the camera switch |
| `visible: bool` | whether 3D renders at all | the workbench (layout perspective) |
| `rect: Option<(UVec2, UVec2)>` | window sub-rect, or full-window | the workbench |

Exactly **one** system writes window-camera `is_active` / `viewport`:
`lunco-usd-bevy`'s **`reconcile_scene_viewport`**. It actuates the viewport
(`is_active = bound-camera && visible`), relocates the big_space
`FloatingOrigin` onto the active camera, and self-heals (revalidates the
binding, defaulting to the local-avatar camera) so async spawns and
provisional→avatar takeover never leave zero or many active cameras. **No other
system touches `is_active`** — this is what eliminated the two-writer conflict
that previously double-rendered and jammed camera switching (the workbench's
`apply_workbench_viewport` used to force-activate every window camera).

### 6.3 Switching

Three surfaces, one mechanism — all rebind `SceneViewport.active_camera`:
- **`SetActiveCamera { name }`** command (API + rhai `set_camera("Name")`); the
  name matches the full USD prim path *or* its leaf.
- the **`KeyC`** hotkey cycles window cameras.
- (cutscenes) a rhai script calling `set_camera(...)` on a timeline; a USD-animated
  `def Camera` supplies moving shots.

### 6.4 Rover-mounted cameras

A `def Camera` authored nested under a moving prim (e.g. under a rover Xform) is
realised as a **grid-direct follower** (`camera_mount.rs`), because big_space
requires the `FloatingOrigin` on a grid-direct entity — a literally-nested
camera could never host it. `resolve_camera_mounts` reparents it to the mount's
grid with a `MountedCamera { mount, offset }`; `follow_mounted_cameras` writes
`mount · offset` back into the camera's grid-local pose each frame in double
precision. So an onboard rover camera rides the rover at full precision and can
host the active-view origin — no follow-code in the authored USD.

### 6.5 Camera rigs still live in `lunco-avatar`

The *behavior* of the free/possession cameras — `SpringArmCamera`,
`OrbitCamera`, `FreeFlightCamera`, `SurfaceCamera` — remains in `lunco-avatar`
(§2). The viewport reconciler decides *which* camera is shown; the rigs decide
*how* a given camera moves. They compose: possession changes the avatar camera's
rig without changing which camera the viewport shows.

---

## Technical Reference

- [**Application Guide**](../README.md#application-guide) — How to run the various binaries and tools.
- [**API Documentation**](12-api.md) — Detailed list of API endpoints, typed commands, and queries.
- [**Crates Index**](../crates-index.md) — Navigation guide for the workspace structure.
