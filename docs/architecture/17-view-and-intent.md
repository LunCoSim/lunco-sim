# 17 — LunCoSim View & Intent Architecture

> Status: Active · Audience: contributors on input, camera, and control systems
>
> **TL;DR.** A 5-layer control model that decouples raw input from physical
> execution (UserIntent → … → actuation), keeping the camera and intent
> systems modular and headless-safe.

**Status: implemented in layers.** The `ViewPoint` / `CameraDevice` names
described in §1–§5 remain an aspirational ontology and are not required
components. The reusable implementation is split across focused owners:
`lunco-usd-bevy-camera` decodes standard USD cameras and camera roles,
`lunco-camera-core` carries reusable render-free rig contracts,
`lunco-camera-runtime` realizes generic interactive camera modes,
`lunco-avatar-core` carries avatar lifecycle/command contracts, `lunco-scene-camera`
exposes script/API camera transactions, `lunco-avatar` is the specialized owner
of raw input translation and possession, and `lunco-avatar-camera` owns
celestial BigSpace orbital placement and vessel spring-arm realization. Avatar
transition state is in
`lunco-avatar-camera-core`.
Generic input response and clip precision policy live in
`lunco-camera-runtime`/`lunco-camera-core`; Rhai selects and tunes presentation
policy through the reflected command surface.
Camera selection and the viewport follow the single-authority design in §6.

This document provides a technical guide to the modular, action-oriented, and headless-safe camera and intent systems in LunCoSim.

---

## 1. The 5-Layer Control Model
LunCoSim decouples human interaction from physical execution using five distinct layers:

| Layer | Name | Responsibility | Logical Flow |
| :--- | :--- | :--- | :--- |
| **5** | **UserIntent** | **Semantic Mapping**: The specialized input owner translates configured devices into abstract goals (`MoveForward`, `Look`, `Zoom`). | Keyboard/gamepad/mouse -> `lunco-input-core` -> `lunco-control-core::UserIntent` |
| **4** | **Controller** | **Translation**: Translates semantic intents into specific typed commands (e.g., `SetPorts`) or actions for a target entity. | `UserIntent` -> `lunco-controller` -> typed command |
| **3** | **FSW / Subsystem**| **The Brain**: Decentralized observers that execute commands and emit ACK/NACK responses. | `Typed Command` -> `Subsystem Observer` -> `ACK` |
| **2** | **Logic / Device** | **Hardware Logic**: The individual components responding to state changes. | `Subsystem` -> `Component Field` |
| **1** | **Plant / Physics**| **Mechanical Truth**: The `f64` spatial state and physical physics interaction. | `Component Field` -> `DVec3` / `Physics Impulse` |

---

## 2. Vision Components: ViewPoint vs. CameraDevice

> **Status note.** The universal `ViewPoint` / `CameraDevice` ontology below is
> still a design vocabulary, not a reason to add a marker to `lunco-core`.
> Today the concrete rig components (`SpringArmCamera`, `OrbitCamera`,
> `FreeFlightCamera`, `SurfaceCamera`) are backend-neutral contracts in
> `lunco-camera-core`; `lunco-camera-runtime` supplies the generic free-flight,
> surface, input-policy, and clip-plane mechanisms, while `lunco-avatar` supplies
> possession, input, and transition logic, `lunco-avatar-camera` supplies
> celestial orbital placement, and `lunco-camera-celestial` supplies surface-frame
> adaptation. Standard
> USD projection, mounted cameras, camera paths, and selection live in
> `lunco-usd-bevy-camera`. `lunco-render-bevy` binds render intent to a
> `Camera3d`, while `lunco-render` remains render-pipeline-free.

### **ViewPoint (Logical)** — *planned*
The universal logical "eye."
- **Crate**: would live in `lunco-core` (Headless Safe). *Not yet implemented.*
- **Purpose**: Defines where an entity is looking and its FOV. Both bots and players read this component to perform spatial math (e.g., "Is the Earth in the center of my ViewPoint?").
- **Precision**: Uses `f64` for planetary-scale accuracy.

