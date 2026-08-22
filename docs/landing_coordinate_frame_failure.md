# Lander landing: active BigSpace frame and Avian state

## Purpose

This is the engine-level record of the moving body-fixed-grid failure exposed by
the tutorial recording. The campaign is only the reproducer. The ownership is
the typed BigSpace/Avian bridge; there is no lander-specific pose reset, force,
camera correction, compatibility API, or frozen-clock workaround.

## Root cause

The scene is authored below a Moon body-fixed surface `Grid`. During scene
mount, the engine selects that site grid as `ActivePhysicsFrame`, the frame in
which Avian's f64 `Position`, `Rotation`, and velocity state are solved.

The bridge had two independent first-observation paths: the canonical
`pose_to_position` read/transport pass, and a hold-safe fixed-update seed pass.
When the site frame changed while physics admission was held, the seed pass
captured newly spawned bodies as already belonging to the new site frame, while
the bridge transaction state still correctly recorded the previous WorldRoot
frame for bodies already seeded. The next physics read then applied the
old-to-new transform to a mixed set of states. Bodies already seeded in the new
frame were transported a second time, producing astronomical Avian positions
and velocities before the first admitted solve.

The static terrain, joints, and colliders were local to the site, so the solver
saw an enormous body/constraint mismatch. The resulting contact and joint
impulses produced the apparent moving-grid escape. It could look like a
velocity-transport defect because the failure appeared when the celestial
hierarchy advanced, but the first invalid state was the split frame
transaction itself.

After that bridge defect was removed, the same scenario exposed three
independent errors at the flight-software boundary. They were not repaired by
loosening assertions or delaying the mission:

- the GNC graph collapsed the sensor's physical `range_valid` bit and its
  vertical `range_confidence` into one `altimeter_valid` input. An oblique but
  geometrically valid ray was therefore discarded as a lateral position
  observation;
- altitude was reconstructed with a fixed sensor-to-COM Y offset. The authored
  mount offset must first be rotated into the navigation frame, so a fixed
  `range + offset` expression is wrong whenever the body tilts;
- when vertical evidence returned after an IMU-only interval, the alpha-beta
  observer applied its full position innovation immediately. That legitimate
  measurement re-entry injected an artificial velocity impulse into the
  estimator and the controller amplified it.

The last two errors explain why an apparently correct ray and a correct
BigSpace/Avian pose could still produce a runaway lander. They are coordinate
and observation-authority errors, not physics-frame transport errors.

## Architectural contract

BigSpace owns the hierarchy representation: `Grid`, `CellCoord`, and
`Transform` are composed with its typed cell-chain operations. Avian owns the
physical state. The bridge is the sole boundary between them.

For a physical body below `ActivePhysicsFrame`, the bridge always uses the
active-frame projection (`grid_relative_pose_seeded`) for initialisation and
external pose changes. For bodies outside that branch it uses the typed
`pose_in_grid_seeded` projection into the active frame. No `GlobalTransform`,
raw celestial translation, or guessed cell arithmetic is used at this boundary.

Celestial ancestors above the selected surface frame are render/inertial
presentation. They are not copied into the local Avian solve each tick. This is
the stable BigSpace arrangement: physics remains in one numerically local frame
while the celestial hierarchy can rotate and translate for presentation.

Coordinate topology is explicit at every conversion boundary. `Some(CellCoord)`
is composed only with a real parent `Grid`; `None` means the entity carries an
ordinary parent-local `Transform`. A cell attached below a non-Grid parent is a
`CoordinateError`, not cell-zero or raw-translation input. Missing spatial
ancestors, cycles, and over-depth chains are also errors. Presentation/query
callers may report an unavailable result when their optional target is not ready,
but they never substitute another coordinate frame; the physics bridge panics on
the same malformed topology instead of continuing with stale state.

If the authored active physics frame itself changes, the bridge performs one
explicit transaction at the physics boundary:

- convert position, orientation, linear velocity, and angular velocity with
  `grid_transform_between_grids`;
- invalidate Avian's public XPBD joint multipliers and contact warm-start
  impulses, which are expressed in the old coordinate basis;
- let Avian rebuild contacts and constraints from the transported state before
  the next solve.

That is a coordinate-basis change, not an impulse. The bridge does not invent
astronomical translational velocity for a body-fixed site solve, and it does
not maintain a parallel physics cache. BigSpace's own representation-only
cell re-split remains distinct from a semantic physical frame change.

