# Lander landing: moving surface-grid / Avian frame failure

## Purpose

This is an engineering handoff for the coordinate-system work in the `usd/`
branch. The marketing episode is only the reproducible consumer. The fix belongs
in the USD/celestial/Avian frame boundary; do not solve this by adding lander-
-specific forces, pose resets, camera offsets, or a second GNC path.

## Short version

The landing scene is authored in a Moon body-fixed surface frame, while Avian
integrates `Position`/`Rotation` as grid-absolute physics state. The Moon surface
`Grid` is updated by `body_rotation_system` as `WorldTime.epoch_jd` advances.

`lunco-usd-avian::pose_to_position` currently notices the changed `Grid` and
re-reads every descendant body pose from the render transform. That copies the
new frame pose into Avian, but does not transfer the frame's linear/angular
transport velocity (or the equivalent relative-frame state). Dynamic landers and
their joints therefore see a discontinuous external pose change. Contact/joint
constraints inject energy until a leg leaves the world.

This is why the same lander/GNC setup can pass the headless probe and then fail in
the filmed scene. The GNC is not the first fault; the physics frame contract is.

## Reproduction and evidence

The episode is:

```text
/home/rod/Documents/lunco/lunco-marketing/campaigns/tutorial-series/episode_01_lander
```

The engine checkout used for the take is:

```text
/home/rod/Documents/luncosim-workspace/tutorials
```

### Control: headless probe passes

The native probe uses the same authored USD landers, sensors, Modelica models,
Avian legs, targets, and GNC parameters. It advances after the completed-physics
event, and passed:

```text
TESTS_OK 45
EPISODE GNC PROBE: PASS
luncosim test PASS ... ticks=11760 sim=196.00s ... jitter=0
```

The final probe state had both landers on their separate targets, engine thrust
and propellant flow at zero, four qualified pad contacts, quiet suspension, and
the required underdamped attitude-error crossings on the baseline lander.

Command used:

```bash
cd /home/rod/Documents/luncosim-workspace/tutorials
env -u LUNCO_ROLLBACK \
  RUST_LOG='warn,lunco_scripting::world_bridge=info' \
  target/debug/luncosim test \
  --scene /home/rod/Documents/lunco/lunco-marketing/campaigns/tutorial-series/episode_01_lander/episode_01_gnc_probe.usda \
  --max-ticks 12000 \
  > target/episode-gnc-probe-canonical-final.log 2>&1
```

### Failure: production recorder with a moving celestial frame

With the production recorder and the Moon surface grid advancing, the landers
leave the camera composition and then the solver becomes unstable. The terminal
failure observed in the simulator log was:

```text
physics-body-escaped: /Episode01Recording/Lander/LegNZ
position=DVec3(876.7658449, -840.5120411, -1862.1602655)
velocity=DVec3(580616.2690, -1061403.3663, -1989018.9535)
```

The recorder stopped after only 98 frames of shot 06. This is a physics escape,
not a render-only crop or a bad HUD.

The earlier diagnostic production take also printed the drift in the same
simulation sequence. At `sim_tick=120` the bodies were near the authored local
site; later the camera and the two bodies no longer shared the same local frame.
By `sim_tick=4320`, for example, the diagnostic showed approximately:

```text
tuned    [105.84, 16.28, 106.41]
baseline [ 24.31,134.16,  72.02]
camera   [ 29.53, 10.88, -21.98]
```

The two landers were not merely displaying different GNC responses; their
physical world poses had diverged from the camera/site frame.

### Diagnostic isolation: freezing only the celestial clock

As an isolation experiment, the episode temporarily authored:

```rhai
cmd("SetClock", #{ clock: "Celestial", parent: "Sim", scale: 0.0 });
```

The next production take completed all six files (`479, 479, 599, 479, 479,
3359` frames at 60 FPS) without `physics-body-escaped`. The simulator log records:

```text
[celestial] site scene re-branched onto body surface grid 1560v0 (body 301)
[time] clock command: clock=Celestial parent=Some(Sim) scale=Some(0.0)
```

This proves that advancing the celestial surface frame is the trigger. It is not
the final fix: the episode must not permanently depend on freezing celestial time
to hide an unsupported moving-frame transfer. Remove this command after the
coordinate boundary is repaired.

## Relevant ownership boundaries

### Scene migration

`crates/lunco-celestial/src/placement.rs` migrates the site root and its physical
descendants onto the body's surface grid in
`attach_site_scene_to_surface_grid`. The production log confirms this is
`MoonSurfaceGrid` entity `1560v0`, body `301`.