### **CameraDevice (Physical)** — *planned*
Representing a sensing hardware unit.
- **Crate**: would live in `lunco-core` (Hardware Marker). *Not yet implemented.*
- **Purpose**: Attaches a `ViewPoint` to a physical presence. It can optionally have a **Physical Collider** (via `avian`) to prevent terrain clipping.

### **Renderer / Blender (Visual)** — *today: `lunco-camera-runtime` + `lunco-avatar` + `lunco-avatar-ui`*
The rendering bridge.
- **Crates**: `lunco-camera-runtime` (generic camera-mode realization),
  `lunco-avatar` (`LunCoAvatarPlugin`, avatar input and transition logic),
  `lunco-avatar-camera` (celestial and vessel camera placement), the optional
  `lunco-avatar-ui` egui adapter, and the focused
  `lunco-avatar-core`/`lunco-avatar-policy` contracts. Sun/shadow in
  `lunco-render`.
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

Avatar camera commands change one exclusive behavior component on the local
avatar. Generic authored-camera selection and camera-path commands do not need
an avatar and are handled by `lunco-usd-bevy-camera`.
`FocusTarget`, `PossessVessel`, `FollowTarget`, `TeleportToSurface`, and
`ReturnFromOrbit` are explicit mode transactions; the active behavior owns the
complete BigSpace `(CellCoord, Transform)` pose. The task-tree runtime owns
long-running authored missions separately and does not use a camera-specific
progress component.

`FocusTarget` keeps the camera's transient, avatar-owned orbital pose history
per stable celestial body id. A target switch records the current user pose and
restores the target body's saved pose when available; otherwise the orbit
writer derives an arrival from the camera's current radial region in the
resolved inertial frame. Twin teardown and avatar demotion clear this history.

---

## 4. Input Preemption
To provide a natural "human" feel, manual user input always takes precedence
over automated camera ownership:
- `Look`, movement, and zoom are consumed only by the currently active camera
  mode; an explicit pose lock owns the pose until the camera is re-authored.
- A mode transaction removes the competing behavior components atomically, so
  manual input and an automated camera solver cannot write the same pose.

---

## 5. Headless Compatibility
The simulation core (`lunco-celestial`, `lunco-core`) exposes scene facts,
spatial poses, and typed semantic/control values. Camera solvers and device
translation are supplied by the specialized avatar runtime, while Bevy's
rendering pipeline is attached by the render adapter.
- **Bots and Modelica** can produce continuous pose/aim/math values through
  authored ports or state, while a camera adapter consumes those values.
- **Server** instances run the full spatial logic without a GPU.
- **Clients** add **`CameraRuntimePlugin`** (`lunco-camera-runtime`) and
  **`LunCoAvatarPlugin`** (`lunco-avatar`) for the local avatar's input,
  possession, and source-specific solvers. Add **`AvatarUiPlugin`**
  (`lunco-avatar-ui`) when egui presentation is needed; post-processing /
  lighting come from `lunco-render`.

---

## 6. Reference Implementation: Scene Viewport & Active Camera

The camera-*selection* and viewport machinery below is **implemented** (distinct
from the aspirational `ViewPoint`/`CameraDevice` ontology in §2). It reuses Bevy
and USD standards rather than inventing bespoke types, and follows a strict
**single-authority** discipline: exactly one system writes window-camera state.

### 6.1 Cameras are standard USD + Bevy

- A scene camera is a standard USD **`def Camera`** (`UsdGeomCamera`) prim.
  `lunco-usd-bevy-camera` (`camera.rs`) translates each to render-free camera intent;
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
- The local avatar is a standard `def Camera` carrying `LunCoCameraAPI` and
  `LunCoAvatarAPI`. `LunCoAvatarAPI` marks only the avatar role; USD simulation
  publishes that role and its spatial identity, while the avatar owner adds the
  generic interactive substrate. Rhai selects camera behavior and parameters
  through typed commands and reflected components.

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
`lunco-usd-bevy-camera`'s **`reconcile_scene_viewport`**. It actuates the viewport
(`is_active = bound-camera && visible`) and relocates the persistent
`OriginAnchor` to the active camera's f64 `WorldGrid` cell. A
missing, stale, or projectionless explicit request produces no active camera
and a visible status diagnostic; it never selects the first authored camera as
a repair or silently substitutes a different authored camera.

