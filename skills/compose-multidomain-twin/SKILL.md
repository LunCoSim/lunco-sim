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
  `twin.toml`, `lunco:modelicaModel`, `lunco:simWires`, a sub-prim-per-domain
  layout, `SimConnection` / port wiring, a `lunco:scenario` orchestration prim,
  or `SetPorts`/`SetModelInput` fighting each other.) Project-specific and
  non-obvious: a vehicle is a USD FILE (not a Rust struct), each physical domain
  is its OWN sub-prim with its own `.mo` wired via ports (SSP), the PortRegistry
  is the SINGLE input-write path (direct `SetModelInput` on a cosim'd entity is
  clobbered every tick), and gravity comes from the environment (don't apply it
  in `.mo`). Design: docs/architecture/33-spacecraft-modeling.md, 34-scenario-and-multidomain.md.
---

# Composing a multi-domain Twin

A full mission layers cleanly — never blur the layers:

| Layer ("…") | Owns | Lives in |
|---|---|---|
| **Structure + wiring** ("what") | bodies, colliders, mass/inertia, joints, topology, model bindings, cosim wires | **USD** (authored) |
| **Subsystem dynamics** ("how a part behaves") | thrust, propellant, battery, thermal, controllers | **Modelica / rhai** (cosim) |
| **Substrate + behavior library** ("the laws") | solver, force/joint/port plumbing, parameterized wheel/suspension/friction | **Rust** (reusable, never bespoke) |

> **Rust ships parameterized behaviors; it never hardcodes a vehicle.** A 6-wheel
> rover is a USD file, not a Rust struct — the physics/materials philosophy applied
> to whole vehicles.

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

## Decision 1 — a multi-domain vehicle is sub-prim-per-model + wires (SSP)

Model each physical domain as its **own prim** under the vehicle Xform, each with
its own `.mo`, wired through the port surface. This *is* FMI/SSP — no new machinery.

```usda
def Xform "Lander" (PhysicsRigidBodyAPI …)              # rigid body (Avian ports)
{
    string lunco:simWires = "thrust:force_local_y"      # GNC thrust → body force
    def Scope "GNC"   { string lunco:modelicaModel = "models/LanderGNC.mo"
                        string lunco:simWires = "height:altitude,velocity_y:descent_rate,gravity_accel:g" }
    def Scope "Power" { string lunco:modelicaModel = "models/Battery.mo" }
    def Scope "Therm" { string lunco:modelicaModel = "models/ThermalNode.mo" }
    # wire prims connect GNC.thrust→Power.load, Power.soc→GNC.engine_enable, …
}
```

`lunco:simWires` is `"provider:consumer,…"` (a source port → a model input). **Do
not** host N solvers on one entity — that forces `SimComponent` to a multi-instance
map and touches the propagation core. Sub-prim-per-model gives the same composition
with zero core change and clean per-domain telemetry.

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
    custom string lunco:script = """ … mission state machine + objectives … """
}
```

- **Orchestration** (rhai): phases via the sequencer (`descend → touchdown →
  deploy → handover → task_1…`), advancing on **port-read predicates** (altitude,
  joint presence, SoC, distance, temp).
- **Per-vehicle** scripts (`lunco:scriptPath`): local behaviour (flight assist,
  autonomy helpers).
- **Objectives / scoring**: rhai predicates over ports — no new engine.

## Recipe

1. **Twin:** create/pick a folder with `twin.toml` (`[usd] default_scene`).
2. **Reference vehicles:** pull authored assets into the scene (e.g.
   `assets/vessels/rovers/{skid,ackermann}_rover.usda`, a lander) — wheel count,
   params, joints, drive type all come from USD; nothing hardcoded.
3. **Add subsystems per vehicle:** a `def Scope` per domain with
   `lunco:modelicaModel` + `lunco:simWires`; the body carries `PhysicsRigidBodyAPI`
   + force wires. Reuse existing `.mo` (`models/RocketEngine.mo`, an MSL `LimPID`
   for GNC).
4. **Wire cross-domain ports** in `simWires` (GNC.thrust→Power.load,
   Power.soc→GNC.engine_enable, …).
5. **Add the Scenario prim:** an orchestration `lunco:script` (phases + objectives
   as port predicates) + per-vehicle `lunco:scriptPath`.
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
- **Per-domain identity is the point** — one `def Scope` per subsystem gives clean per-domain telemetry; don't collapse them onto the body.
