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
| One program per prim | a `LunCoProgram` prim (or `info:*` authored in place) + `info:sourceAsset` → `SimComponent`; the engine follows the extension | ✅ |
| Wiring (SSP Connection) | a native USD connection on the consumer — `inputs:x.connect = </Other.outputs:y>` → `SimConnection`; same form within a prim and across prims | ✅ |
| Gravity from environment | env publishes `gravity_accel` output (`GRAVITY_SOURCE_CONNECTOR`); the model connects `inputs:g` to it | ✅ |
| Many scripts in one world | each `LunCoProgram` prim → own `EmbeddedScenarioSource` → independent rhai | ✅ |
| Task state machines | rhai `seq`/`par_all`/`par_race`/`repeat` sequencer + `fn task(me)` | ✅ |
| Connector/`connect()` Modelica | rumoca flattens `RC_Circuit.mo`, `CascadedRCFilter.mo` | ✅ (verify MSL `LimPID` specifically) |
| Live input retune (no recompile) | port write changes `input Real` next step | ✅ (must be a model **input**, not a `parameter`) |
| Named trigger zones (geofence events) | `lunco:triggerZone="name"` → overlap-only Sensor → `enter:/exit:<name>` events | ✅ |
| Threshold events on a model port | one `def LunCoPortEvent` child prim per rule (`lunco:event:port`/`op`/`threshold`/`emit`) → edge-detect a model output in native code → event | ✅ (rumoca-safe: edge logic out of the model) |
| Per-instance program config | one typed attribute per key on the program prim — `custom float lunco:param:wmax = 1.05` → rhai `param(me,k,default)` | ✅ (the right answer instead of `name(me)` matching) |
| Emitter identity on events | `TelemetryEvent.source` (sensor/script gid); `wait_for_from(name, src)`, `evt.source` | ✅ |
| On-screen notifications | `ShowNotification` command + rhai `notify`/`notify_kind` + ui overlay | ✅ |
| Native/foreign event → script bus | `App::project_events::<E>(…)`; e.g. keyboard → `key:<KeyCode>` events | ✅ (input wired; network projector pending) |
| Throttle-driven engine plume | WGSL shader on a fixed bounding cone (`shaders/plume.wgsl`), `inputs:throttle.connect` on the gprim; its light from `LunCo.Propulsion.PlumePhotometry` onto scene-property ports | ✅ (no script — the visual is wired, not animated) |

> **Authoring walkthrough:** [`../tutorials/01-lander-rover-mission.md`](../tutorials/01-lander-rover-mission.md)
> builds this entire mission from scratch in USD + rhai + Modelica, exercising
> every mechanism in this table.

**Conclusion:** "several models / several scripts in the world" needs **no core
change** — it is the SSP one-program-prim-per-domain pattern below.

## Decision 1 — Multi-domain vehicle = one program prim per domain + connections (SSP)

A program is a PRIM with typed ports, and ports connect — the same shape `UsdShade`
gives a shader. Model each physical domain as its **own** `LunCoProgram` under the
vehicle Xform, each naming its own `.mo`, wired through the port surface. This *is* the
FMI/SSP system structure and needs nothing new.