The same rule applies during scene handoff: a local avatar receives interactive
behavior only after a standard USD camera has been projected with its
`SceneCamera` intent. Missing camera intent is an explicit no-camera state, not
an invitation for the avatar runtime to create or guess a camera.

An authored camera that fails USD attribute/API validation is a terminal
projection failure: the prim is hidden and carries `UsdSceneProjectionFailed`,
so the scene/UI diagnostic identifies the authored path. It is not reclassified
as an omitted camera, assigned a guessed projection, or replaced by the prior
scene's camera. USD schema defaults are used only for genuinely unauthored
attributes.

### 6.3 Switching

The viewport has explicit presentation ownership:

- **Director:** `SetActiveCamera { name }` (API + Rhai `set_camera("Name")`) and
  `CameraTrack` cuts select authored cameras. Director requests are held while
  the operator owns the viewport.
- **Operator:** `SetUserCamera { name }`, `ObserveAvatar`, or `KeyC` explicitly
  selects a camera and takes ownership. `ObserveAvatar` is an operator intent;
  the presence of an avatar never emits it implicitly. `ResumeCameraDirector`
  returns control to the authored track.

Names match a full USD prim path or its leaf. A windowed scene normally authors
its initial presentation through `CameraTrack` (including a single key for a
static initial view) or exactly one `LocalAvatar` camera. When a window host
opts into standalone presentation, the camera adapter passes authored
USD/ECS counts to the `camera.default_presentation` policy. The shipped Rhai
policy chooses `avatar`, `generated`, or `none`; Rust validates and realizes
only that closed decision. `generated` frames finite projected bounds with one
Twin-scoped camera and directional light. The pair is selected only after
projection settles and is removed when authored presentation takes ownership
or the scene tears down. A missing, faulting, or invalid policy is a visible
diagnostic; `none`, invalid, or boundless scenes remain camera-less. The engine
never selects the first authored camera or turns avatar presence into a hidden
policy decision.

### 6.4 Rover-mounted cameras

An onboard camera explicitly applies `LunCoCameraAPI` with
`lunco:cameraPose = "mounted"`. `resolve_camera_mounts` in
`lunco-usd-bevy-camera` realises that declared
contract as a **grid-direct follower** (`MountedCamera { mount, offset }`), and
`follow_mounted_cameras` writes `mount · offset` in double precision. This lets
the persistent origin tracker follow the camera without changing its
hierarchy. A nested camera with `cameraPose =
"authored"` remains in ordinary USD composition; hierarchy never infers a mount.

### 6.5 Camera rigs and input ownership

The *behavior contracts* of the free/possession cameras — `SpringArmCamera`,
`OrbitCamera`, `FreeFlightCamera`, `SurfaceCamera` — live in
`lunco-camera-core`. Generic mode exclusivity, free-flight orientation, and
surface-frame pose writing, validated input response, and clip precision live in
`lunco-camera-runtime`/`lunco-camera-core`; avatar-owned input and transitions
stay in `lunco-avatar`, while celestial BigSpace orbital placement and vessel
spring-arm realization are in `lunco-avatar-camera`; avatar-only orbit return
state is in `lunco-avatar-camera-core`. Avatar lifecycle and
possession commands remain in
`lunco-avatar-core`. The viewport reconciler decides *which* camera is shown; a rig
decides *how* its pose is solved. They compose: possession changes the avatar
camera's rig without changing which camera the viewport shows.

The celestial camera adapter is `lunco-camera-celestial`. It resolves the
camera's body-fixed ENU frame from its own live BigSpace pose and
`GravityBody` binding, then publishes the backend-neutral `SurfaceCameraFrame`.
It also owns the celestial-body query used for adaptive perspective clip
planes. `lunco-avatar-camera` places avatar orbital cameras in explicit
body-centered inertial grids; the avatar runtime does not write
celestial projection precision or orbital placement.
The generic runtime consumes that contract without importing celestial or
BigSpace types, and avatar interaction does not own this conversion.

`lunco-avatar` is the default raw-input owner. It is the only layer that turns
the configured keyboard/gamepad/mouse surface into `UserIntent` for the local
avatar. `lunco-usd-sim`, USD composition, celestial, physics, and generic
camera projection must not import an input-map or controller crate. An editor
or networking adapter may read device state only when it is itself the
specialized owner of that interaction, and it must publish the same typed
intent/command surface rather than leaking devices downstream.

