> **Status:** Partially implemented — G1/G1b/G2/G3/G4/G4b/G5/G6 done (G5 verified on an
> isolated differential rig); G7/G8 deferred.
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

### G7 — Extra joint types (spherical / D6 / distance)  **[LOW — optional]**
Unsupported joints warn and fall through. Rocker-bogie can avoid them; landing-gear
or robotic arms may want them later.

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
