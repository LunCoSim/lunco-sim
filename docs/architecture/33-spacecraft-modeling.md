> **Status:** Implemented — G1/G1b/G2/G3/G4/G4b/G5/G6/G7/G9/G10/G11 done (G5 on an
> isolated differential rig; G9 = generic joint actuation + USD drive schema;
> G7 = spherical/distance joints; G10 = USD-authored sensors; G11 = single-track
> lean wheel). All Avian joint-building consolidated in `lunco-usd-avian`. G8
> (determinism/FMI) is a separate track.
> **Audience:** Engineers working on vehicle, cosim, and USD-physics subsystems

# 33 — Modeling Spacecraft (Landers & Rovers) in LunCoSim

## Goal

1. **Accurate lander** with a Modelica engine model behind the thrust.
2. **Extract rover dynamics from Rust into USD** so vehicle *structure and wiring*
   are authored, not coded — enabling 6-wheel rovers, rocker-bogie suspension,
   etc. without bespoke Rust per vehicle.

This doc establishes the layering, then lists the concrete gaps that block it.

---

## The layering principle

The system already separates concerns well. Make it explicit and finish it:

| Layer | Owns | Lives in |
|---|---|---|
| **Structure + wiring** ("what") | bodies, colliders, mass/inertia, joints, vehicle topology, model bindings, cosim wires | **USD** (authored) |
| **Subsystem dynamics** ("how a part behaves") | engine thrust, propellant, battery, thermal, controllers | **Modelica / rhai** (cosim) |
| **Physics substrate + behavior library** | solver, generic force/joint/port plumbing, *parameterized* primitives (raycast wheel, suspension law, friction circle) | **Rust** (reusable, never bespoke) |

Rule: **Rust ships a library of parameterized physical behaviors; it never hardcodes
a specific vehicle.** A "6-wheel rover" is a USD file, not a Rust struct. This matches
the existing `feedback_no_bespoke_materials_use_shadermaterial` philosophy applied to physics.

---

## What already works (no Rust changes needed)

The earlier assumption "rover is hardcoded in Rust" is **largely false today**:

- **Rover structure is data-driven from USD.** `lunco-usd-sim::process_usd_sim_prims`
  maps NVIDIA PhysX-Vehicle API schemas → ECS. Wheel **count, positions, radii,
  suspension/engine/tire params, drive type (skid vs Ackermann)** all come from USD
  attributes. Two authored examples: `assets/vessels/rovers/{skid,ackermann}_rover.usda`.
- **Joints are USD-authorable.** `lunco-usd-avian` builds Avian `Revolute`,
  `Prismatic`, `Fixed` joints from `Physics*Joint` prims (body refs, axis, anchors, limits).
- **Compound rigid bodies** follow OpenUSD spec: `PhysicsRigidBodyAPI` on parent +
  `PhysicsCollisionAPI` on children → one body, compound collider.
- **Cosim binding is USD-authored.** `lunco:modelicaModel="models/Foo.mo"` +
  `lunco:simWires="out:port,in:port"` binds a model to a body and wires its I/O.
  Live example: `assets/vessels/balloons/modelica_balloon.usda`.
