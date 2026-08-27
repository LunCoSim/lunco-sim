# 34 — Scenarios & Multi-Domain Vehicles

> Status: Draft · Audience: contributors building the rover-mission scenario
>
> How to compose a vehicle out of several domain models (GNC, power, thermal,
> comms) wired together SSP-style, drive a multi-phase mission from rhai, author
> the whole thing as a USD "scenario", and collapse the input-write paths onto
> one canonical port surface. Extends [22-domain-cosim](22-domain-cosim.md),
> [20-domain-modelica](20-domain-modelica.md), [21-domain-usd](21-domain-usd.md),
> [33-spacecraft-modeling](33-spacecraft-modeling.md).

## The target mission (driving use case)

Animated lander descent → rover deployment (joint detach + fall) → control
hands to the rover → progressively harder player tasks that exercise **energy**,
**bandwidth**, and **thermal** budgets (drive to a rock, take a photo, …).

## What already exists (author-time, not core work)

| Capability | Mechanism | Status |
|---|---|---|
| One program per prim | a `LunCoProgramAPI` prim with one selected `info:*` implementation arm → `SimComponent`; file-backed sources dispatch by extension | ✅ |
| Wiring (SSP Connection) | a native USD connection on the consumer — `inputs:x.connect = </Other.outputs:y>` → `SimConnection`; same form within a prim and across prims | ✅ |
| Gravity from environment | env publishes `gravity_accel` output (`GRAVITY_SOURCE_CONNECTOR`); the model connects `inputs:g` to it | ✅ |
| Many scripts in one world | each owner may have one executable Rhai `LunCoProgramAPI`; different owners remain independent | ✅; duplicate executable programs on one owner are rejected visibly |
| Task state machines | rhai `seq`/`par_all`/`par_race`/`repeat` sequencer + `fn task(me)` | ✅ |
| Connector/`connect()` Modelica | rumoca flattens `RC_Circuit.mo`, `CascadedRCFilter.mo` | ✅ (verify MSL `LimPID` specifically) |
| Live input retune (no recompile) | port write changes `input Real` next step | ✅ (must be a model **input**, not a `parameter`) |
| Named trigger zones (geofence events) | `lunco:triggerZone="name"` → overlap-only Sensor → `enter:/exit:<name>` events | ✅ |
| Model events | Modelica condition output → `LunCoEvent.inputs:trigger.connect` → named telemetry event on its rising edge | ✅ (condition and hysteresis remain solver-owned) |
| Per-instance program config | one typed attribute per key on the program prim — `custom float lunco:param:wmax = 1.05` → rhai `param(me,k,default)` | ✅ (the right answer instead of `name(me)` matching) |
| Emitter identity on events | `TelemetryEvent.source` (sensor/script gid); `wait_for_from(name, src)`, `evt.source` | ✅ |
| On-screen notifications | `ShowNotification` command + rhai `notify`/`notify_kind` + ui overlay | ✅ |
| Native/foreign event → script bus | `App::project_events::<E>(…)`; e.g. keyboard → `key:<KeyCode>` events | ✅ (input wired; network projector pending) |
| Throttle-driven engine plume | WGSL shader on a fixed bounding cone (`shaders/plume.wgsl`), `inputs:throttle.connect` on the gprim; its light from `LunCo.Propulsion.PlumePhotometry` onto scene-property ports | ✅ (no script — the visual is wired, not animated) |

> **Authoring walkthrough:** [`../tutorials/01-lander-rover-mission.md`](../tutorials/01-lander-rover-mission.md)
> builds this entire mission from scratch in USD + rhai + Modelica, exercising
> every mechanism in this table.

**Conclusion:** "several models / several scripts in the world" needs **no core
change** — it is the SSP one-program-prim-per-domain pattern below. The current
runtime has one `ScriptedModel` slot per ECS owner. If two executable Rhai
program prims target the same owner, USD projection reports the ambiguity and
attaches neither; authors must split ownership or compose the policy into one
program. This prevents order-dependent last-writer-wins behavior.

## Decision 1 — Multi-domain vehicle = one program prim per domain + connections (SSP)

A program is a PRIM with typed ports, and ports connect — the same shape `UsdShade`
gives a shader. Model each physical domain as its **own** `LunCoProgramAPI` under the
vehicle Xform, each naming its own `.mo`, wired through the port surface. This *is* the
FMI/SSP system structure and needs nothing new.

