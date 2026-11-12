# Live vessel following

`lunco-camera-core` owns the optional `camera.follow(ctx: Map) -> Map` seam.
`lunco-avatar-camera` invokes it for the selected live spring-arm subject during
Application/Presentation/Preparation, using the presentation clock even when
simulation is paused. USD `CameraFollow` and the existing binding transaction
choose the initial rig per subject. A Twin may replace the follow policy through
its ordinary policy manifest or a Twin-scoped operator tool; no vessel type is
compiled into Rust.

Facts include `subject_id`, `subject_name`, `subject_follow` (authored heading,
orbit or chase), `subject_forward`/`subject_up` in the named `subject_grid`,
`body_velocity`/`body_angular_velocity` in the body frame (or unit when absent),
`delta_seconds`, `collision_available`, `obstruction_distance` (unit for no hit),
and `settings`. No backend Entity bits, JSON or private pose storage cross this
boundary. Policies receive facts and return decisions; they do not write a pose.

The settings and partial decision share these fields:

| Field | Meaning |
|---|---|
| distance | Positive boom length in metres |
| yaw / pitch | Radians; pitch strictly between -pi/2 and pi/2 |
| height | Focus offset along surface up (world Y for a flat grid) |
| focus_x / focus_z | Focus offsets along surface east / south, or flat-grid X / Z |
| attitude | `world`, `heading`, or `body` |
| track_heading | Whether heading mode includes the subject heading |
| rotation_alpha / position_alpha | Per-presentation-frame interpolation weights in [0,1] |

Rhai can choose an easing law using `delta_seconds` (for example
`1-exp(-rate*dt)`), offsets, and mode from the selected vessel's facts. These are
presentation decisions, separate from physical or Modelica integration. The
existing orbital rig, mounted cameras, and subject-binding transactions remain
the owners of their respective camera modes; this seam tunes the spring arm.

The solver sweeps the lens-centred camera sphere along the focus offset, boom
and actual damped movement. Native prop casts exclude the followed articulated
assembly and native DEM realizations. The terrain owner sweeps the full composed
source independently of physical support residency. The camera envelope encloses
the near-plane corners; source overlap and unavailable work remain explicit.
Movement casts the physical sphere and applies skin as a directional pullback
from an approaching contact, matching Avian's movement mechanism. The margin is
not an enlarged colliding shell, so clear tangent travel remains possible.
The nearest source or prop contact retracts the boom before pose publication;
recovery eases outward. The retained draw-geometry query then certifies presentation.
Coarse or preparing geometry holds the pose and requests stable refinement,
without introducing an upward offset. A policy cannot disable this safety.
Preparation covers the continuous requested corridor and persists across render
frames while its camera position, output, source and projection remain valid.
The spring arm publishes orientation and translation together after admission;
preparation holds the rig pose while free-flight look retains its own input owner.
Rotation input alone does not cancel stationary preparation. Capacity refusal
is reported explicitly rather than admitting a partially prepared corridor.
Narrow ridges, steep slopes, props and near-plane clearance still require
production visual acceptance.
`scripts/api/test_camera_terrain_follow.py` runs the production scene and
checks clearance and actual displacement while a long boom turns through eight
directions, allowing its declared bounded preparation window only while work is
pending. It then tests low free flight, capacity refusal/recovery, unsafe stationary
coarsening, render-origin rebasing, explicit traversal policy and distant zoom on
repository DEM fixtures. These gates prove clearance and bounded recovery rather
than a preparation-latency or frame-time target.

An absent optional policy retains the authored rig. A registered callback fault,
unknown field, invalid mode, non-finite number or out-of-range weight holds the
camera pose and publishes a `camera-follow` diagnostic. A valid next decision
clears it. No scene resources or policy state are cached in a second registry.
Twin closure retires its policy through the existing policy-layer owner.

`SetCameraFollow` atomically changes distance, height, yaw, pitch, mode and damping
on the live local rig, without releasing possession or rebinding its subject.
Omitted fields remain unchanged; invalid batches reject before mutation. Angles
narrow only at the render-rig boundary. Existing component/resource inspection
and reflection remain available for native settings.

Camera cycling consumes `UserIntent::SwitchMode` through `lunco-avatar-input`
and emits `CycleCamera`; USD camera projection observes that request and selects
the next window camera. The authored keymap owns the physical key (default V).
No raw C key is reserved by the camera runtime.