- **Force application path exists.** wire → `PendingForces` accumulator →
  `apply_pending_forces` → Avian `Forces`. Gravity applied separately by
  `lunco-environment` (don't double-apply in `.mo`).
- **Engine model exists.** `assets/models/RocketEngine.mo` (throttle→thrust,
  propellant burn). `AnnotatedRocketStage.mo` is a full acausal Tank→Valve→Engine→Airframe.

So both goals are ~70% infrastructure-complete. The work is closing a **small, shared
set of gaps**, most of which serve *both* the lander and the rover.

---

## Gap analysis (prioritized)

### G1 — Body-frame force + torque cosim ports  **[DONE]**
Force ports were **world-space only**. A gimbaled lander thrust, a reaction wheel,
or an RCS thruster all need **body-frame force** and **torque**.
- **Shipped:** `force_local_{x,y,z}` + `torque_{x,y,z}` added to `RIGID_BODY_GROUP`
  (`crates/lunco-cosim/src/avian.rs`). `PendingForces` extended with `f_local` +
  `torque`; `apply_pending_forces` applies via avian `apply_local_force` /
  `apply_torque` (avian does the attitude rotation — no manual quat math).
  `propagate.rs` unchanged. Compiles clean.
- Unlocks gimbaled thrust, thrusters, reaction wheels, any body-frame actuation.
- **Verified end-to-end** via `Lander.mo` + `lander_test.usda` on the headless
  `sandbox-server`: lander fell from 12 m, engine arrested it to a soft hover at
  the 6 m set-point — `thrust ≈ 14716 N ≈ mass·g`, `vy ≈ 0`, controller `a_cmd ≈ g`.
- Booting the lean headless server also required fixing pre-existing headless-safety
  bugs (render-gated resources held non-optionally by CPU systems): `WorkspaceResource`
  made optional in `lunco-networking` tutor systems; `Assets<ShaderMaterial>` store
  `init_asset`'d in `TerrainSurfacePlugin`. Use the `sandbox-server` bin for headless,
  not `sandbox --no-ui` (the latter keeps `ui` systems compiled without a GPU).

### G1b — Attitude + body-rate output ports  **[DONE]**
The rigid-body port group exposed only the *translational* half of state
(`position_*`, `velocity_*`). A spacecraft controller is blind without orientation.
- **Shipped:** `quat_w/x/y/z` (canonical attitude), `yaw`/`pitch`/`roll` (euler
  convenience, `YXZ` = yaw-pitch-roll for a Y-up world), and `angvel_x/y/z` (body
  rates) added to `RIGID_BODY_GROUP` as read-only outputs from Avian `Rotation` /
  `AngularVelocity`. Pairs with the `torque_*` inputs to close an attitude loop —
  read attitude+rates → compute corrective torque → write `torque_*`. Compiles clean.

### G2b/G3 — Live mass-properties ports (mass + inertia + COM)  **[DONE]**
The triple moves together (propellant burn lightens mass, shifts COM, shrinks
inertia), so they're exposed as one read+write set on `RIGID_BODY_GROUP`: `mass`,
`inertia_xx/yy/zz`, `com_x/y/z` — reachable by wires, API, rhai, python, inspector
through the unified **`PortRegistry`** (see doc below / `project_port_registry_substrate`).

**The write mechanism** (the real subtlety): Avian splits user *overrides*
(`Mass`/`AngularInertia`/`CenterOfMass`) from the engine `Computed*` the integrator
reads. **Reads** return `Computed*` (the effective value). **Writes** set the
*override* — avian recomputes `Computed*` from it, and an override takes precedence
over collider-derived mass, so **no `NoAuto*` marker is needed**. (Writing `Computed*`
directly is wrong: `lunco-usd-avian` inserts a `Mass` override from `physics:mass`,
and avian recomputes `Computed*` from it each step, clobbering the direct write — this
was found and fixed during verification.) Overrides are `f32`; principal (diagonal)
inertia only — off-diagonal left to static USD authoring. Verified: `SetPort
mass/inertia_xx/com_y` all stick on read-back while the lander keeps hovering.

### G2 (load-time inertia/COM)  **[DONE]**
`lunco-usd-avian` now reads `physics:diagonalInertia` and `physics:centerOfMass`
at spawn → Avian `AngularInertia { principal, local_frame }` / `CenterOfMass`
*override* components (the SAME components the runtime mass-props ports write, so
authored and model-driven values share one path). Implemented as a single
`apply_rigid_body_mass_props` helper called from both the main
`PhysicsRigidBodyAPI` path and the legacy `rigidBodyEnabled` fallback — which
also fixed the WP-3-flagged mass-handling divergence (the fallback used to skip
the 1000 kg default). `physics:principalAxes` (a quat rotating the principal
frame) defaults to identity — off-diagonal inertia is left to that quat and is
almost always identity for landers/rovers. `gravity_accel` is auto-injected into
models.

### G4 — Arbitrary actuator port topology authored in USD  **[wiring DONE; mix = G4b]**
The wheel→actuator **wiring topology** was hardcoded in two Rust spots: the port
*set* (4 fixed names) and `try_wire_wheel`'s `index%2`/`index<2` parity. Both are
now USD-authorable:
- **Per-wheel binding** — `lunco:drivePort` / `lunco:steerPort` (token) on a wheel
  prim name the FSW port it wires to, overriding index parity. So a 6-wheel rover,
  per-wheel independent drive, or a non-2×N layout is declared in USD, not coded.
  Unauthored wheels keep the documented parity default (even→`drive_left`,
  odd→`drive_right`, front→`steering`), so the 4-wheel base rovers are unchanged.
- **Extensible port set** — `lunco:drivePorts` (space-separated tokens) on the
  rover root spawns extra named `DigitalPort`s beyond the canonical four, which
  wheels bind to and a wire/rhai/Modelica mix drives.
- **Proof:** `assets/vessels/rovers/six_wheel_rover.usda` — a self-contained
  6-wheel skid rover whose three-per-side wheels bind explicitly via
  `lunco:drivePort`; also authors G2 inertia/COM.

**G4b — authored mix  [DONE]:** `lunco:driveMix` on the rover root declares a
**linear per-port mix** — whitespace-separated `port=forward,steer[,brake]` terms
→ `GenericDriveMix` component (`lunco-mobility`). `on_drive_rover` projects the
`DriveRover` command onto every named port (`value = fwd·f + steer·s + brake·b`,
clamped, scaled to i16), taking precedence over the built-in skid/Ackermann
routing. Covers skid, Ackermann-style, and true **per-wheel independent** drive
(each wheel its own port + coefficients). While braking, forward/steer are forced
to 0 so only brake-coefficient ports stay live (matches the skid/Ackermann
branches). Proof: `assets/vessels/rovers/six_wheel_independent.usda` — six custom
`drive_w0..w5` ports, each wheel bound to its own, skid-steered through all six
independent channels with no `PhysxVehicleDriveSkidAPI`. Parse + projection
unit-tested. (Nonlinear mixes — e.g. exact Ackermann geometry — would still want
a rhai hook; the linear table covers the stated skid/per-wheel cases.)

### G5 — Differential / coupling constraint for rocker-bogie  **[DONE — verified]**
Rocker-bogie = chassis + rocker + bogie links via revolute joints **plus a
differential** that averages the two rockers' pitch. Avian has no gear/differential
joint, so this is a Rust **soft holonomic coupling**.

- **USD reader** — `lunco:differential:rockerA/B` (rel) + `:axis`/`:stiffness`/
  `:damping`/`:restSum` on a chassis prim → `PendingDifferential` →
  `resolve_differential_coupling` (deferred, matches prim-paths → entities, gated
  on `With<Position>`, same pattern as USD joints) → `DifferentialCoupling`
  (`lunco-mobility`). Reads each rocker's chassis-frame pitch (swing-twist about the
  hinge axis) and applies a PD torque enforcing `θ_a + θ_b → rest_sum`, equal on
  each rocker, `−2τ` reaction on the chassis (momentum-conserving). Idle unless a
  `DifferentialCoupling` exists, so it's free for every other vehicle.