### 6.6 Scriptable camera composition

The camera architecture is intended to let Rhai compose many camera styles
without growing a Rust state machine for each one:

1. **USD owns identity and authored facts.** A standard `UsdGeomCamera` owns
   projection, photographic values, and transform. `LunCoCameraAPI` owns the
   LunCo-specific camera role, pose authority, and optional look-at; the
   `LunCoAvatarAPI` only marks the local avatar role.
2. **Rhai owns policy.** Scripts choose cameras, follow/focus targets, start or
   scrub camera paths, and decide when a camera transaction begins or ends.
   Those actions use typed commands and authored scene queries, not raw input
   names or direct ECS mutation.
3. **Rust owns reusable fast substrate.** BigSpace cell-safe pose commits,
   collision-aware spring arms, orbit frame conversion, camera projection, and
   selection reconciliation stay in the specialized runtime crates. They are
   reusable mechanisms, not scenario policy.
4. **Modelica may own continuous camera math.** A Modelica participant can
   publish an authored aim/pose trajectory through generic ports; a camera
   adapter can consume it. Modelica does not become a second camera selector or
   input reader.

The current public surface supports authored camera cuts and paths, avatar
`focus`/`follow` transactions, generic session control claims, and
`SetCameraLookAt` addressed by an explicit camera identity. The command commits
the pose in the active physics frame and establishes the generic explicit pose
owner, so a Rhai-authored rig can target any eligible scene camera. Mounted and
path-driven cameras retain their authored pose owners; a direct command reports
that ownership conflict instead of creating a second writer.

### 6.7 Avatar identity and ownership

`lunco-avatar-core::roles` owns the avatar embodiment markers and derived local
lookup. `Avatar` is an embodiment component, not a user, session, or control authority.
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
possession. Plain-click resolution continues past an avatar endpoint and gives
an enclosing authored control root (`ControlBinding` or `MobilityRoot`)
priority over nested component input surfaces, so clicking any part of a vehicle
rebinds the camera and controller to the vehicle. Standalone non-avatar input
surfaces remain direct click targets. Render markers such as `SceneCamera` do
not participate in this control decision.

For a discrete control investigation, `SimulateIntentEdge` returns the command
correlation id and publishes the same id in `intent.edge.value.correlation_id`.
Pass that id with the target to the read-only `CausalTrace` query. The query
joins the semantic edge to the authored control binding, selected
`PortRegistry` owner, USD connection/native-joint admission, and current
`SignalRegistry` measurements; it is diagnostic composition, not another
control or telemetry path.

Control authority has two independent layers. The generic session layer's
`SessionRegistry` answers *which session controls which stable target id*; the
backend-neutral `lunco_cosim_core::ControlLink` answers *which local producer
projects semantic input onto which entity*. Neither layer knows what a vessel,
avatar, or camera is. A producer can therefore be an avatar, an autopilot, a
remote-control adapter, or another specialized controller. `ClaimControl` and
`ReleaseControlClaim` expose the session transition directly for those
headless/authored producers; each accepted transition emits
`ControlAuthorityChanged`, and the co-simulation backend applies safe-stop
handling to every released endpoint.

`PossessVessel` is the avatar-level composition of those primitives: its command
validates the target's writable input surface, asks the session authority to
commit the claim, installs the local `ControlLink`, and optionally performs the
camera transaction. `FocusTarget` and `FollowTarget` are likewise reachable
high-level avatar camera commands; Rhai selects when and which target to request,
while Rust enforces the local-avatar boundary and the BigSpace-safe pose update.
Rhai can also call `set_camera_input(...)` to tune the generic camera response
without a Rust rebuild; Rust retains only the validated hot-path mechanism.
The generic command/value surface remains `SetPorts` for a controller that does
not need an avatar camera. `PossessVessel` uses the same `SessionRegistry`
transaction rather than maintaining a parallel ownership path.

A possession handoff releases all prior claims for the session except the selected
target, hard-stops every released vessel, and then commits the new link. Release
performs the same hard stop before returning the avatar to free flight;
wire-applied commands update authority without binding a remote avatar to the
local camera.

