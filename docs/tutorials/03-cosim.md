# 03 — Cosim: when a Model flies physics

> **Pair with:** the *Cosim — Model meets Physics* in-app lesson (luncosim 🎓 menu).
> **Reference:** [Co-Simulation Domain](../architecture/22-domain-cosim.md) ·
> [compose-multidomain-twin](../../skills/compose-multidomain-twin/SKILL.md) ·
> [inspect-simulation](../../skills/inspect-simulation/SKILL.md)

A rover that drives, a lander that descends, a balloon that rises — when the
*behaviour* lives in a [Modelica](https://modelica.org) model and the *motion*
lives in the physics engine, the two run side by side and exchange values at
declared communication points. That is **co-simulation** (cosim). This walkthrough uses the lander asset
`vessels/landers/descent_lander.usda` — a real, working cosim vessel, referenced by
`scenes/luncosim/lander_ops.usda` — to show the whole loop, then points at how to
wire your own.

## The pattern in one paragraph

A Modelica model is attached to an entity. The fixed master clock advances the
world; at each participant's declared communication point, model outputs
propagate through `SimConnection`s, Avian applies the resulting forces, and the
new state becomes the next model input sample. Outputs are held between points.
The causal ordering is shaped like the [FMI master algorithm](https://fmi-standard.org/),
but this runtime is not itself an FMI implementation and not every participant
is forced to communicate at the physics tick rate.

## Read it off the vessel

The scene only says *where* the lander starts; the cosim chain travels with the
vessel. Open `assets/vessels/landers/descent_lander.usda` and read the root prim.
A model is a PROGRAM, and a program is a prim with typed ports — exactly as a
`UsdShade` shader is a prim. Three things do the work:

```usda
# 1. Name the model. The lander's flight-control system is inseparable from the
#    airframe, so the vessel prim authors the `info:*` properties on ITSELF
#    (a bolted-on program — a guidance law, a supervisor — is a child prim instead).
uniform asset info:sourceAsset = @lunco://models/Lander.mo@

#    It drives a force on a body the client predicts, so it must promise it steps
#    fast enough to be trusted with one.
uniform bool lunco:program:realtimeSafe = true

# 2. Wire it. A wire is a native USD connection, authored on the prim that CONSUMES
#    the value, and derived into a SimConnection — the FMI/SSP pattern.
float inputs:force_y.connect = </DescentLander.outputs:force_y>
float inputs:q_w.connect     = </DescentLander.outputs:quat_w>
#    (…see the asset for the full set of edges.)

# 3. Modelica owns the condition; USD connects its 0/1 output to the event bus.
def LunCoEvent "LowFuel" {
    float inputs:trigger.connect = </DescentLander/MainPropulsion.outputs:low_fuel>
    uniform token lunco:event:name = "lander_low_fuel"
    uniform token lunco:event:severity = "warning"
}
```

`Lander.mo` holds the guidance law and exposes command inputs (`throttle`,
`pitch`, `roll`, `yaw`) plus a `piloted` gate that yields control to whoever
possesses the vessel. The forces the model computes land on the body as
`PendingForces`; Avian applies them. No script steps the loop — it is pure
wiring.

## Watch it from the side

A small supervisor script (`assets/scenarios/lander_subsystems.rhai`) rides on the
same vessel as a `def Scope "Subsystems" (prepend apiSchemas = ["LunCoProgramAPI"])` child prim — bolted on, so deleting
the prim removes it. It does **not** drive the lander — it only reacts, which is the
right shape for cosim orchestration:

The child is the authored attachment point, while the runtime passes the owning
vessel as `me`. That is why the supervisor resolves `/GNC` from `name(me)`; walking
to `parent(me)` would address the scene above the vessel and lose the typed latch
handoff.

```rhai
fn on_event(me, evt) {
    // The fuel events, from the connected `LunCoEvent` prims above.
    if evt.name == "lander_low_fuel"  { notify_kind("Lander low on fuel.", "warn"); }
    else if evt.name == "lander_depleted" { notify_kind("Propellant depleted.", "warn"); }
}
```

Landing transitions use the same explicit projection. The scene wires the native
`engine_cutoff_contact` predicate into the GNC, and wires the GNC's accepted
`landing_engine_cutoff`/`landing_handoff` modes back into `Lander.mo`. The GNC then
publishes `landing_engine_cutoff_request` and `landing_handoff_request`, while
`Lander.mo` publishes the settled `touchdown` predicate; USD maps them to the
latched `lander_engine_cutoff`, `lander_flight_handoff`, and `lander_touchdown`
events. A mission joins those three source-qualified events before releasing a
payload. The script contains no polling, threshold math, or second touchdown state
owner.

## Verify the chain is live

Don't trust the picture — read the ports. Over the HTTP API
(`--api 4101`, see [the API doc](../architecture/12-api.md)):

- `CosimStatus` — snapshots the co-simulation graph and live Modelica variables.
- `ReadPorts` — reads one entity's typed named ports.
- `GetBrokenConnections` — reports terminal and pending wiring diagnostics.

If `CosimStatus` lists the entity and its variables are changing, the
Modelica→physics chain is real.

## Your turn: a battery on a rover

The bundled `assets/models/LunCo/Electrical/Battery.mo` sets the bus voltage and
integrates its own charge from whatever current flows through its pin:

```modelica
within LunCo.Electrical;
model Battery
  Pin p;
  Real soc(start = soc_init) "State of charge, 0..1";
  output Real soc_out;
equation
  p.v = voltage_nom * (0.8 + 0.2 * soc) + p.i * R_internal;   // droops with SoC and ESR
  der(soc) = p.i / (capacity * 3600.0);                       // charge balance
  soc_out = soc;
end Battery;
```

**Note what it is *not*.** There is no `input Real current_in`. `Pin` carries a
`flow` variable, so the current is decided by the circuit — every load and source
on the bus at once — not written in by a caller. Nobody sums the draw; Kirchhoff
does. That is the whole reason the electrical domain is acausal (see
[54 — The Electrical Domain](../architecture/54-electrical-domain-and-modelica-libraries.md)).

To cosim it onto a rover:

1. **Attach** — add a `def Scope "Power" (prepend apiSchemas = ["LunCoProgramAPI"])`
   child prim on the rover with
   `uniform asset info:sourceAsset = @lunco://models/LunCo/Electrical/Battery.mo@`.
   It is a subsystem bolted on, not the rover's own control law, so it is a child
   prim — and a shipped asset is addressed `@lunco://…@`, never by a bare relative
   path.
2. **Wire the circuit, not a number** — the battery, the PDU and each load are
   `connect()`-ed pin to pin inside one Modelica model. A rover motor pulling
   current is a load on that bus; adding a second load changes the first one's
   share without editing either.
3. **Observe** — poll `ReadPorts` for `soc_out` while you drive; it falls as the bus
   draws.

> **Status note:** the model ships and `assets/scenes/tests/lint_selftest.usda`
> references it, but no gameplay scene currently wires a battery to a rover's
> real current draw — so this last section is the exercise, not a pre-baked
> example. The lander above is the proven, runnable reference.

## Where models live and how to edit them

The full Modelica IDE is embedded in the luncosim as the **Design workspace** —
open `Lander.mo`, `LunCo/Electrical/Battery.mo`, or any [Modelica Standard Library](https://github.com/modelica/ModelicaStandardLibrary)
class, edit the source or the diagram, compile, and run with live plots. That is
the same workbench the standalone *lunica* app provides; in the luncosim it sits
alongside the 3D scene so a model and the physics it drives can be open at once.

See [`../architecture/22-domain-cosim.md`](../architecture/22-domain-cosim.md)
for the master-loop schedule and the `SimConnection` / `PortRegistry` contract.