- **Scalar PD law** (`angle_about_axis`, `differential_torque`) is unit-tested.
- **Verified live** on `differential_rig_test.usda` — a fixed base, a front-heavy
  rocker A + a balanced rocker B on lateral revolutes. A/B via the hinge `angle`
  ports: coupling OFF → A free-falls to the pendulum bottom (`+3.06`), B untouched
  (`+0.06`); coupling ON → A held at `+1.72`, B mirrors to `−1.65`, `θ_A+θ_B ≈ 0.07`.
  Textbook differential behaviour.

**Gotchas (both real, both cost a debugging loop):**
- **Stability:** the explicit penalty needs `stiffness < I/dt²` *and* damped rockers,
  else it rings/diverges (a first try at `k=4e5` on `I≈60`, `dt≈1/64` tripped avian's
  collider-AABB assert). Author rocker `physics:diagonalInertia` (G2) so `I` is sane.
- **Redundant rigs hide the effect:** a passive two-rocker rover where each rocker is
  pinned by its own two ground feet already self-levels — the coupling has nothing to
  do, so an A/B shows no difference. Demonstrating the differential needs a rig where
  the coupled DOF is otherwise free (the isolated `differential_rig` is that case;
  `rocker_bogie_test.usda` is the redundant one, kept as a caution).

