# Tutorial 02 — Author your own vessel controller

Build a self-flying vessel from scratch, the LunCoSim way: the **control law in
Modelica**, **logic in rhai**, **structure + authority in USD** — and a human (or an
autopilot) that can take over. By the end you'll understand every layer of the
lander's GNC and be able to write your own.

> Reference (dense): [`skills/authoring-vessel-controllers`](../../skills/authoring-vessel-controllers/SKILL.md).
> Worked example: `assets/models/Lander.mo` + `scenarios/lander_subsystems.rhai` +
> `vessels/landers/descent_lander.usda` (the vessel), dropped into a mission by
> `scenes/sandbox/lander_ops.usda` (the scene).

We'll build **`Hover.mo`** — a thruster that holds a target altitude and yields to a
pilot. Small, but it exercises the whole stack.

## Step 1 — the control law (Modelica)

Create `assets/models/Hover.mo`. The model reads a **sensed** altitude and vertical
velocity, computes a hover thrust, and yields to a pilot on the `piloted` gate:

```modelica
model Hover
  parameter Real max_thrust = 20000.0;
  input Real vehicle_mass = 500.0;     // wired from the body `mass`
  input Real g = 1.62;
  input Real target_alt = 10.0;
  input Real altitude = 0.0;           // SENSED (wired, Step 3)
  input Real climb_rate = 0.0;         // body velocity_y (wired)
  input Real piloted = 0.0;            // authority gate (wired, Step 4)
  input Real external_throttle = 0.0;  // pilot stick
  output Real force_y, throttle;
  Real gnc, cmd, filt(start = 0.0);
equation
  // GNC law: feed-forward hover + PD to the set-point. DIRECT (no spool).
  gnc = min(max((g + 1.5*(target_alt - altitude) - 2.0*climb_rate) * vehicle_mass
               / max_thrust, 0.0), 1.0);
  der(filt) = (external_throttle - filt) / 0.3;      // spool the pilot stick (keeps it live)
  cmd = piloted*filt + (1.0 - piloted)*gnc;          // yield-to-pilot gate, branch-free
  force_y = cmd * max_thrust;
  throttle = cmd;
end Hover;
```

Why it's shaped this way: `min`/`max` (not `if`) for rumoca-safe clamps; the GNC path
is direct so its braking isn't laggy; the pilot stick feeds a `der` so it stays a live
input; `piloted` selects between them. (See the skill for the *input-folding* rule —
the #1 gotcha.)

## Step 2 — the supervisor (rhai, events only)

Create `assets/scenarios/hover_super.rhai`. **No control loop** — just react to events:

```rhai
fn on_event(me, evt) {
    if evt.name == "low_fuel" { notify_kind("Hover: low fuel", "warn"); }
}
```

(A real controller sequences phases here with `wait_for`/`wait_until` or reacts to
events raised by `LunCoPortEvent` prims. Never write command ports every tick from rhai.)

## Step 3 — sensors + wiring (USD)

On your vessel prim, mount a referenced altimeter and wire sensors → model inputs, and
model force → body. Mass comes from the body's own port (no magic number):

```usda
def "Altimeter" (prepend references = @../../vessels/sensors/altimeter.usda@</Altimeter>)
{ double3 xformOp:translate = (0, -1, 0); uniform token[] xformOpOrder = ["xformOp:translate"] }

# on the vessel prim — its flight-control system is inseparable from the airframe, so
# name the model in place, on the prim itself:
uniform asset info:sourceAsset = @models/Hover.mo@
uniform bool  lunco:program:realtimeSafe = true      # it drives a force on a predicted body

# a wire is a native USD connection, authored on the consumer:
float inputs:force_y.connect      = </Vessel.outputs:force_y>
float inputs:climb_rate.connect   = </Vessel.outputs:velocity_y>
float inputs:vehicle_mass.connect = </Vessel.outputs:mass>
float inputs:piloted.connect      = </Vessel.outputs:piloted>
float inputs:throttle.connect     = </Vessel.outputs:throttle>

# the supervisor is bolted on, so it is a child program prim:
def LunCoProgram "Supervisor" {
    uniform asset info:sourceAsset = @scenarios/hover_super.rhai@
}
```

Feed `altitude` from the altimeter with a cross-prim connection — the same form, the
target path just names another prim:
`float inputs:altitude.connect = </Vessel/Altimeter.outputs:range>`.

## Step 4 — control authority (already done!)

You connected `inputs:piloted` in Step 3 — that's the whole authority mechanism. The
`piloted` port is `1.0` whenever a session (a user **or** an autopilot) possesses the
vessel, derived from the possession registry (`PILOTED_BACKEND`). Add a pilot
intent→port binding — a `Controls` child that `references` a profile — so the stick
reaches `external_throttle` when possessed:

```usda
# on the vessel prim:
def "Controls" (prepend references = @../../vessels/control_profiles.usda@</LanderControls>) {}
```

(Deliver it as this child `references` arc, not root `subLayers` + `inherits` — only the
child arc composes when the vessel is spawned/referenced. See the
[skill](../../skills/authoring-vessel-controllers/SKILL.md) for the full `Controls` scope.)

Nothing else to write: **unpossessed → the GNC hovers; possess → the pilot flies;
release → the GNC resumes.**

## Step 5 — run & tune

Load the scene, watch it hover. Possess it (click / F) and throttle — you fly it.
Release — it holds again. Open the Inspector during the run and drag `target_alt` (make
it a der-fed live input if you want it editable at sim-rate — see the skill).

## What you learned

- **Three languages, three jobs**: math→Modelica, events→rhai, structure/authority→USD.
- **`piloted`** is the one, general control-authority signal — never a bespoke gate.
- **Sensors, not god-view**; **wire constants** from the body; **keep inputs live**.

Next: read the [lander GNC](../../assets/models/Lander.mo) — it's this same pattern with
a velocity-scheduled descent, IMU attitude, and τ=I·α torque wired from inertia.

## Related

- **Previous walkthrough**: [01 — Lander → Rover mission](01-lander-rover-mission.md) — the full mission this controller flies in.
- **Reference skills**: [authoring-vessel-controllers](../../skills/authoring-vessel-controllers/SKILL.md) (dense reference for this stack),
  [author-scenario](../../skills/author-scenario/SKILL.md) (the rhai supervisor layer),
  [compose-multidomain-twin](../../skills/compose-multidomain-twin/SKILL.md) (dropping the vessel into a mission). Full index: [skills/](../../skills/README.md).