Free-flight and surface movement are kinematic camera motion and use the shared
BigSpace/Avian collision contract described in
[`45-big-space-correct-usage.md`](45-big-space-correct-usage.md#physics-boundary).
The avatar is not given a second authored USD body or collider; its capsule
query consumes the standard stage colliders. Traversal is disabled unless the
active Twin explicitly sets `avatar.allow_through_soil = true` through the
existing generic Twin-settings command.

### 6.7 Vehicle control frame

The input stack has one shared input path: `lunco-input-core` resolves persisted `input_bindings` from raw
devices to semantic `lunco-control-core::UserIntent`s, then the vessel's authored
`lunco-control-core::ControlBinding` resolves those intents to named command
ports. `lunco-control-core` is the reusable contract package for input state,
focus gating, authored bindings, and semantic edges; the controller and avatar
are producers/consumers around that contract. A vehicle profile owns the second
mapping; Rust does not add key-specific or vehicle-kind exceptions.

The free-flight avatar uses the same contract: the configured `SpeedBoost`
intent maps to its normalized `speed_boost` command port and is emitted in the
same `SetPorts` batch as movement. The avatar actuator consumes that command
frame, so modifier transitions cannot bypass controller ordering or diverge
from Q/E movement.

`LanderControls` is body-relative. Its forward/back, left/right, and yaw
intents write the authored lander's body `pitch`, `roll`, and `yaw` ports; thrust
and release write their corresponding vehicle ports. It selects
`lunco_camera_core::CameraFollow::Orbit`, which keeps a stable external/gravity frame while a
6-DOF lander rotates inside it. Camera yaw, pitch, or roll therefore never
changes the signs or axes of the physical command. An authored `Chase` camera
may follow the full vehicle attitude for presentation, but it still does not
alter the body-frame control contract.

The bundled W/S/A/D/Q/E and other key labels are not the control contract. They
are the current projection of `lunco-input-core::InputBindingsSettings`; UI help and tutorials
must resolve labels from that resource so remapping updates presentation while
the semantic profile and physical actuator ownership remain unchanged.

A press in the main scene is also the keyboard-focus handoff: the workbench
surrenders retained egui editor focus before publishing `EguiFocus`, so a
possessed vessel receives the shared input map immediately after the scene click.
Text fields retain keyboard ownership until that explicit scene press.

### 6.8 Atomic semantic intent edges

Held controls and discrete actions use separate contracts. `SimulateIntent` is
the level-triggered API/Rhai command for a target-scoped held intent. For a
single transition, use `SimulateIntentEdge` with `edge` set to `pressed`,
`released`, or `pulse`:

```rhai
intent_pulse(lander, "release");
// equivalent: intent_edge(lander, "release", "pulse");
```

The controller validates the shared `UserIntent` vocabulary and emits one
`SemanticIntentEdge { target, intent, kind }`. It does not select a port or
mutate the Twin. Authored Rhai/Modelica policy consumes the edge and decides
whether it means a latch, release, toggle, or other action. Rhai `on_event`
hooks receive the same edge on the existing telemetry bus as `intent.edge`,
with `value.intent`, `value.edge`, and `value.target_gid`; the event source is
also the target gid. The target remains subject to the normal command authority
policy, so two spawned vehicles cannot receive one another's edge.

### 6.9 Editor keyboard input

The workbench host owns one app-level semantic input surface in addition to the
local avatar's input surface. Both use the same `lunco-input-core::InputBindingsSettings` map and
publish `UserIntent`; editor-only views therefore do not need an avatar merely
to receive `Cancel`. `CancelIntent` reads the app-level surface when present and
still reads the local avatar for simulation camera/possession behavior. Egui
text focus remains the single suppression gate, so Escape/Backspace is ignored
by scene/editor consumers while a text field owns the keyboard.

---

## Technical Reference

- [**Application Guide**](../README.md#application-guide) — How to run the various binaries and tools.
- [**API Documentation**](12-api.md) — Detailed list of API endpoints, typed commands, and queries.
- [**Crates Index**](../crates-index.md) — Navigation guide for the workspace structure.