```
def Xform "Lander" (PhysicsRigidBodyAPI …)        # the rigid body (avian ports)
{
    float inputs:force_local_y.connect = </Lander/GNC.outputs:thrust>   # GNC thrust → body

    def LunCoProgram "GNC" {
        uniform asset info:sourceAsset = @models/LanderGNC.mo@
        uniform bool  lunco:program:realtimeSafe = true                 # it drives a force
        float inputs:altitude.connect      = </Lander.outputs:height>
        float inputs:descent_rate.connect  = </Lander.outputs:velocity_y>
        float inputs:g.connect             = </Environment.outputs:gravity_accel>
        float inputs:engine_enable.connect = </Lander/Power.outputs:soc>
    }
    def LunCoProgram "Power" {
        uniform asset info:sourceAsset = @models/Battery.mo@
        float inputs:load.connect = </Lander/GNC.outputs:thrust>
    }
    def LunCoProgram "Therm" {
        uniform asset info:sourceAsset = @models/ThermalNode.mo@
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

## Decision 2 — One canonical input-write path (collapse API onto ports)

Today there are **two** ways to set a model input, and they fight:

- `SetModelInput` API / `apply_set_model_input` → writes `ModelicaModel.inputs`
  **directly**.
- `SetPort` / wires / rhai `set(id,name,v)` → `PortRegistry::write_port` →
  `SimComponent.inputs`.

`sync_modelica_inputs` copies `SimComponent.inputs → ModelicaModel.inputs` **every
tick**, so a direct `SetModelInput` write is **clobbered** within one frame on any
cosim'd entity. (This is why engine-cut-via-`set_input` silently fails on the
lander, and why the embedded script's `set_input(...)` — which isn't even a
registered rhai verb — is dead.)

**Fix — make the `PortRegistry` the single write surface:**

1. Reimplement `apply_set_model_input` to **port-first**: if the entity exposes a
   writable port of that name (`PortRegistry::write_port` succeeds) use it; else
   fall back to the direct `ModelicaModel.inputs` write (bare workbench / batch
   models with no `SimComponent`).
2. rhai `set()` already routes through `write_port` — **no rhai change needed**
   for correctness. (Only fix the *content* of the embedded scene script:
   `set_input(me,…)` → `set(me,…)`.)
3. Net: `SetModelInput`, `SetPort`, rhai `set()`, Python, and wires all converge
   on `SimComponent.inputs` for cosim'd entities → the cosim value *is* the value
   everyone sees → no clobber, one source of truth. The MCP `set_input` tool
   keeps its ergonomic name + input-name validation but now actually sticks.

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

    def LunCoProgram "Mission" {                                     # orchestration
        uniform asset info:sourceAsset = @scenarios/rover_surface_ops.rhai@
        # …or author the state machine in place:
        # uniform string info:sourceCode = """ … """
    }
    # objectives as child prims or typed attributes:
    #   drive_to_rock(target, radius) · capture_photo() · hold_thermal(band) …
}
```

- **Orchestration script** (rhai) owns phases via the sequencer: `descend →
  touchdown → deploy → handover → task_1 … task_n`, advancing on port-read
  predicates (altitude, joint presence, battery SoC, distance-to-target, temp).
- **Per-vehicle scripts** own local behavior (lander manual-flight assist, rover
  autonomy helpers).
- **Objectives / scoring** are rhai predicates over ports — no new engine, reuse
  the sequencing/ConOps direction (timeline + rhai = exec).

## Decision 4 — Lander GNC: reuse MSL `LimPID`, gravity from env, gains live

- Control law: `Modelica.Blocks.Continuous.LimPID` (chosen). Connector flattening
  is proven in-tree; **smoke-test `LimPID` specifically**, keep a flat-equation
  `LanderGNC.mo` PID as the guaranteed fallback.
- **Gravity is an `input g`** wired `gravity_accel:g` — never hardcode 9.81 (lunar
  g ≈ 1.62). The env feed is position-correct.
- **Gains + set-point are `input Real`** (`kp,ki,kd,target_altitude,manual,
  manual_throttle,engine_enable`) so they retune live via the port (Decision 2).
- **Anti-windup**: integrate only within a band of the set-point and while armed,
  so the 30 m descent error can't wind the integral to garbage.
- **Manual override**: `manual=1` (player holds Space) selects
  `manual_throttle*max_thrust`; release → PID resumes. The descent is auto;
  handover is the same model, no runtime model-swap.

## Resource models for the progressive tasks (new authoring)

| Budget | Model (new) | Wires | Gap |
|---|---|---|---|
| Energy | `Battery.mo` (SoC integral, solar in, load out) | solar-tracker → battery → consumers | small |
| Thermal | `ThermalNode.mo` (reuse lunar thermal solver settings) | env flux → node → heater load | small |
| Bandwidth | `CommsLink.mo` (range → data-rate → buffer) | rover↔lander range → link | **biggest — no model yet** |

## Implementation phasing

1. **Now (unblocks play):** legs orientation + footpad/​hull colliders **(done)**;
   `LanderGNC.mo` (flat PID, gravity input, input-gains, manual override); rewire
   `lander_ops.usda` to GNC sub-prim + `gravity_accel:g`; fix embedded script
   `set_input`→`set`; switch `lander_manual_control` to write `manual`/
   `manual_throttle`.
2. **Port-path unification (Decision 2):** port-first `apply_set_model_input`.
3. **Scenario concept (Decision 3):** typed scenario prim + orchestration script +
   objective predicates; split lander/rover/mission scripts.
4. **Domain models (Decision 1):** `Battery.mo`, `ThermalNode.mo`, then `CommsLink.mo`;
   add as rover sub-prims; gate tasks on their ports.
5. **MSL LimPID swap** once smoke-tested.

## Non-goals / explicitly deferred

- N solvers on one entity (use sub-prims).
- Rewriting the cosim propagation core or rhai port verbs.
- A bespoke objective/scoring engine (rhai predicates suffice).
