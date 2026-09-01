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
| **4** | **Controller** | **Translation**: Translates `UserIntent` into specific typed commands (e.g., `SetPorts`) or `Actions` for a target entity. | `UserIntent` -> `Avatar` -> `Typed Command` |
| **3** | **FSW / Subsystem**| **The Brain**: Decentralized observers that execute commands and emit ACK/NACK responses. | `Typed Command` -> `Subsystem Observer` -> `ACK` |
| **2** | **Logic / Device** | **Hardware Logic**: The individual components responding to state changes. | `Subsystem` -> `Component Field` |
| **1** | **Plant / Physics**| **Mechanical Truth**: The `f64` spatial state and physical physics interaction. | `Component Field` -> `DVec3` / `Physics Impulse` |

---

## 2. Vision Components: ViewPoint vs. CameraDevice

> ⚠️ **Status note.** The 5-layer control model in §1 is real and implemented.
> The clean `ViewPoint` / `CameraDevice` component split below is **aspirational
> ontology — not yet in code.** There is no `ViewPoint` or `CameraDevice` type,
> and there is no `lunco-camera` crate / `LunCoCameraPlugin`. Today the camera
> lives in **`lunco-avatar`** (`LunCoAvatarPlugin`) as concrete camera-rig
> components — `SpringArmCamera`, `OrbitCamera`, `FreeFlightCamera`,
> `SurfaceCamera` — driving Bevy `Camera3d` while the persistent
> `OriginAnchor` owns `big_space::FloatingOrigin`
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
- **Purpose**: Drives a Bevy `Camera3d`; the persistent `OriginAnchor` tracks
  the selected camera's f64 cell while camera rigs (spring-arm, orbit,
  free-flight, surface-relative) handle motion between simulation truth and
  the rendered frame.

---

## 3. The Lifecycle: Command -> Camera Mode

### **Typed Command** (The Pulse)
A discrete instruction event.
- **Self-Describing**: Commands are typed structs (derived with `#[Command]`) and carry their own parameters and documentation, discovered via reflection.
- **Feedback**: Every command execution triggers an acknowledgment result (`Result<Ack, String>`) for verification.

Camera commands change one exclusive behavior component on the local avatar.
`FocusTarget`, `PossessVessel`, `FollowTarget`, `TeleportToSurface`, and
`ReturnFromOrbit` are explicit mode transactions; the active behavior owns the
complete BigSpace `(CellCoord, Transform)` pose. The task-tree runtime owns
long-running authored missions separately and does not use a camera-specific
progress component.

---

## 4. Input Preemption
To provide a natural "human" feel, manual user input always takes precedence
over automated camera ownership:
- `Look`, movement, and zoom are consumed only by the currently active camera
  mode; an authored cinematic lock explicitly owns the pose until released.
- A mode transaction removes the competing behavior components atomically, so
  manual input and an automated camera solver cannot write the same pose.

---

## 5. Headless Compatibility
The simulation core (`lunco-celestial`, `lunco-core`) has NO dependency on the camera rigs or Bevy's rendering systems.
- **Bots** can "see" and "look at" objects through the same `Action` / intent system (against the planned `ViewPoint`; today against the avatar/camera transform).
- **Server** instances run the full spatial logic without a GPU.
- **Clients** add **`LunCoAvatarPlugin`** (`lunco-avatar`) to provide the camera rigs and visual bridge; post-processing / lighting come from `lunco-render`.

---

## 6. Reference Implementation: Scene Viewport & Active Camera

The camera-*selection* and viewport machinery below is **implemented** (distinct
from the aspirational `ViewPoint`/`CameraDevice` ontology in §2). It reuses Bevy
and USD standards rather than inventing bespoke types, and follows a strict
**single-authority** discipline: exactly one system writes window-camera state.

### 6.1 Cameras are standard USD + Bevy

- A scene camera is a standard USD **`def Camera`** (`UsdGeomCamera`) prim.
  `lunco-usd-bevy` (`camera.rs`) translates each to render-free camera intent;
  `lunco-render-bevy` then creates the **inactive** Bevy `Camera3d` and its
  complete render graph atomically: `focalLength` / `verticalAperture` → vertical
  FOV, `clippingRange` → near/far, `projection` token → perspective/orthographic.
  The optional
  `lunco:cameraLookAt` (double3, parent-local) aims the camera at a point.
- "Which camera renders" is Bevy's own **`Camera::is_active`** — there is no
  bespoke "active camera" marker.
- A *switchable* camera is a `def Camera` with `LunCoCameraAPI` and
  `lunco:cameraRole = "viewport"`, plus the local avatar camera. Instrument
  cameras use `lunco:cameraRole = "sensor"` and are never main-window
  candidates. RTT (`Image`-target) cameras and the egui `Camera2d` are excluded.

### 6.2 The Viewport is the single source of truth

`lunco_core::SceneViewport` models the main window's 3D viewport (à la an
Omniverse Viewport, which owns an active `camera`):

| Field | Meaning | Written by |
| :--- | :--- | :--- |
| `active_camera: Option<Entity>` | resolved camera entity that renders; `None` is an intentional no-camera state | `reconcile_scene_viewport` |
| `visible: bool` | whether 3D renders at all | the workbench (layout perspective) |
| `rect: Option<(UVec2, UVec2)>` | window sub-rect, or full-window | the workbench |

An authored selection is retained as `(stage, USD prim path)` and re-resolved
after re-projection; the ECS entity is only the current realization. A command
or camera track changes the selection intent, while exactly **one** system writes
`SceneViewport::active_camera`, window-camera `is_active`, and `viewport`:
`lunco-usd-bevy`'s **`reconcile_scene_viewport`**. It actuates the viewport
(`is_active = bound-camera && visible`) and relocates the big_space
the persistent `OriginAnchor` to the active camera's f64 `WorldGrid` cell. A
missing, stale, or projectionless explicit request produces no active camera
and a visible status diagnostic; it never selects the first authored camera as
a repair or silently substitutes a different authored camera.