```
def Xform "Lander" (PhysicsRigidBodyAPI …)        # the rigid body (avian ports)
{
    float inputs:force_local_y.connect = </Lander/GNC.outputs:thrust>   # GNC thrust → body

    def Xform "Battery" (
        prepend references = @lunco://components/power/battery.usda@</Battery>
    ) {}

    def Scope "GNC" (prepend apiSchemas = ["LunCoProgramAPI"]) {
        uniform asset info:sourceAsset = @lunco://models/DescentGuidance.mo@
        uniform bool  lunco:program:realtimeSafe = true                 # it drives a force
        float inputs:altitude.connect      = </Lander.outputs:position_y
        float inputs:descent_rate.connect  = </Lander.outputs:velocity_y>
        float inputs:g.connect             = </Environment.outputs:gravity_accel>
        # Consume the battery output at its authored USD path.
        float inputs:engine_enable.connect = </Lander/Battery.outputs:soc_out>
    }
    def Scope "Therm" (prepend apiSchemas = ["LunCoProgramAPI"]) {
        uniform asset info:sourceAsset = @lunco://models/LunCo/Thermal/ThermalMass.mo@
    }
}
```

**Reject "N models on one entity" as the default.** `SimComponent` is one
input/output map; hosting multiple solvers on a single entity would force it to a
keyed multi-instance map and touch the propagation core. One program prim per domain
gives the same composition with zero core change and clean per-domain
identity/telemetry. Revisit only if a need appears that program prims genuinely can't
express.

(The one prim that carries a program *in place*, authoring `info:*` on itself, is a vessel
whose flight-control system is its airframe — its `inputs:` are the control surface the
stick writes.)

### Domain coupling: causal across, acausal within

**One network root per solver graph; cross-domain coupling is causal; acausal
(`connectors:`) stays inside a domain.**

A network root with `CollectionAPI:components` compiles to ONE generated composite
Modelica root. The synthesizer emits one child unit per connected graph component
inside that root. Acausal `connectors:<name>.connect` edges become
`connect(a.<name>, b.<name>)` inside one child unit — they cannot span two generated
units. So a wire between
an electrical `Pin` and a thermal `HeatPort` must never be acausal: it would
either fail to compose (different names) or compile to an invalid
`connect(Pin, HeatPort)`. Cross-domain coupling is therefore always a causal
`inputs:`/`outputs:` wire, routed at runtime as a `SimConnection` — exactly the
FMI/SSP scalar-exchange contract.

This is why the rover's electrical network is owned by the rover root while the thermal domain is a separate scope:
- The rover root — Battery + motors, acausal `connectors:p` (Kirchhoff current)
  solved together. Its members retain ownership of their solved outputs.
- `Scope "Thermal"` — heat loads + masses + radiators, acausal `connectors:port`
  (heat balance) solved together; consumes `inputs:motor_heat_*`. If the authored
  thermal graph has independent banks, the thermal synthesizer emits one composite
  root with one generated unit per bank; the asset does not duplicate Scope shells.
- **The cross-domain wire names the real producer:** Thermal connects each
  `inputs:motor_heat_*` directly to the corresponding motor's `outputs:heat`.
  The generated-network member-output resolver maps that USD address to the
  electrical solver's declared output. A root forward is only appropriate when
  the assembly intentionally publishes a public boundary for scripts or
  external consumers; it must never hide the producer from another solver.

Connector domains are not inferred from USD property names. The resolved
Modelica connector declarations own their types, flow variables and stream
variables; generated `connect()` equations and the compiler's structural/type
checks reject incompatible or unbalanced units. This avoids duplicating
Modelica semantics in a filename list or a type-specific USD capability. See
`docs/architecture/reviews/2026-07-30-rover-domain-layering.md`.

The one exception is a model that genuinely couples two domains in one DAE
(`ThermostatHeater`, with both `Pin elec_port` and `HeatPort thermal_port`): it
lives in the domain that owns its switching state (electrical — it is a bus load)
and publishes the result causally into the other.

## Decision 2 — One canonical input-write path (collapse API onto ports)

Every public mutation now uses the existing command/port substrate:

- `SetModelInput` is a reflected Modelica command owned by the UI-free core;
  its shared helper writes through `PortRegistry` whenever the entity exposes
  a cosimulation port, and its deferred response reports the actual apply result.
- `SetPorts` / wires / the scalar-port form of Rhai `set(id,name,v)` →
  `PortRegistry::write_port` → `SimComponent.inputs`. Rhai `set` also owns the
  separate host-local reflected component/resource tuning surface.

`sync_modelica_inputs` copies `SimComponent.inputs → ModelicaModel.inputs` **every
tick**, so a direct `SetModelInput` write is **clobbered** within one frame on any
cosim'd entity. (This is why engine-cut-via-`set_input` silently fails on the
lander, and why the embedded script's `set_input(...)` — which isn't even a
registered rhai verb — is dead.)

The `PortRegistry` remains the single write surface for cosim entities:

1. `apply_set_model_input` is **port-first**: if the entity exposes a
   writable port of that name (`PortRegistry::write_port` succeeds), use the
   shared port surface. A bare workbench/batch model without a `SimComponent`
   uses its own `ModelicaModel.inputs` state, which is the authoritative input
   owner for that non-co-simulation mode.
