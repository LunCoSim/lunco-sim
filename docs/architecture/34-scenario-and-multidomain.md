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
| One model per prim | `lunco:modelicaModel` / `lunco:pythonModel` → `SimComponent` | ✅ |
| Cross-entity wiring (SSP Connection) | typeless wire prim with `lunco:wireFrom`/`wireTo`/`fromPort`/`toPort`/`scale` → `SimConnection` | ✅ |
| Self-wire (output→input, same entity, cross-backend) | `lunco:simWires = "out:in,…"` | ✅ |
| Gravity from environment | env publishes `gravity_accel` output (`GRAVITY_SOURCE_CONNECTOR`); model takes `g` input + wire `gravity_accel:g` | ✅ |
| Many scripts in one world | each prim with `lunco:script` → own `EmbeddedScenarioSource` → independent rhai | ✅ |
| Task state machines | rhai `seq`/`par_all`/`par_race`/`repeat` sequencer + `fn task(me)` | ✅ |
| Connector/`connect()` Modelica | rumoca flattens `RC_Circuit.mo`, `CascadedRCFilter.mo` | ✅ (verify MSL `LimPID` specifically) |
| Live input retune (no recompile) | port write changes `input Real` next step | ✅ (must be a model **input**, not a `parameter`) |

**Conclusion:** "several models / several scripts in the world" needs **no core
change** — it is the SSP sub-prim-per-model pattern below.

## Decision 1 — Multi-domain vehicle = sub-prim-per-model + wires (SSP)

Model each physical domain as its **own prim** under the vehicle Xform, each with
its own `.mo`, wired through the port surface. This *is* the FMI/SSP system
structure and needs nothing new.

```
def Xform "Lander" (PhysicsRigidBodyAPI …)        # the rigid body (avian ports)
{
    string lunco:simWires = "thrust:force_local_y" # GNC thrust → body
    def Scope "GNC"   { string lunco:modelicaModel = "models/LanderGNC.mo"
                        string lunco:simWires = "height:altitude,velocity_y:descent_rate,gravity_accel:g" }
    def Scope "Power" { string lunco:modelicaModel = "models/Battery.mo" }
    def Scope "Therm" { string lunco:modelicaModel = "models/ThermalNode.mo" }
    # wire prims connect GNC.thrust→Power.load, Power.soc→GNC.engine_enable, …
}
```

**Reject "N models on one entity" as the default.** `SimComponent` is one
input/output map; hosting multiple solvers on a single entity would force it to a
keyed multi-instance map and touch the propagation core. Sub-prim-per-model gives
the same composition with zero core change and clean per-domain identity/telemetry.
Revisit only if a need appears that sub-prims genuinely can't express.

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
    custom string lunco:script = """ … mission state machine … """   # orchestration
    # objectives as child prims or a structured custom attr:
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
   `lander_test.usda` to GNC sub-prim + `gravity_accel:g`; fix embedded script
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
