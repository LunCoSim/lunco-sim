---
name: authoring-vessel-controllers
description: >
  Author vehicle controllers in LunCoSim so spacecraft, landers, rovers, and
  drones can move, fly, drive, land, or accept pilot control. Use when adding or
  debugging autopilot, GNC, guidance, waypoint following, thruster response,
  manual takeover, a Modelica control model, a controller program prim, or the
  wired `piloted` authority signal. This skill assigns control math to Modelica,
  sequencing and events to Rhai, and structure, sensors, wiring, and authority
  to USD. It covers the unwired-input failure mode, PortRegistry input ownership,
  sensor-based feedback, and reuse of the closest production exemplar. The
  shipped lander is an example, not the universal production contract.
---

# Authoring vessel controllers

Read [`luncosim-architecture`](../luncosim-architecture/SKILL.md) before
adding a sensor, actuator, generated network, or custom USD field. This skill
owns the vessel-controller recipe; the architecture skill owns the
standard-schema and no-compatibility gate.

A vessel that drives itself (a GNC, an autopilot) is built from **three layers,
each in the language that fits it**. Never blur them.

| Layer | Language | Owns | Rule |
|---|---|---|---|
| **Control LAW** | **Modelica** (`.mo`) | the math: PID, schedules, mixing, τ=I·α | ALL control math lives here. Never compute a control law in rhai. |
| **Logic / sequencing** | **rhai** (`.rhai`) | phases, events, mission steps, reactions | EVENT-DRIVEN only. No per-tick control loops, no time-stepping. |
| **Structure / wiring / authority** | **USD** (`.usda`) | sensors, wires, possession, identity | Declarative. Sensors are referenced library prims; a wire is a native USD connection. |

The shipped lander is one reference implementation: `assets/models/Lander.mo`,
`assets/scenarios/lander_subsystems.rhai`, `assets/vessels/landers/descent_lander.usda`
(referenced by `assets/scenes/luncosim/lander_ops.usda`). For another vehicle,
select the closest **production** controller, vehicle asset, composing scene,
and test before authoring a new one. A tutorial model can clarify the idea while
still being too simplified to serve as the production contract.

## Start with the closest production exemplar

Before writing a new controller:

1. Search `assets/models/`, `assets/vessels/`, `assets/components/`,
   `assets/scenes/`, and `assets/scenarios/` for an existing controller and its
   consumer.
2. Read the model's declared inputs/outputs, the USD wires, and the scene test
   that observes the behavior.
3. Reuse the same authority, sensor, frame, and actuation boundaries. Override
   vehicle facts in USD and controller tuning through the established input or
   parameter contract.
4. Add a new `.mo` only when the existing equation or public interface is
   genuinely insufficient; do not fork a working controller for naming alone.

This is a discovery rule, not a requirement to reuse one particular vehicle.

## 1. The control law → a Modelica model

The model reads what the vessel **senses** and outputs force/torque. It is a PROGRAM,
and a program is a prim: the vessel's own flight-control system is inseparable from the
airframe, so the vessel prim applies `LunCoProgramAPI` and names the model in place —
`uniform asset info:sourceAsset = @models/MyController.mo@`. Its `inputs:` ARE
the vessel's control surface. A control law that is *bolted on* (a guidance component, a
supervisory script) is a a `Scope` applying `LunCoProgramAPI` CHILD prim instead, so deleting the prim
removes the behaviour. Ports are wired with native USD connections (§3).

Because it drives a force on a body the client predicts, it must promise it steps fast
enough for that: `uniform bool lunco:program:realtimeSafe = true`. Without the promise
the wiring pass refuses it a `force_*`/`torque_*` port and says why.

```modelica
model MyController
  input Real altitude, descent_rate;      // SENSED (wired from sensors, see §3)
  input Real piloted = 0.0;               // authority gate (wired, see §4)
  input Real external_throttle = 0.0;     // the pilot's stick (when piloted)
  output Real force_y, throttle;
  Real gnc_throttle, cmd;
equation
  gnc_throttle = <your control law>;                 // math, DIRECT (no lag)
  cmd = piloted*external_throttle + (1.0-piloted)*gnc_throttle;  // yield-to-pilot gate
  force_y = cmd * max_thrust;
end MyController;
```

**Gotchas that will waste hours if you don't know them:**

- **rumoca folds unwired, algebraic-only inputs to their default.** An `input Real x`
  used only in algebraic equations, never wired and never written, is constant-folded
  → runtime writes to it never reach the solver. To keep an input LIVE, either **wire
  it** (see §3/§4) or **route it through a `der`** (`der(x_live)=(x-x_live)/0.02; use x_live`).
  Symptom: `set()` returns true but has no effect.
- **Inputs that feed a `der` are always live** (a state depends on them) — that's why
  `external_throttle`/`pitch` are spool-filtered (`der(filter)=(cmd-filter)/tau`): it
  both gives pilot feel AND keeps them live.
