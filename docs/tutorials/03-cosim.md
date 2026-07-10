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
Three attributes do the work:

```usda
# 1. Attach the model. This is what turns a physics body into a cosim vessel.
string lunco:modelicaModel = "models/Lander.mo"

# 2. Wire it. Native USD connectionPaths are derived into a SimConnection
#    (source output port -> target input port), the FMI/SSP pattern.
#    (connectionPaths block on the prim — see the scene for the exact edges.)

# 3. Surface Modelica `when` events on the bus. "<condition>:<event-name>".
custom string lunco:portEvents = "m_prop<200:lander_low_fuel, m_prop<=0.5:lander_depleted"
```

`Lander.mo` holds the guidance law and exposes command inputs (`throttle`,
`pitch`, `roll`, `yaw`) plus a `piloted` gate that yields control to whoever
possesses the vessel. The forces the model computes land on the body as
`PendingForces`; Avian applies them. No script steps the loop — it is pure
wiring.

## Watch it from the side

A small supervisor script (`assets/scenarios/lander_subsystems.rhai`) is attached
to the same prim via `lunco:scriptPath`. It does **not** drive the lander — it
only reacts, which is the right shape for cosim orchestration:

```rhai
fn on_event(me, evt) {
    // Native Modelica fuel `when` events, from `lunco:portEvents` above.
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

1. **Attach** — put `lunco:modelicaModel = "models/Battery.mo"` on the rover prim.
2. **Wire** — add a `connectionPaths` edge from the rover's motor current (or a
   proxy proportional to throttle) into the model's `current_in`, and from
   `soc_out` back out to a port you want to observe.
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
