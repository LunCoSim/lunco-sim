---
name: authoring-vessel-controllers
description: >
  How to model a vehicle's behaviour in LunCoSim — making a spacecraft,
  lander, rover, or drone move, fly, drive, or land under its own control,
  and letting a person take over. USE THIS SKILL whenever the user asks, in
  plain words, things like: "how do I model this lander / rover?", "how do
  I make it fly (or drive, land, hover) itself?", "how do I add an
  autopilot / a control system / guidance / a GNC?", "how do I make it
  follow waypoints?", "how do I let the user take control?" or "why doesn't
  my controller / thruster respond?". Any request to model how a vehicle
  behaves under power, or to add/fix its self-driving or manual control,
  belongs here — the user will NOT know the internal terms. (For the agent
  mid-code, it also covers: a `.mo` control model, `lunco:simWires`, the
  `piloted` port, `external_throttle`, `possess`/`follow`, or a rumoca
  input that `set()` writes but that has no effect — and catch-yourself
  moments like putting control math in rhai, a bespoke mode flag, a
  self-wire, or reading the god-view pose instead of a sensor.) These rules
  are project-specific: a naive approach silently FOLDS unwired Modelica
  inputs (writes vanish) and CLOBBERS the pilot; the three-layer split
  (math→Modelica, logic/events→rhai, structure/authority→USD) and the
  wired `piloted` authority signal are not obvious from Modelica/Bevy
  alone. Reference impl: the lander (models/Lander.mo,
  scenarios/lander_subsystems.rhai, scenes/sandbox/lander_test.usda).
---

# Authoring vessel controllers

A vessel that drives itself (a GNC, an autopilot) is built from **three layers,
each in the language that fits it**. Never blur them.

| Layer | Language | Owns | Rule |
|---|---|---|---|
| **Control LAW** | **Modelica** (`.mo`) | the math: PID, schedules, mixing, τ=I·α | ALL control math lives here. Never compute a control law in rhai. |
| **Logic / sequencing** | **rhai** (`.rhai`) | phases, events, mission steps, reactions | EVENT-DRIVEN only. No per-tick control loops, no time-stepping. |
| **Structure / wiring / authority** | **USD** (`.usda`) | sensors, wires, possession, identity | Declarative. Sensors are referenced library prims; control is `simWires`. |

The reference is the lander: `assets/models/Lander.mo`,
`assets/scenarios/lander_subsystems.rhai`, `assets/scenes/sandbox/lander_test.usda`.

## 1. The control law → a Modelica model

The model reads what the vessel **senses** and outputs force/torque. It is a
`SimComponent` bound by `lunco:modelicaModel` and self-wired by `lunco:simWires`.

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

A `lunco:scriptPath` scenario on the vessel does supervision and sequencing —
**never a control loop**. React to events; don't poll or step.

```rhai
fn on_event(me, evt) {
    if evt.name == "lander_touchdown" { /* advance the mission */ }
    if evt.name == "low_fuel" { notify_kind("Low fuel", "warn"); }
}
```

- Phase timing comes from the mission sequencer (`wait`, `wait_for`, `wait_until`) or
  `lunco:portEvents` (Modelica threshold crossings → bus events), not `dt` counting.
- **Do not** write the vessel's command ports every tick from rhai. If you're tempted
  to, the logic belongs in the model (math) or the wiring (authority).

## 3. Sensors → USD library primitives, wired

The controller reads SENSORS, not the god-view body. Sensors are reusable prims in
`assets/vessels/sensors/` (`imu.usda`, `altimeter.usda`), referenced + mounted:

```usda
def "Altimeter" (prepend references = @../../vessels/sensors/altimeter.usda@</Altimeter>)
{ double3 xformOp:translate = (0, -3.3, 0); uniform token[] xformOpOrder = ["xformOp:translate"] }
```

Wire sensor outputs → model inputs in `lunco:simWires` (`provider:consumer`), e.g.
`velocity_y:descent_rate`. Wired inputs are live (they reach the solver). Physical
constants a model needs (mass, inertia) come from the body's own ports
(`mass:vehicle_mass`, `inertia_xx:inertia_xx`) — USD-derived, not magic numbers.

## 4. Control authority → the `piloted` signal + possession

**This is the key pattern. Do not build a bespoke gate.**

- **The GNC is INTERNAL** (part of the model). **A user and an autopilot are both
  external SESSIONS** that *possess* the vessel; user-vs-autopilot is arbitrated by
  possession + RBAC (`may_take_control`), which already exists.
- The internal controller **yields** to whoever possesses via the **`piloted`** port:
  a read-only cosim port (`PILOTED_BACKEND`, `lunco-cosim/src/ports.rs`) that is `1.0`
  when any session owns the vessel (`SessionRegistry::owner_of(...).is_some()`), else `0`.
- Wire it (`piloted:piloted`) into the model and gate on it:
  `cmd = piloted ? pilot_stick : gnc`. Because it's wired it's a live input — **no
  in-model flag, no rhai toggle, no per-tick check.** Possession is the single source
  of truth; Rust never reasons about "autopilot" vs "user".
- The pilot's stick reaches `external_throttle`/`pitch`/… through the normal control
  binding (`_LanderControl` profile: intent→port) when they possess. Camera-follow
  without taking control: `follow(entity)` (inserts a chase camera, no `ControllerLink`).

## The recipe (checklist)

1. Write the control law as a `.mo` model: sensed inputs → force/torque; `min`/`max`
   clamps; DIRECT control path; a `piloted` gate. Der-feed any tunable gain you want
   Inspector-editable at sim-rate.
2. Reference the sensors it needs from `assets/vessels/sensors/` and mount them.
3. In the vessel prim: `lunco:modelicaModel`, `lunco:simWires` (sensor+body ports →
   model inputs, incl. `piloted:piloted`, and model force/torque → body), and a
   `_LanderControl`-style intent→port binding for the pilot.
4. Add a `lunco:scriptPath` rhai supervisor for events/sequencing (no control loop).
5. Verify: unpossessed → the GNC flies it; possess → the pilot drives (gate flips via
   `piloted`); release → GNC resumes. Tune live via the Inspector or `set()`.

## Anti-patterns (all cost us real time this codebase)

- ❌ Control math in rhai — belongs in Modelica.
- ❌ Per-tick rhai routing / an unconditional self-wire — clobbers the pilot; use the
  `piloted` gate instead.
- ❌ An in-model `manual` flag toggled at runtime — folds unless der-fed; and it's
  per-model. `piloted` is the general, wired, first-class signal.
- ❌ Reading the god-view body pose — read sensors (altimeter, IMU) so it's a real GNC.
- ❌ Magic constants (torque, mass) — wire them from the body's ports (inertia, mass).