### 6.3 Switching

The viewport has explicit presentation ownership:

- **Director:** `SetActiveCamera { name }` (API + Rhai `set_camera("Name")`) and
  `CameraTrack` cuts select authored cameras. Director requests are held while
  the operator owns the viewport.
- **Operator:** `SetUserCamera { name }`, `ObserveAvatar`, or `KeyC` explicitly
  selects a camera and takes ownership. `ObserveAvatar` is an operator intent;
  the presence of an avatar never emits it implicitly. `ResumeCameraDirector`
  returns control to the authored track.

Names match a full USD prim path or its leaf. A windowed scene must author its
initial presentation through `CameraTrack` (including a single key for a static
initial view). If no authored window camera or track resolves, the viewport
stays inactive and the owning diagnostic is shown. The engine does not invent a
camera, select the first camera, or turn avatar presence into presentation
policy.

### 6.4 Rover-mounted cameras

An onboard camera explicitly applies `LunCoCameraAPI` with
`lunco:cameraPose = "mounted"`. `resolve_camera_mounts` realises that declared
contract as a **grid-direct follower** (`MountedCamera { mount, offset }`), and
`follow_mounted_cameras` writes `mount · offset` in double precision. This lets
the persistent origin tracker follow the camera without changing its
hierarchy. A nested camera with `cameraPose =
"authored"` remains in ordinary USD composition; hierarchy never infers a mount.

### 6.5 Camera rigs still live in `lunco-avatar`

The *behavior* of the free/possession cameras — `SpringArmCamera`,
`OrbitCamera`, `FreeFlightCamera`, `SurfaceCamera` — remains in `lunco-avatar`
(§2). The viewport reconciler decides *which* camera is shown; the rigs decide
*how* a given camera moves. They compose: possession changes the avatar camera's
rig without changing which camera the viewport shows.

### 6.6 Avatar identity and ownership

`Avatar` is an embodiment component, not a user, session, or control authority.
It identifies an entity that can carry a presentation rig and a controller link.
The local/remote distinction is an ownership qualifier on that same embodiment:

- `LocalAvatar` is the authoritative marker for the one embodiment that may
  consume this process's input and drive its local interactive camera.
- `RemoteAvatar` identifies another session's replicated embodiment. It may be
  rendered, but it is not eligible for local input or camera commands.
- `TheLocalAvatar` is a derived entity index maintained by the `LocalAvatar`
  lifecycle hooks. It is a read-only lookup cache, not a second ownership
  contract and not a user object; callers never write it.

Session/control authority is separate from presentation. A headless API,
autopilot, or mission script can control a vessel without creating an avatar.
Commands that need a local camera accept an explicit complete `LocalAvatar`, or
the derived `TheLocalAvatar` selection when the avatar is omitted. An invalid
explicit entity or a missing local camera is rejected at the avatar-camera
boundary and remains visible through runtime diagnostics; no entity-order
selection is permitted.

The local avatar also carries an `InputPorts` surface for free-flight movement
(`forward`/`side`/`up`) and its normalized `speed_boost` modifier,
but the `Avatar` domain marker makes that endpoint ineligible for vessel
possession. Plain-click resolution continues past an avatar endpoint to the
nearest non-avatar input surface, and direct `PossessVessel` requests apply the
same rule before either camera binding or authority claim. Render markers such
as `SceneCamera` do not participate in this control decision.

Free-flight and surface movement are kinematic camera motion and use the shared
BigSpace/Avian collision contract described in
[`45-big-space-correct-usage.md`](45-big-space-correct-usage.md#physics-boundary).
The avatar is not given a second authored USD body or collider; its capsule
query consumes the standard stage colliders. Traversal is disabled unless the
active Twin explicitly sets `avatar.allow_through_soil = true` through the
existing generic Twin-settings command.

### 6.7 Vehicle control frame

The controller has one shared input path: persisted `input_bindings` resolve raw
devices to semantic `UserIntent`s, then the vessel's authored `ControlBinding`
resolves those intents to named command ports. A vehicle profile owns the second
mapping; Rust does not add key-specific or vehicle-kind exceptions.

The free-flight avatar uses the same contract: the configured `SpeedBoost`
intent maps to its normalized `speed_boost` command port and is emitted in the
same `SetPorts` batch as movement. The avatar actuator consumes that command
frame, so modifier transitions cannot bypass controller ordering or diverge
from Q/E movement.

`LanderControls` is body-relative. Its forward/back, left/right, and yaw
intents write the authored lander's body `pitch`, `roll`, and `yaw` ports; thrust
and release write their corresponding vehicle ports. It selects
`CameraFollow::Orbit`, which keeps a stable external/gravity frame while a
6-DOF lander rotates inside it. Camera yaw, pitch, or roll therefore never
changes the signs or axes of the physical command. An authored `Chase` camera
may follow the full vehicle attitude for presentation, but it still does not
alter the body-frame control contract.

The bundled W/S/A/D/Q/E and other key labels are not the control contract. They
are the current projection of `InputBindingsSettings`; UI help and tutorials
must resolve labels from that resource so remapping updates presentation while
the semantic profile and physical actuator ownership remain unchanged.

---

## Technical Reference

- [**Application Guide**](../README.md#application-guide) — How to run the various binaries and tools.
- [**API Documentation**](12-api.md) — Detailed list of API endpoints, typed commands, and queries.
- [**Crates Index**](../crates-index.md) — Navigation guide for the workspace structure.