2. Rhai `set()` uses the same `PortRegistry` for scalar co-simulation names and
   has no second input API. Its reflected component/resource path is explicitly
   host-local tuning, not a competing authoritative input channel.
3. Net: `SetModelInput`, `SetPorts`, scalar-port Rhai `set()`, Python, and wires
   all converge on `SimComponent.inputs` for co-simulated entities → the cosim
   value *is* the value everyone sees → no clobber, one source of truth. The
   MCP `set_input` tool keeps its ergonomic name + input-name validation but now
   actually sticks.

This keeps the cosim propagation core untouched (per "don't rewrite the core") and
matches the existing "one canonical form" principle.

## Decision 3 — Scenario as a first-class USD concept

A *scenario* = a USD scene that bundles: the vehicles (referenced assets), the
cosim wiring, **per-vehicle behavior scripts**, and **one orchestration script**
that runs the mission state machine + objectives.

Introduce a typed scenario root so tooling can recognize it:

```
def Scope "Scenario" ( kind = "component" )
{
    custom string lunco:scenario = "rover-surface-ops"

    def Scope "Mission" (prepend apiSchemas = ["LunCoProgramAPI"]) {                                     # orchestration
        uniform asset info:sourceAsset = @scenarios/rover_surface_ops.rhai@
        # …or author the state machine in place:
        # uniform token info:implementationSource = "sourceCode"
        # uniform string info:sourceCode = """ … """
    }
    # objectives as child prims or typed attributes:
    #   drive_to_rock(target, radius) · capture_photo() · hold_thermal(band) …
}
```

- **Orchestration script** (rhai) owns phases via the sequencer: `descend →
  landing-transition-join → deploy → handover → task_1 … task_n`, advancing on
  authored event projections and port-read predicates (altitude, joint presence,
  battery SoC, distance-to-target, temp). A landing join is explicit: it waits for
  engine cutoff and flight handoff from the GNC plus physical touchdown from the
  airframe, with source-qualified event waits.
- **Per-vehicle scripts** own local behavior (lander manual-flight assist, rover
  autonomy helpers).
- **Objectives / scoring** are rhai predicates over ports — no new engine, reuse
  the sequencing/ConOps direction (timeline + rhai = exec).

## Decision 4 — Lander GNC: reuse MSL `LimPID`, gravity from env, gains live

- Control law: `Modelica.Blocks.Continuous.LimPID` (chosen). Connector flattening
  is proven in-tree; **smoke-test `LimPID` specifically**, keep the flat-equation
  law in `DescentGuidance.mo` as the guaranteed fallback.
- **Gravity is an `input g`** wired `gravity_accel:g` — never hardcode 9.81 (lunar
  g ≈ 1.62). The env feed is position-correct.
- **Gains + set-point are `input Real`** (`kp,ki,kd,target_altitude,manual,
  manual_throttle,engine_enable`) so they retune live via the port (Decision 2).
- **Anti-windup**: integrate only within a band of the set-point and while armed,
  so the 30 m descent error can't wind the integral to garbage.
- **Manual override**: `manual=1` (player holds Space) selects
  `manual_throttle*max_thrust`; release → PID resumes. The descent is auto;
  handover is the same model, no runtime model-swap.

## Resource models for the progressive tasks

All three ship. The remaining work is **wiring them onto a rover in a gameplay
scene** and gating tasks on their ports — not authoring the maths.

| Budget | Model | Wires |
|---|---|---|
| Energy | `LunCo/Electrical/Battery.mo` (+ `SolarPanel`, `PDU`, `Pin`) — one acausal bus | pins `connect()`-ed inside one model; only signals cross as USD ports |
| Thermal | `LunCo/Thermal/ThermalMass.mo` (+ `ThermalConductor`, `Radiator`, `ThermostatHeater`) | env flux → mass → heater load |
| Bandwidth | `CommsLink.mo` (range → data-rate → buffer) | rover↔lander range → link |

## What remains

- **Wire the resource models onto a rover** as sub-prims and gate progressive
  tasks on their ports. The models exist; no gameplay scene consumes them yet.
- **MSL `LimPID` swap.** The shipped GNC (`DescentGuidance.mo`) is a flat
  velocity-scheduled law. `Modelica.Blocks.Continuous.LimPID` is the intended
  replacement; connector flattening is proven in-tree, but `LimPID` itself needs
  a smoke test first, and the flat law stays as the guaranteed fallback.

## Non-goals / explicitly deferred

- N solvers on one entity (use sub-prims).
- Rewriting the cosim propagation core or rhai port verbs.
- A bespoke objective/scoring engine (rhai predicates suffice).
