---
name: compose-multidomain-twin
description: >
  How to assemble a complete multi-domain mission in LunCoSim — a vehicle (or
  fleet) whose geometry, physics, subsystem dynamics (GNC / power / thermal), and
  behaviour all cosimulate together, packaged as a Twin. USE THIS SKILL whenever
  the user asks, in plain words, things like: "build the lander+rover mission",
  "wire this Modelica model to that USD body", "make a spacecraft with a battery
  and thermal model that interact", "set up a full scenario with vehicles +
  behaviour", "package this as a Twin", or "connect the GNC output to the
  thruster". Any request to compose more than one domain (USD + Modelica + cosim
  + rhai) into one running system belongs here. (For the agent mid-code:
  `twin.toml`, a `LunCoProgram` prim per domain, native USD connections,
  `SimConnection` / port wiring, a `lunco:scenario` orchestration prim,
  or `SetPorts`/`SetModelInput` fighting each other.) Project-specific and
  non-obvious: a vehicle is a USD FILE (not a Rust struct), each physical domain
  is its OWN program prim with its own `.mo` wired via ports (SSP), the PortRegistry
  is the SINGLE input-write path (direct `SetModelInput` on a cosim'd entity is
  clobbered every tick), and gravity comes from the environment (don't apply it
  in `.mo`). Design: docs/architecture/33-spacecraft-modeling.md, 34-scenario-and-multidomain.md.
---

# Composing a multi-domain Twin

A full mission layers cleanly — never blur the layers:

| Layer ("…") | Owns | Lives in |
|---|---|---|
| **Structure + wiring** ("what") | bodies, colliders, mass/inertia, joints, topology, program prims, port connections | **USD** (authored) |
| **Subsystem dynamics** ("how a part behaves") | thrust, propellant, battery, thermal, controllers | **Modelica / rhai** (cosim) |
| **Substrate + behavior library** ("the laws") | solver, force/joint/port plumbing, parameterized wheel/suspension/friction | **Rust** (reusable, never bespoke) |

> **Rust ships parameterized behaviors; it never hardcodes a vehicle.** A 6-wheel
> rover is a USD file, not a Rust struct — the physics/materials philosophy applied
> to whole vehicles.

**Who computes what.** Rust owns rigid-body kinematics and dynamics — bodies,
colliders, contacts, joints. Modelica owns everything else that evolves: thermal,
electrical, propulsion, structural. Modelica reaches physics through cosim ports,
and may also carry GNC or flight-software math (an equation is an equation); what it
must never become is a second physics engine. rhai stays logic.

### Physics ports vs sensors — pick the wrong one and you author a bug

| | Physics ports | Sensors |
|---|---|---|
| There because | the body/collider EXISTS | you AUTHORED an instrument in USD |
| Ports | `position_*`, `velocity_*`, `contact`, `contact_force` | `range`, `accel_*`, `spec_force_*`, `contact` |
| Adds | nothing — ground truth | mount offset, range limits, out-of-range mode, noise, failure |
| Wire to | **physical parts** — struts, dampers, structure | **flight software** — GNC, OBC, autopilot |

A physical part reads PHYSICS: a strut's glow takes the `force` port off its own
prismatic joint, because a leg carries load when the ground pushes on it — not when
an instrument says so. Flight software reads SENSORS, because a computer knows only
what its instruments report: `DescentGuidance` reads the altimeter *with* its mount
offset and `rangeMax`, not the true height.

Backwards costs real bugs. The struts were once gated on the altimeter, whose datum
sits 3.3 m above the pads — so a hand-copied `contact_alt` had to restate the
geometry, got it wrong, and lit the legs 3.9 m before touchdown. **When a constant in
a `.mo` exists only to translate between two prims' positions, the wire is wrong.**

**A sensor reads physics; it never re-derives it.** The touchdown switch and the
collider contact ports share one computation (`avian::contact_of`). Two copies are
free to drift, and nothing in the log says which one you are looking at.

**No per-tick computation.** Prefer an on-demand port read over a mirror component
kept in step by a sync system, and `Changed<T>` over an unfiltered system. Never
per-tick work in rhai — except in a rhai *test*, where stepping is the point.