### G6 — Finish USD-driven dynamics tuning  **[DONE (tuning); maxForce intentionally not honored]**
Tuning knobs an author of a dynamic vehicle actually sets are now USD attributes:
- **Raycast:** `lunco:driveForcePerNormal` (was const 2.0) → per-wheel
  `WheelRaycast.drive_force_per_normal`.
- **Joint:** `lunco:maxDriveOmega` (12), `lunco:driveDamping` (30),
  `lunco:stallTorqueGain` (6), and the joint wheel `physics:mass` (was a hardcoded
  `Mass(100)`) — all read into a `JointDriveParams` struct in `setup_physical_wheel`.
  Defaults reproduce the verified feel.
- **Left as a const (deliberately):** `MAX_SUSPENSION_FORCE_N` (100 kN) is a
  numerical guard-rail (caps a deeply-compressed strut / velocity-spike impulse),
  not a feel knob — documented as such.
- **`drive:angular:physics:maxForce` is still NOT honored**, on purpose: the demo
  scenes author it at 12000, which fed straight into the motor made the rover
  apply ~30× its lunar weight and wheelie on every input (see
  `project_physical_rover_suspension`). The engine `peakTorque × stallTorqueGain`
  is the canonical drive authority and is now itself USD-tunable, which is the
  right way to raise drive force without the regression.

### G9 — Generic joint actuation + USD drive schema  **[DONE]**
Joint **motor drive** used to be revolute-only: a revolute joint auto-exposed an
`angle` port, but a prismatic joint was *built and then undrivable* — no cosim
port, no way to deploy a landing-gear strut, raise an elevator/piston, or extend
an arm stage from a wire/FSW/rhai/Modelica. The `AVIAN` port table was hardcoded
to `[RIGID_BODY_GROUP, REVOLUTE_JOINT_GROUP]` (`lunco-cosim/src/ports.rs`).
- **Prismatic `displacement` port** — `PRISMATIC_JOINT_GROUP` (`lunco-cosim/src/
  joint.rs`), the translational mirror of the revolute `angle` group: `In` drives
  the Avian `LinearMotor` (position control), `Out` measures the signed slider
  offset (anchors projected onto the world axis). One entry in the `AVIAN` table —
  no new struct/observer/system, exactly the extension the table was built for.
- **USD / Omniverse drive schema** — `lunco-usd-avian` now reads the standard
  `UsdPhysicsDriveAPI` at load: the `linear` instance on a prismatic joint, the
  `angular` instance on a revolute one (`drive:{linear,angular}:physics:
  {targetPosition,targetVelocity,maxForce}`). An authored target **enables the
  motor at load**, so an Omniverse-authored mechanism seeks its setpoint with no
  wire; `physics:maxForce` replaces the hardcoded motor saturation. A cosim wire
  on the joint's port overrides the target per tick. The port pair is the runtime
  face of `PhysxJointStateAPI:{linear,angular} physics:position` (out) +
  `PhysicsDriveAPI` `targetPosition` (in).
- **Not yet mapped:** `physics:stiffness`/`physics:damping` — Avian's `MotorModel`
  reparameterizes these as frequency/damping-ratio (needs body mass), so the proven
  overdamped 3 Hz spring-damper is kept as the model; the load-bearing knobs
  (`maxForce` + targets) are honored. Wheels are unaffected: their revolute joints
  are built in `lunco-mobility`, not the authored-joint path, so the G6
  `drive:angular:maxForce` wheelie cannot recur here.
- **Proof:** `assets/scenes/sandbox/prismatic_drive_test.usda` — a standard
  `PhysicsPrismaticJoint` + `PhysicsDriveAPI:linear` elevator (50 kg platform).
  **Verified live** on the headless `sandbox-server`: (A) the USD load-time drive
  holds at `-1.5276` (target `-1.5`; ~0.027 m droop = spring-damper steady-state
  under load); (B) `SetPort displacement -0.5` → `-0.5276`; (C) `+0.3` → `+0.2716`;
  (D) `-5.0` clamps at `-3.0000` (authored `limitLower`). Projection math
  unit-tested (`lunco-cosim` `joint::tests`).