- **Keep the control (GNC) path DIRECT — no spool.** A lag on the autonomous path makes
  it sluggish and can tumble the vehicle. Spool only the pilot's stick.
- **rumoca mis-lowers `if` on algebraic vars.** Use `min`/`max` for clamps and a
  branch-free arithmetic blend (`a*x+(1-a)*y`) for selection, never a nested `if`.

## 2. High-level logic → rhai, event-driven

A a `Scope` applying `LunCoProgramAPI` child prim on the vessel, naming a `.rhai` scenario
(`uniform asset info:sourceAsset = @scenarios/my_supervisor.rhai@`), does
supervision and sequencing — **never a control loop**. React to events; don't poll or
step.

```rhai
fn on_event(me, evt) {
    if evt.name == "lander_touchdown" { /* advance the mission */ }
    if evt.name == "low_fuel" { notify_kind("Low fuel", "warn"); }
}
```

- Phase timing comes from the mission sequencer (`wait`, `wait_for`, `wait_until`) or
  from Modelica condition outputs connected to `LunCoEvent` prims, not `dt` counting:
  `def LunCoEvent "LowFuel" { float inputs:trigger.connect = </Lander.outputs:low_fuel>; uniform token lunco:event:name = "lander_low_fuel"; uniform token lunco:event:severity = "warning" }`.
- **Do not** write the vessel's command ports every tick from rhai. If you're tempted
  to, the logic belongs in the model (math) or the wiring (authority).

## 3. Sensors → USD library primitives, wired

The controller reads SENSORS, not the god-view body. Sensors are reusable prims in
`assets/vessels/sensors/` (`imu.usda`, `altimeter.usda`), referenced + mounted:

```usda
def "Altimeter" (prepend references = @../../vessels/sensors/altimeter.usda@</Altimeter>)
{ double3 xformOp:translate = (0, -3.3, 0); uniform token[] xformOpOrder = ["xformOp:translate"] }
```

A wire is a native USD connection, authored on the prim that CONSUMES the value —
`float inputs:descent_rate.connect = </Lander.outputs:velocity_y>`,
`float inputs:altitude.connect = </Lander/Altimeter.outputs:range>`. Wired inputs are
live (they reach the solver). Physical constants a model needs (mass, inertia) come from
the body's own ports (`inputs:vehicle_mass.connect = </Lander.outputs:mass>`,
`inputs:inertia_xx.connect = …`) — USD-derived, not magic numbers.

A **Modelica parameter** is authored as a typed USD constant and is placed in
the compile-time parameter set by contract classification. A runtime Modelica
`input` is a live signal and needs a native USD connection or an explicit
runtime writer; do not infer its kind from the USD spelling alone.

Rust publishes only generic built-in observations. The sensor asset and USD
connections identify placement and topology; Modelica performs filtering,
frame conversion, navigation, and control. Do not add a semantic sensor
registry, a world-coordinate force special case, or a fallback port when the
parsed Modelica contract does not match the authored scene.

## 4. Control authority → the `piloted` signal + possession

**This is the key pattern. Do not build a bespoke gate.**

- **The GNC is INTERNAL** (part of the model). **A user and an autopilot are both
  external SESSIONS** that *possess* the vessel; user-vs-autopilot is arbitrated by
  possession + RBAC (`may_take_control`), which already exists.
- The internal controller **yields** to whoever possesses via the **`piloted`** port:
  a read-only cosim port (`PILOTED_BACKEND`, `lunco-cosim/src/ports.rs`) that is `1.0`
  when any session owns the vessel (`SessionRegistry::owner_of(...).is_some()`), else `0`.
- Wire it (`float inputs:piloted.connect = </Lander.outputs:piloted>`) into the model and gate on it:
  `cmd = piloted ? pilot_stick : gnc`. Because it's wired it's a live input — **no
  in-model flag, no rhai toggle, no per-tick check.** Possession is the single source
  of truth; Rust never reasons about "autopilot" vs "user".
- The pilot's stick reaches `external_throttle`/`pitch`/… through the vessel's
  intent→port `Controls` scope (next section) when they possess. Camera-follow
  without taking control: `follow(entity)` (inserts a chase camera, no `ControllerLink`).

### Autopilots must drive like humans

An autopilot is only a different policy. It must acquire authority with
`PossessVessel`, then use the vessel's `ControlBinding`/intent surface and the
same live port writes as a human. Do not set `Position`, `LinearVelocity`,
`ModelicaModel.inputs`, or a private actuator component to make a scenario move;
those bypass possession, arbitration, and the authored input contract.

## What makes an entity *active* — the intent→port `Controls` scope

A vessel is **possessable + drivable** when it carries two things:

1. **An actuation surface** — the command ports a pilot or AI writes. A rover gets
   `throttle/steer/brake` from `PhysxVehicleContextAPI` (the projection stamps a
   separate `MobilityRoot` and `OutputPorts`, plus a `DriveMix` chosen by the
   drivetrain schema, e.g. `PhysxVehicleTankDifferentialAPI`); a cosim vessel gets its
   `.mo` inputs (`external_throttle`, `pitch`, …). This surface is topology-derived; you
   don't hand-write it.
2. **A `Controls` scope** — the intent→port map (stage 2 of control), read into a
   `lunco_core::ControlBinding`. Without it a vessel can be possessed but **keyboard
   input does nothing** — `drive_from_bindings` skips a bindingless vessel. (API /
   `set_input` / rhai can still drive it by port name — that path needs only the surface.)

The local `Avatar` is a separate domain role. It also has an `InputPorts` surface and
binding, but those ports are its free-flight embodiment and are not a vessel
possession target. Click resolution and `PossessVessel` enforce this boundary by
accepting a non-`Avatar` input surface; `SceneCamera` is presentation metadata and
does not participate in possession.

Author the scope as a **child `references` arc** to the shared profile — the SAME arc
kind the wheels use, so it composes through a spawn/reference. Root `subLayers` and
`inherits` do not provide the required spawned control profile.

```usda
# on the vessel prim — a rover:
def "Controls" (
    prepend references = @../control_profiles.usda@</RoverControls>   # lander: </LanderControls>
)
{
}
```

- Profiles live in `assets/vessels/control_profiles.usda`: `RoverControls`
  (forward/back→throttle, left/right→steer, brake→brake) and `LanderControls`
  (forward/back→body pitch, left/right→body roll, yaw_left/right→body yaw,
  thrust→external_throttle, release→release). The lander profile selects a
  stable `orbit` camera: camera orientation never remaps these body axes.
  W/S/A/D/Q/E and G are only the bundled `input_bindings` labels; help must
  resolve the current settings rather than hardcoding those keys.
  The path is relative to the vessel file (`@../../control_profiles.usda@` one dir deeper,
  `@../../vessels/control_profiles.usda@` from a scene).
- **Override one intent** by redefining that child locally over the reference:
  `def "Controls" (references=…) { def "action" { uniform string lunco:port = "handbrake" } }`.
- **A new control scheme** = new intents in the referenced profile (or authored inline) —
  data, not Rust. The key→intent half is the shared leafwing `UserIntent` map, so a saved
  keymap rebinds every vessel; you only choose what each intent *actuates* here.
- **Make an entity drivable at RUNTIME**: author the `Controls` child (and give it an
  actuation surface) via the USD-op API on the new prim — it composes immediately and the
  possessing avatar can drive it. No Rust, no restart. This is how you "build a new entity
  and teach the avatar to control it."

## The recipe (checklist)

0. Complete the exemplar audit above and identify the actual controller,
   vehicle, scene, and test owner before creating files.
1. Write or reuse the control law as a `.mo` model: sensed inputs → force/torque; `min`/`max`
   clamps; DIRECT control path; a `piloted` gate. Der-feed any tunable gain you want
   Inspector-editable at sim-rate.
2. Reference the sensors it needs from `assets/vessels/sensors/` and mount them.
3. On the vessel prim: apply `LunCoProgramAPI`, name the model
   (`uniform asset info:sourceAsset = @models/MyController.mo@`), promise
   `uniform bool lunco:program:realtimeSafe = true`, author the connections (sensor +
   body ports → model `inputs:`, incl. `inputs:piloted`, and model force/torque → the
   body), and add a `Controls` child that `references` a profile (`</LanderControls>`)
   so the pilot's intents reach the stick ports.
4. Add a `Scope` applying `LunCoProgramAPI` child prim naming a `.rhai` supervisor for events/sequencing
   (no control loop), with connected `LunCoEvent` children for model conditions.
5. Verify: unpossessed → the GNC flies it; possess → the pilot drives (gate flips via
   `piloted`); release → GNC resumes. Tune live via the Inspector or `set()`.

## Anti-patterns

- ❌ Control math in rhai — belongs in Modelica.
- ❌ Per-tick rhai routing / an unconditional self-wire — clobbers the pilot; use the
  `piloted` gate instead.
- ❌ An in-model `manual` flag toggled at runtime — folds unless der-fed; and it's
  per-model. `piloted` is the general, wired, first-class signal.
- ❌ Reading the god-view body pose — read sensors (altimeter, IMU) so it's a real GNC.
- ❌ Magic constants (torque, mass) — wire them from the body's ports (inertia, mass).
- ❌ Starting a new controller from a mission report or tutorial snippet without
  reading the closest shipped production exemplar and its runtime test.
- ❌ Forking a production controller only to rename the vehicle — keep reusable
  equations and put vehicle-specific facts in USD-authorized parameters.