This skill is the *assembly* layer over the single-domain skills:
[`build-usd-scene`](../build-usd-scene/SKILL.md) (author the scene),
[`authoring-vessel-controllers`](../authoring-vessel-controllers/SKILL.md) (a vessel's GNC),
[`author-scenario`](../author-scenario/SKILL.md) (behaviour),
[`run-modelica`](../run-modelica/SKILL.md) (the `.mo` models),
[`inspect-simulation`](../inspect-simulation/SKILL.md) (verify the chain).

## The Twin (on-disk mission unit)

A **Twin** = a folder + a `twin.toml` manifest that owns a default USD scene:

```toml
name = "sandbox"
version = "0.1.0"
description = "…"
[usd]
default_scene = "sandbox_scene.usda"   # loaded as the active stage on open; other .usda here are a referenceable library
```

Everything the mission needs lives under that folder: the scene, referenced
vehicle assets, `.mo` models, `.rhai` scenarios. Open the Twin and the default
scene loads.

## Decision 1 — a multi-domain vehicle is one program prim per domain + connections (SSP)

A program is a PRIM, not an attribute on the thing it drives — the same reason a
`UsdShade` shader is a prim: it has typed ports, and ports connect. Model each physical
domain as its own `def LunCoProgram` under the vehicle Xform, each naming its own `.mo`,
wired through the port surface. This *is* FMI/SSP — no new machinery.

```usda
def Xform "Lander" (PhysicsRigidBodyAPI …)              # rigid body (Avian ports)
{
    float inputs:force_local_y.connect = </Lander/GNC.outputs:thrust>   # GNC thrust → body force

    def LunCoProgram "GNC" {
        uniform asset lunco:program:sourceAsset = @models/LanderGNC.mo@
        uniform bool  lunco:program:realtimeSafe = true                 # it drives a force
        float inputs:altitude.connect     = </Lander.outputs:height>
        float inputs:descent_rate.connect = </Lander.outputs:velocity_y>
        float inputs:engine_enable.connect = </Lander/Power.outputs:soc>
        float inputs:g = 1.62                                           # a parameter is an input with a constant
    }
    def LunCoProgram "Power" {
        uniform asset lunco:program:sourceAsset = @models/Battery.mo@
        float inputs:load.connect = </Lander/GNC.outputs:thrust>
    }
    def LunCoProgram "Therm" {
        uniform asset lunco:program:sourceAsset = @models/ThermalNode.mo@
    }
}
```

A wire is a native USD connection, authored on the prim that CONSUMES the value. **Do
not** host N solvers on one entity — that forces `SimComponent` to a multi-instance map
and touches the propagation core. One program prim per domain gives the same composition
with zero core change and clean per-domain telemetry.

A vessel's OWN flight-control system is the one exception to the child prim: it is not
separable from the airframe (its `inputs:` are what the stick talks to), so the vessel
prim applies `LunCoProgramAPI` in place — see
[`authoring-vessel-controllers`](../authoring-vessel-controllers/SKILL.md).

## Decision 2 — the PortRegistry is the ONE input-write path

`SetModelInput`, `SetPort`, rhai `set(id,name,v)`, Python, and wires **all
converge** on `SimComponent.inputs` for a cosim'd entity — the cosim value *is* the
value everyone reads. `sync_modelica_inputs` copies `SimComponent.inputs →
ModelicaModel.inputs` every tick, so a **direct `ModelicaModel.inputs` write is
clobbered within one frame.** Always write through a port (`SetPorts`, `set_input`,
rhai `set()`), never bypass to the model.

## Decision 3 — the scenario is a first-class USD concept

A *scenario* is the scene that bundles vehicles (referenced assets) + cosim wiring
+ per-vehicle behaviour + **one orchestration script**:

```usda
def Scope "Scenario" ( kind = "component" )
{
    custom string lunco:scenario = "rover-surface-ops"

    def LunCoProgram "Mission" {
        uniform asset lunco:program:sourceAsset = @scenarios/rover_surface_ops.rhai@
        # or author the mission state machine in place:
        # uniform string lunco:program:sourceCode = """ … """
    }
}
```

- **Orchestration** (rhai): phases via the sequencer (`descend → touchdown →
  deploy → handover → task_1…`), advancing on **port-read predicates** (altitude,
  joint presence, SoC, distance, temp).
- **Per-vehicle** scripts: a `def LunCoProgram` child prim on the vehicle for local
  behaviour (flight assist, autonomy helpers) — delete the prim and the behaviour goes
  with it.
- **Objectives / scoring**: rhai predicates over ports — no new engine.

## Recipe

1. **Twin:** create/pick a folder with `twin.toml` (`[usd] default_scene`).
2. **Reference vehicles:** pull authored assets into the scene (e.g.
   `assets/vessels/rovers/{skid,ackermann}_rover.usda`, a lander) — wheel count,
   params, joints, drive type all come from USD; nothing hardcoded.
3. **Add subsystems per vehicle:** a `def LunCoProgram` per domain naming its
   `lunco:program:sourceAsset`; the body carries `PhysicsRigidBodyAPI` + the force
   connections. Reuse existing `.mo` (`models/RocketEngine.mo`, an MSL `LimPID`
   for GNC).
4. **Wire cross-domain ports** with connections on the consumer
   (`inputs:load.connect = </Lander/GNC.outputs:thrust>`,
   `inputs:engine_enable.connect = </Lander/Power.outputs:soc>`, …).
5. **Add the Scenario prim:** a `LunCoProgram` child naming the orchestration script
   (phases + objectives as port predicates), plus a `LunCoProgram` child on each
   vehicle for its own behaviour.
6. **Open + verify:** load the Twin, then use
   [`inspect-simulation`](../inspect-simulation/SKILL.md) — `cosim_status` to see the
   whole Modelica→physics chain, `read_ports` for specific values, a screenshot to
   confirm motion. Iterate on the `.usda`/`.mo`/`.rhai` (all hot-editable).

## Gotchas

- **Don't apply gravity in `.mo`** — `lunco-environment` applies it separately; doing both double-counts.
- **Don't `SetModelInput` directly on a cosim'd entity** — clobbered every tick (Decision 2). Write the port.
- **`set_input(me,…)` is not a rhai verb** — inside a scenario use `set(me, "port", v)` (routes through `write_port`).
- **A vehicle is a USD file** — spawn/param it in USD; if you're writing a Rust struct for a specific rover, stop.
- **Unwired algebraic Modelica inputs fold to their default** — see [`authoring-vessel-controllers`](../authoring-vessel-controllers/SKILL.md) for the `der`-feed / wiring fix.
- **Per-domain identity is the point** — one `LunCoProgram` prim per subsystem gives clean per-domain telemetry; don't collapse them onto the body.
