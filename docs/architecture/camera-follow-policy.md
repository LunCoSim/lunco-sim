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

The native solver checks the focus offset and boom against scene collision,
excluding the followed articulated assembly. It also checks the analytic DEM
in the active physics frame, because physical terrain colliders are streamed
only around contact bodies. The DEM boom check reserves the avatar collision
radius along the terrain's local up axis, even for shallow approaches. The nearest physics or
terrain hit retracts the boom before pose publication; recovery eases outward.
A policy cannot disable that safety. The terrain query is a sampled ray, not a
camera-frustum sweep; narrow ridges and near-plane clipping still require
separate visual acceptance.
`scripts/api/test_camera_terrain_follow.py` runs the production scene and
checks clearance continuously while a long boom turns through eight directions.

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