This migration is correct as an authoring operation: terrain, landing target,
lander bodies, and camera site content must share the body-fixed site frame. Do
not move the episode back to `WorldGrid` to make the failure disappear.

### Surface-grid motion

`crates/lunco-celestial/src/systems.rs::body_rotation_system` writes the body
grid rotation from `WorldTime.epoch_jd`. The body grid is intentionally rotating;
surface children inherit that rotation. `sync_inertial_anchors` deliberately
copies only position and leaves the inertial anchor rotation as identity, which
is the correct distinction between a surface frame and a star-fixed observer.

### USD/Avian read bridge

`crates/lunco-usd-avian/src/big_space_bridge.rs::pose_to_position` currently:

1. detects changed plain nodes, including a moving `Grid`;
2. marks descendant rigid bodies as moved;
3. composes each body's current render hierarchy with `world_pose_seeded`;
4. writes the resulting pose directly into Avian `Position`/`Rotation`.

The corresponding `position_to_pose` path converts Avian's grid-absolute solved
pose back to a parent-grid-local `Transform`. That position conversion is not
enough for a moving parent: no corresponding `LinearVelocity` or
`AngularVelocity` transport is applied when the parent frame changes.

The existing `q_rebranched` filter must remain: a paired `CellCoord + Transform`
change caused by BigSpace re-splitting is representation-only and must not be
treated as physical motion. A real moving-frame change and a recenter/rebranch
must be separate typed cases.

## Required fix in `usd/`

Choose and document one authoritative contract for dynamic bodies under moving
grids. The likely correct implementation is a generic moving-frame transport in
the bridge, not an episode special case:

1. Distinguish a physical parent-frame update from a BigSpace representation
   rebranch using the existing typed change information.
2. Capture the previous and current frame pose at the bridge boundary. Derive
   the frame delta over the actual physics step: translation velocity and
   angular velocity, with the same coordinate convention used by
   `world_pose_seeded` and Avian's global `Position`/`Rotation`.
3. Transform each descendant dynamic body's position and orientation into the
   new Avian frame without treating the change as a user teleport.
4. Transform its linear and angular velocity consistently, or run the solver in
   the body-fixed relative frame and apply the corresponding inertial transport
   terms. Do not write only pose and leave stale velocity behind.
5. Preserve joint anchors, contact manifolds, sleeping/waking state, and collider
   transforms in the same frame. A four-leg lander must not receive four
   independent teleports.
6. Keep kinematic/editor-drive handling separate from dynamic body transport.
   Do not add `PhysicsPoseAuthoritative`, per-lander resets, force impulses, or
   name/path checks as a substitute for the frame contract.
7. Make the conversion helpers typed and shared. The camera, terrain, sensors,
   gravity, Avian bridge, and telemetry must all use the same frame conversion
   path; no second hand-written ENU/grid/world conversion.

If the engine deliberately chooses a local body-fixed landing approximation for
short operations, that choice must be a generic authored frame/regime policy,
not a hidden episode hack. It must be explicit in the coordinate architecture and
must not be required only because the bridge cannot transport a moving grid.

## Regression tests to add

Add focused tests in the engine branch before re-recording the campaign:

### 1. Moving-grid free body

Create a rotating `Grid` with one dynamic rigid body and no forces. Advance the
frame by several physics steps. Assert:

- the body's parent-local pose remains constant;
- Avian position, rotation, linear velocity, and angular velocity stay finite;
- no artificial kinetic energy grows from the frame update;
- a BigSpace cell/transform rebranch alone does not change the physical pose.

### 2. Moving-grid joint assembly

Use the real four-leg lander topology (dynamic leg bodies, prismatic joints,
fixed pad colliders). Advance a rotating surface frame through the same bridge.
Assert that joint errors, leg stroke, and body rates remain bounded and that all
four pads retain their authored local offsets.

### 3. Production integration acceptance

Run both paths:

- the 196-second headless probe (`TESTS_OK 45`);
- the full 98-second production recorder with the default celestial transport
  running and **without** the temporary `SetClock(...scale: 0.0)` command.

The recorder must produce all six takes, never emit `physics-body-escaped`, and
keep both landers in the same local landing site. The final frame must show both
four-pad assemblies settled, with zero main-engine thrust/flow and RCS activity
derived only from the live control response.

## Definition of done

This issue is fixed only when the episode-level `SetClock` experiment can be
removed and the production take still passes. A successful take with the clock
frozen is useful isolation evidence, but is not evidence that the moving-grid
coordinate architecture is correct.