The sensor/GNC boundary is explicit as well. `Altimeter` publishes the raw-hit
validity, vertical usefulness, and back-projected vehicle position in all three
navigation axes as separate signals. The position observer uses the geometric
hit for X/Z and the vertical-confidence signal for Y. The vertical observer's
authority is a continuous Modelica state with an authored acquisition time
constant; it ramps measurement authority continuously and begins releasing it
as soon as the evidence disappears. No frame-count wait, mission-script handoff,
truth-position feed, or stale-value fallback is involved.

## Implemented fix

`crates/lunco-usd-avian/src/big_space_bridge.rs` now has one bridge-owned
read/transport state machine. The same `pose_to_position` pass runs before
nested physics admission and during normal physics ticks; it seeds never-seen
bodies, commits active-frame handoffs, transports the complete Avian state, and
publishes `PhysicsPoseSeeded` only after the pose is in the committed frame.
There is no parallel seed pass that can label a body with a frame the
transaction has not committed.

The scene lifecycle also resets `ActivePhysicsFrame` immediately during
celestial teardown. This prevents a deferred teardown command from overwriting
a replacement site frame after a scene transition.

USD avatar projection follows the same rule: it requires a connected BigSpace
`Grid` and a valid typed pose chain, then derives the camera's actual `CellCoord`
and local transform from that frame. It does not create a cell-zero avatar when
the authored hierarchy is incomplete.

The reusable lander scene and the recording scene now wire the complete
altimeter contract (`range_valid`, `range_confidence`, and
`vehicle_position_x/y/z`) into the canonical GNC ports. The fixed
`altimeter_valid` and `altimeter_mount_offset` interfaces were removed rather
than retained as aliases.

## Evidence

Native bridge integration coverage passes 15/15, including:

- first active-site-frame seeding;
- velocity transport across an active-frame handoff;
- astronomical cell offsets and BigSpace re-splits;
- rotating celestial parents with free and jointed surface assemblies;
- child-collider contact, scaled rootless grids, teleport wake-up, and nested
  lander-like strut contacts;
- a body spawned during the active-frame handoff, proving it is seeded directly
  in the committed frame while an existing body's velocity is rebased exactly
  once.

The Modelica sensor contract suite passes 7/7, including shared-frame
conversion, raw-ray altimeter conversion without a fallback, and both reusable
stateful measurement boundaries.

The USD connection derivation suite passes 12/12, including the lander asset
composition and authored actuator/sensor connection topology.

The rebuilt DEBUG engine ran the complete lander GNC probe through `t=98 s` and
reported `TESTS_OK 45` and `EPISODE GNC PROBE: PASS`. Both landers remained in
the local site frame with finite positions and velocities, engines off, and
bounded body rates. The log contains no `physics-body-escaped`, terminal
runtime failure, callback failure, or solver panic. This probe exercises the
real USD composition, Modelica sensor/GNC models, Avian state, and the normal
celestial clock; it is not a unit-only recognition test.

The rebuilt DEBUG engine now completes the canonical episode recording with the
campaign's stable controller profile. The production run at API port 5040
captured all six MP4 streams at 720x1280 and 60 FPS: 480, 479, 599, 480, 479,
and 3,359 frames. The log contains no transport escape, terminal runtime
failure, callback failure, or panic, and port 5040 closed after exit. The
recorder now rejects a non-zero simulator exit even when partial MP4s exist, so
a partial take cannot be reported as successful.

The earlier `hold_kd=3` take was also an underdamped contact profile and remains
invalid evidence. The canonical scene authors the stable `hold_kd=4` profile
directly. That controller tuning is separate from, and does not replace, the
coordinate/API fixes above.

A preceding port-5037 run reproduced the same unstable-profile failure with a
different leg: shot 06 stopped at frame 2,328 when `BaselineLander/LegNZ`
reached `DVec3(680.52, -326.70, 742.48)` with approximately `187,648 m/s`
horizontal velocity. That partial remains invalid evidence; it is retained as
the root-cause diagnostic and is not used as a successful take.

## Regression requirements

The focused native tests and the production probe must remain part of the
coordinate boundary acceptance. A valid production acceptance uses the normal
celestial clock and does not add `SetClock(... scale: 0.0)` to hide a transport
defect. The recorder must produce every episode take with all lander bodies,
joints, and contacts finite and in the same active site frame.

## Non-solutions deliberately excluded

- freezing celestial time;
- adding per-lander forces, pose snaps, or velocity clamps;
- copying `GlobalTransform` into Avian;
- preserving obsolete physics-clock or coordinate aliases;
- keeping a second compatibility bridge beside the canonical active-frame API.