- **Gotcha (cost a debug loop):** UsdPhysics/Omniverse authors physics scalars as
  `float`, so `prim_attribute_value::<f64>` silently returns `None` — the drive and
  joint limits must be read **f32-first** (`read_scalar_attribute`). The first run
  fell straight through the (also-`f64`-read) limit before this was fixed. Same
  class as the `localPos` "bodies launched into orbit" trap — *all* USD physics
  scalar reads are f32-first.

### G7 — Extra joint types (spherical / distance)  **[DONE; D6 reduces or warns]**
`lunco-usd-avian::build_usd_physics_joints` now builds two more avian joint kinds
from standard UsdPhysics prims:
- **`PhysicsSphericalJoint`** → avian `SphericalJoint` (ball joint: 3-DOF swing +
  twist). `physics:axis` = twist axis; `physics:coneAngle0/1Limit` → swing cone
  (larger half-angle as a symmetric bound, since avian carries one swing
  `AngleLimit`); `physics:limitLower/Upper` → twist limit. Suspension uprights,
  robotic wrists, gimbals.
- **`PhysicsDistanceJoint`** → avian `DistanceJoint` (tether/strut within
  `[physics:minDistance, physics:maxDistance]`). Cables, fixed-length links.
- **Generic `PhysicsD6Joint`/`PhysicsJoint`** has no avian primitive (avian offers
  fixed/revolute/prismatic/spherical/distance, not a configurable 6-DOF
  constraint). It warns with guidance to author an explicit joint for the needed
  DOF — full D6 reduction (per-DOF `PhysicsLimitAPI` analysis) is the remaining
  edge.
- **Verified live** on `assets/scenes/sandbox/g7_joints_test.usda`: both build
  with no "Unsupported" warning; the distance-tethered Weight settles at exactly
  `2.0 m` below its anchor (= `maxDistance`); the ball-jointed arm hangs stable.
  Also note **all programmatic joint construction now lives in `lunco-usd-avian`**
  — the wheel revolute joint moved out of `lunco-usd-sim::setup_physical_wheel`
  into `lunco_usd_avian::wheel_revolute_joint` (one home for joint-building).

### G10 — USD-authored sensors (IMU / range / contact)  **[DONE]**
Telemetry was limited to a body's own kinematic state + joint DOFs — no *sensor*
concept. `lunco-cosim/src/sensors.rs` adds three USD-authorable sensor kinds, each
a component with cached outputs filled by a small system and surfaced through the
same port mechanism as the rigid body (gated on the marker, so unsensed bodies pay
nothing). Authored in `lunco-usd-sim` from `lunco:sensor:*`:
- **`lunco:sensor:imu`** → `ImuSensor` → ports `accel_x/y/z` (world-frame linear
  acceleration, finite-differenced from `LinearVelocity`). Pairs with the existing
  `angvel_*` + `quat_*` for a full 9-DOF IMU. (Body-frame specific force —
  subtracting gravity — is a future refinement.)
- **`lunco:sensor:range`** (+ `:rangeAxis` token, `:rangeMax`) → `RangeSensor` →
  port `range`. A raycast altimeter/lidar along the body-local axis (default `-Y`).
- **`lunco:sensor:contact`** → `ContactSensor` → ports `contact` (0/1) +
  `contact_force` (N). From avian's `Collisions`.
- **Verified live** on `assets/scenes/sandbox/sensor_test.usda` (100 kg box at
  rest): `range = 0.500` (exact centre-to-ground), `contact = 1`, `accel ≈ 0`
  (correct for a static body). Caveat: `contact_force` reads ≈2× the static weight
  at rest — it currently includes the solver's penetration-correction impulse, not
  just gravity support; it tracks load but the absolute calibration is a known
  refinement.

