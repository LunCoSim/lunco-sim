> **Status:** Design / analysis — not yet implemented
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

**Remaining G2 (load-time):** read `physics:diagonalInertia` / `physics:principalAxes`
/ `physics:centerOfMass` in `lunco-usd-avian` → Avian override components, so an
*authored* inertia/COM is honoured at spawn (today only scalar `physics:mass` is read).
The runtime ports above already let a model *drive* them. `gravity_accel` is auto-injected into models.

### G4 — Arbitrary actuator port topology authored in USD  **[MED-HIGH — rover extraction]**
The drive port set is **hardcoded to 4 names** (`drive_left/right/steering/brake`,
`lunco-usd-sim/src/lib.rs:640`). Fine for 4-wheel skid; insufficient for
per-wheel independent drive or 4-corner steering (Curiosity-class).
- Replace the fixed set with a **USD-declared port list** + a USD- or rhai-authored
  **mixing function** (skid/Ackermann/per-wheel). Move the mix out of Rust constants.

### G5 — Differential / coupling constraint for rocker-bogie  **[MED]**
Rocker-bogie = chassis + rocker + bogie links via revolute joints **plus a
differential** that mechanically averages the two rockers' pitch. Avian has no
gear/differential joint. Options:
- (a) Rust "differential constraint" primitive (couple two revolute angles), or
- (b) model the differential kinematically in Modelica as a constraint, or
- (c) a passive spring coupling (approximate).
Everything *except* the differential is buildable with today's revolute joints.

### G6 — Finish USD-driven dynamics tuning  **[MED]**
Holdouts still hardcoded in Rust: force-law constants (drive-per-normal 2.0, max
suspension 100 kN, contact grip 50), joint-wheel tuning (`MAX_DRIVE_OMEGA=12`,
`DRIVE_DAMP=30`, `Mass(100)`), and USD `drive:angular:physics:maxForce` is
**ignored** in favor of Rust engine `peakTorque`. Promote these to USD attributes;
honor the authored drive force.

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
