# 03 — Cosim: when a Model flies physics

> **Pair with:** the *Cosim — Model meets Physics* in-app lesson (sandbox 🎓 menu).
> **Reference:** [Co-Simulation Domain](../architecture/22-domain-cosim.md) ·
> [compose-multidomain-twin](../../skills/compose-multidomain-twin/SKILL.md) ·
> [inspect-simulation](../../skills/inspect-simulation/SKILL.md)

A rover that drives, a lander that descends, a balloon that rises — when the
*behaviour* lives in a [Modelica](https://modelica.org) model and the *motion*
lives in the physics engine, the two run side by side and talk every step. That
is **co-simulation** (cosim). This walkthrough uses the lander asset
`vessels/landers/descent_lander.usda` — a real, working cosim vessel, referenced by
`scenes/sandbox/lander_test.usda` — to show the whole loop, then points at how to
wire your own.

## The pattern in one paragraph

A Modelica model is attached to an entity. Every fixed timestep the model emits
outputs (forces, throttle), a `SimConnection` copies each output to a target
input, Avian integrates the resulting forces, and the new altitude/velocity flow
back into the model. Read outputs → propagate → apply forces → step engines.
That round-trip is the [FMI master algorithm](https://fmi-standard.org/), and it
runs in `FixedUpdate` at one shared `dt` so both engines agree on time.

## Read it off the vessel

The scene only says *where* the lander starts; the cosim chain travels with the
vessel. Open `assets/vessels/landers/descent_lander.usda` and read the root prim.
A model is a PROGRAM, and a program is a prim with typed ports — exactly as a
`UsdShade` shader is a prim. Three things do the work:

```usda
# 1. Name the model. The lander's flight-control system is inseparable from the
#    airframe, so the vessel prim APPLIES `LunCoProgramAPI` and carries it in place
#    (a bolted-on program — a guidance law, a supervisor — is a child prim instead).
uniform asset lunco:program:sourceAsset = @models/Lander.mo@

#    It drives a force on a body the client predicts, so it must promise it steps
#    fast enough to be trusted with one.
uniform bool lunco:program:realtimeSafe = true

# 2. Wire it. A wire is a native USD connection, authored on the prim that CONSUMES
#    the value, and derived into a SimConnection — the FMI/SSP pattern.
float inputs:force_y.connect = </DescentLander.outputs:force_y>
float inputs:q_w.connect     = </DescentLander.outputs:quat_w>
#    (…see the asset for the full set of edges.)

# 3. Surface threshold crossings on the bus — one `LunCoPortEvent` child prim per
#    rule: the port, the comparison, the threshold and the name, each typed.
def LunCoPortEvent "LowFuel" {
    uniform token lunco:event:port = "m_prop"
    uniform token lunco:event:op = "lt"
    double lunco:event:threshold = 200.0
    uniform token lunco:event:emit = "lander_low_fuel"
}
```

`Lander.mo` holds the guidance law and exposes command inputs (`throttle`,
`pitch`, `roll`, `yaw`) plus a `piloted` gate that yields control to whoever
possesses the vessel. The forces the model computes land on the body as
`PendingForces`; Avian applies them. No script steps the loop — it is pure
wiring.

## Watch it from the side

A small supervisor script (`assets/scenarios/lander_subsystems.rhai`) rides on the
same vessel as a `def LunCoProgram "Subsystems"` child prim — bolted on, so deleting
the prim removes it. It does **not** drive the lander — it only reacts, which is the
right shape for cosim orchestration:

```rhai
fn on_event(me, evt) {
    // The fuel events, from the `LunCoPortEvent` prims above.
    if evt.name == "lander_low_fuel"  { notify_kind("Lander low on fuel.", "warn"); }
    else if evt.name == "lander_depleted" { notify_kind("Propellant depleted.", "warn"); }
}
```

It also reads live state the cosim loop produces — `get(ent, "range")` from an
altimeter, `get(me, "velocity_y")` from the body — to detect touchdown. Those are
the same ports you can read over the API.

## Verify the chain is live

Don't trust the picture — read the ports. Over the HTTP API
(`--api 3000`, see [the API doc](../architecture/12-api.md)):

- `cosim_status` — snapshots **every** cosim entity and its live Modelica
  variables (`y`, `vy`, `netForce`, force inputs, buoyancy, …). One call tells
  you the model is stepping and what it computes.
- `read_ports` / `watch_ports` — read one named port, or sample it as a
  time-series to watch a signal evolve (SOC draining, propellant burning).

If `cosim_status` lists the entity and its variables are changing, the
Modelica→physics chain is real.

## Your turn: a battery on a rover

The bundled `assets/models/Battery.mo` is a state-of-charge integrator:

```modelica
input  Real current_in "Raw input current in Amperes";
output Real soc_out;
output Real voltage_out;
equation
  der(soc) = -current / (capacity * 3600.0);   // charge balance
```

To cosim it onto a rover:

1. **Attach** — add a `def LunCoProgram "Power"` child prim on the rover with
   `uniform asset lunco:program:sourceAsset = @models/Battery.mo@`. It is a subsystem
   bolted on, not the rover's own control law, so it is a child prim.
2. **Wire** — connect the rover's motor current (or a proxy proportional to throttle)
   into the program's `inputs:current_in`, and read `outputs:soc_out` from wherever you
   want to observe it.
3. **Observe** — `watch_ports` on `soc_out` while you drive; it falls as current
   flows.

> **Status note:** the `Battery.mo` model ships, but no scene currently wires it
> to a rover's current draw — so this last section is the exercise, not a
> pre-baked example. The lander above is the proven, runnable reference.

## Where models live and how to edit them

The full Modelica IDE is embedded in the sandbox as the **Design workspace** —
open `Lander.mo`, `Battery.mo`, or any [Modelica Standard Library](https://github.com/modelica/ModelicaStandardLibrary)
class, edit the source or the diagram, compile, and run with live plots. That is
the same workbench the standalone *lunica* app provides; in the sandbox it sits
alongside the 3D scene so a model and the physics it drives can be open at once.

See [`../architecture/22-domain-cosim.md`](../architecture/22-domain-cosim.md)
for the master-loop schedule and the `SimConnection` / `PortRegistry` contract.