### G11 — Single-track / lean wheel (bikes, motorcycles)  **[DONE]**
The raycast wheel decomposed traction in a flat wheel basis (forward `-Z`/right
`+X`), assuming an upright wheel on a flat patch — a leaning two-wheeler got its
lateral force in the wrong plane. `lunco-mobility::contact_plane_basis` now builds
the traction basis in the **actual contact plane** (the ray hit normal): the wheel
heading projected onto the plane ⟂ to the contact normal, `right = forward ×
normal`. For an upright wheel (normal ≈ wheel up) this is *mathematically identical*
to the old basis — existing rovers are unchanged (unit-tested + live: a six-wheel
rover rests at `speed 0`, no drift). For a leaning bike the basis follows the
cambered contact plane, so drive + lateral grip are computed correctly. Unit-tested
(`force_law_tests::contact_basis_*`). **Remaining for full fidelity** (deferred):
raked steering-head axis (steer is still about chassis `Y`, `lib.rs:539`) and
gyroscopic precession — balance itself is a *controller* (already expressible via
the torque + attitude/rate ports), not a substrate gap.

### G8 — Determinism / FMI interop  **[SEPARATE TRACK]**
Live cosim is non-deterministic, no FMU. Matters if a lander descent must be
reproducible. Out of scope for first pass.

---

## Concrete recipe: the accurate lander

1. **USD prim** `/World/Lander`: `PhysicsRigidBodyAPI`, `physics:mass`,
   `physics:diagonalInertia`, `physics:centerOfMass` (needs G2), collider(s).
2. **Bind model**: `lunco:modelicaModel = "models/Lander.mo"` (extend `RocketEngine.mo`
   with gimbal angles + throttle).
3. **Wires** (`lunco:simWires`):
   - `thrust:force_local_z` (body-frame, needs G1)
   - `gimbal_torque_x:torque_x`, `gimbal_torque_y:torque_y` (needs G1)
   - `mass:mass` (propellant burn, needs G3)
   - feedback: `height:height`, `velocity_y:velocity_y`, attitude → model inputs
4. **Gravity** stays on Avian. `Lander.mo` exports thrust only (mirror `Balloon.mo:53`).
5. **Guidance/throttle controller**: rhai or a Modelica controller reading
   altitude/velocity → throttle/gimbal commands (closed-loop descent).

**Minimum to fly a lander: G1.** G2/G3 make it *accurate*.

## Concrete recipe: 6-wheel rover / rocker-bogie

1. Author 6 `PhysxVehicleWheelAPI` wheels (count is already data-driven).
2. Rocker + bogie links as bodies joined by `PhysicsRevoluteJoint`s (USD today).
3. Differential coupling the two rockers — **needs G5**.
4. Drive/steer routing — **needs G4** for per-wheel drive or corner steering;
   plain left/right skid works today.
5. Real CG via **G2**.

**Minimum for a working 6-wheel skid rover: nearly today** (author the USD).
Rocker-bogie fidelity needs G4+G5+G2.

---

## Suggested sequencing

1. **G1** (body-frame force + torque) — unblocks the lander and all body-frame
   actuation; smallest, highest leverage.
2. **G2** (inertia/COM) — accuracy multiplier for everything.
3. Lander vertical slice: `Lander.mo` + scene + descent controller.
4. **G4** (actuator topology) — unblocks rover extraction beyond 4-wheel skid.
5. **G3 / G5 / G6** as the lander and rocker-bogie demos demand.

---

## Key file references

- Cosim force path: `crates/lunco-cosim/src/avian.rs:57,143`, `systems/propagate.rs:50`
- Gravity: `crates/lunco-environment/src/lib.rs:167`
- USD physics/joints: `crates/lunco-usd-avian/src/lib.rs:457,583,685`
- USD vehicle/wheel spawn: `crates/lunco-usd-sim/src/lib.rs:335,636,690,1026`
- Wheel physics: `crates/lunco-mobility/src/lib.rs` (raycast), `wheel_spin.rs`
- Model binding (USD→cosim): `crates/lunco-usd-sim/src/cosim.rs:113,467`
- Engine models: `assets/models/RocketEngine.mo`, `AnnotatedRocketStage.mo`, `Balloon.mo`
