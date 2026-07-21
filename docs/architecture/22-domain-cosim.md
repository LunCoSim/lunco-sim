# 22 — Co-Simulation Domain

> Status: Active · Audience: contributors wiring simulation engines together
>
> Connects multiple simulation engines (Modelica, FMU, GMAT, Avian) in a
> single Bevy world via explicit `SimConnection`s between named ports.
> Implements the FMI/SSP pattern.

This architecture doc summarizes the high-level model. For in-depth
engineering docs (system ordering, panel registration, convention details),
see **[`../../crates/lunco-cosim/README.md`](../../crates/lunco-cosim/README.md)**.

## Core concepts

Defined in [`01-ontology.md`](01-ontology.md) section 4a:

- **`SimComponent`** — wraps a model instance; exposes named inputs / outputs.
  It is the prim's **port interface**, and it is published from the model's
  DECLARATION, not from its solution (see *Interface before solution* below).
- **`SimConnection`** — links a source port to a target port (FMI/SSP Connection)
- **`SimPort`** — metadata for a connectable interface point
- **`PortRegistry`** — the unified scalar-port surface (in `lunco-core::ports`) every
  participant reads/writes through; the cosim engine registers the built-in backends.
- **Avian as a cosim participant** — Avian physics is wired in through a typed-port
  spec table (`AvianGroup`/`AvianPort`) plus a `PendingForces` component, not a
  bespoke `AvianSim` struct.

## The port surface (one telemetry + actuation API)

Every participant's state is exposed as **named scalar ports** through the shared
**`PortRegistry`** — the single surface wires, the HTTP API (`ListPorts`/`GetPort`/
`SetPort`), the inspector, rhai, and Python all use. Avian rigid bodies, joints,
and sensors are exposed declaratively via the `AVIAN` spec table (an `AvianGroup`
per kind), not a mirror component. The available ports:

| Kind | Ports |
|---|---|
| **Rigid body** | out: `position_{x,y,z}`, `height`, `velocity_{x,y,z}`, `quat_{w,x,y,z}`, `yaw`/`pitch`/`roll`, `angvel_{x,y,z}`; in: `force_{x,y,z}`, `force_local_{x,y,z}`, `torque_{x,y,z}`, `mass`, `inertia_{xx,yy,zz}`, `com_{x,y,z}` |
| **Revolute joint** | `angle` (out = measured, in = drives `AngularMotor`) |
| **Prismatic joint** | `displacement` (out = slider offset, in = drives `LinearMotor`) |
| **Sensors** (USD `lunco:sensor:*`) | IMU `accel_{x,y,z}` + `spec_force_{x,y,z}`; range `range`; contact `contact` + `contact_force` |
| **Modelica / hardware** | model `input`/`output` vars; `value` / `raw` |

Full closures + the "add a kind = one `AvianGroup` entry" pattern live in
[`../../crates/lunco-cosim/README.md`](../../crates/lunco-cosim/README.md). USD
authoring of joints + sensors is in [`21-domain-usd.md`](21-domain-usd.md);
vehicle/lander modeling that builds on this surface is in
[`33-spacecraft-modeling.md`](33-spacecraft-modeling.md).

## Execution pipeline

All cosim and physics systems run in `FixedUpdate` at a shared fixed timestep
so every engine advances with the same `dt`:

```
FixedUpdate:
  1. ModelicaSet::HandleResponses   — receive async results from worker thread
  2. sync_modelica_outputs          — ModelicaModel.variables → SimComponent.outputs
  3. CosimSet::Propagate            — propagate_connections: source outputs → target inputs
                                       (force_* → PendingForces; joint angle/displacement → motor)
  4. CosimSet::ApplyForces          — apply_pending_forces: drain PendingForces into Avian Forces
  5. sync_inputs_to_modelica        — SimComponent.inputs → ModelicaModel.inputs
  6. ModelicaSet::SpawnRequests     — send next step command with fixed dt

FixedPostUpdate:
  7. Avian PhysicsSchedule          — integrate_positions, constraint solve, writeback
                                       (Avian outputs — Position / LinearVelocity — read on demand
                                        via PortRegistry; no separate read_avian_outputs snapshot system)
```

The master loop reads outputs, propagates through connections, writes inputs,
then steps all engines — this is the FMI master algorithm.

## The macro-step contract (what step 6 actually promises)

The ordering above is *within* a tick. The other half of an FMI-CS master is the
**communication step** itself: what `dt` each engine is asked for, and who waits
for whom. Stating it explicitly (finding `A3` — it was previously unstated, and
the code did not implement any coherent version of it):

**1. The communication grid is the FIXED-STEP clock.** Every model carries
`target_time` — the world clock, in model-local seconds — and it advances by
exactly one `Time<Fixed>` delta per **unpaused fixed tick**. Not per render
frame. A `TimeTransport.rate` burst yields *more ticks*, so it yields
proportionally more model time, automatically.

**2. The macro step is `target_time − current_time`, clamped.** `current_time` is
the model's own clock (`stepper.time()`), reported back by the worker. The
requested `dt` is therefore the model's whole **deficit**, capped at
`MAX_MACRO_STEP_DT` (~0.18 s) so one long gap cannot hand the solver a ten-second
step. A model that missed ticks — a slow solver, a long compile, a hitched frame
— **catches the time back up over the following ticks**. Nothing is dropped.
Consequently: **model time is a pure function of the fixed-step clock**, not of
frame rate, GPU load, or window focus.

**3. The coupling is explicit (Jacobi-flavoured), and the delay is measured, not
assumed.** The `Step` dispatched at tick *N* is executed on the worker thread and
its result lands at tick *N+k*, k ≥ 1. So the forces avian integrates at tick *N*
were computed from a model state that is one macro step old. This is an
**explicit / loosely-coupled** master (no iteration to a fixed point, no step
rejection, no `SetFMUState` rollback), and the resulting coupling error is
first-order in the macro step. A strict Gauss-Seidel barrier — block the fixed
tick on the worker result — is the rigorous alternative and is **deliberately not
implemented**: it would put an unbounded solver on the critical path of the main
loop, which the app's responsiveness mandate forbids.

Because the delay is real, it is **surfaced**: `lunco_modelica::worker::CosimLag`
records `|model_time − world_time|` for every live model every fixed tick, and
`warn!`s (rate-limited) past 0.25 s. In steady state it sits at about one macro
step; a sustained rise means the solver cannot keep up with the sim rate and the
forces are being computed from a stale model — the coupling has degraded from
co-simulation into extrapolation, and you can see it happen.

**4. Steps are never coalesced.** A `Step` is an integration, not a setpoint.
The worker's command-squashing (which correctly collapses redundant
`UpdateParameters`/`Compile`) explicitly does **not** apply to `Step`: dropping
one would delete `dt` of simulated time and ack it as a success. If back-pressure
is ever needed there, `dt`s must be **summed**, never dropped.

**5. The live solver is not the batch solver.** The interactive path integrates a
fixed ladder of `SECS_PER_TICK / 3` micro-steps with an explicit-family solver;
the batch/Fast-Run path keeps its adaptive-implicit BDF. See
[`28-modelica-realtime-physics.md`](28-modelica-realtime-physics.md) §2a — that
doc also states, honestly, how far short of true Tier-A determinism this still
falls.

## Where the master loop fits

The pipeline above is the *body* of the per-tick advance. The layer that
**owns** the pipeline is `Twin` — the Bevy Resource introduced in
[`14-simulation-layers.md`](14-simulation-layers.md). The loop advances
the active `Run`s, which reference `Scenario`s materialised from
`twin.toml` `[scenarios.*]`. Today's implicit "one doc, one model,
steps forever" is the degenerate case: one implicit Twin, one implicit
Run, one participant — same master loop.

## Control plane vs data plane

The master loop is the **data plane**: it runs *directly* as `FixedUpdate`
systems every tick (`propagate_connections`, `sync_*`, the stepper, etc.).
It must **never** be driven through the typed-command pipeline. A command
per tick — minting a request id, dispatching a Reflect event, recording a
`CommandResults` outcome — would put a `HashMap` insert and an envelope in
the hot loop for no benefit. Commands gate the run; the loop then runs free
(the ROS/F′ shape: a Service/Action call activates a node, whose rate group
ticks autonomously thereafter).

The **control plane** is typed commands (see AGENTS.md § 4.2 and
[`12-api.md`](12-api.md)). It owns discrete, occasional intents only:

| Plane | Examples | Mechanism |
|---|---|---|
| **Control** — discrete, occasional | `LoadScene`, `CompileModel`, `RunExperiment`, Pause/Resume/Reset, time-warp | typed `#[Command]` / `TwinCommand`. May return an `Ack` ("launched"); a long-running run then reports **completion/progress via domain state** (`Run.status`, `CompileStatus`, `RunStatus`), *not* by polling `QueryCommandResult` per tick. |
| **Data** — continuous, per-tick | the FMI master loop, the solver step, `run_scripted_models` | plain `FixedUpdate` systems. No command, no id, no result store. |
| **Live inputs** — high-frequency, latest-wins | parameter scrubs during a run, joystick/throttle (`SetModelInput`) | the **`ControlStream`** channel ([`01-ontology.md`](01-ontology.md)), applied directly (e.g. `sim.rs::apply_set_model_input` bypasses the event bus by design). Never a pollable result-returning command. |

Rule of thumb: **commands start/stop/configure a run and one-shot actions;
the simulation runs directly once started; live continuous inputs ride
ControlStream.** The result/requestId machinery (`QueryCommandResult`,
`CommandResults`) stays on the discrete control surface and never enters
the per-tick loop. Async completion of long-running runs is reported via
domain state, so it is an explicit **non-goal** of the command-result store.

## Backend registry (dynamic, plugin-driven)

Backends self-register at app boot. Each domain crate ships a Bevy
plugin that inserts itself into `BackendRegistry`:

```rust
// lunco-modelica
impl Plugin for ModelicaBackendPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BackendRegistry>();     // idempotent
        app.world_mut().resource_mut::<BackendRegistry>()
            .register(Arc::new(ModelicaBackend));
    }
}
```

Dropping a crate removes its backend. Scenarios referencing missing
backends fail gracefully at load. FMU / Python / GMAT / DCP backends
arrive as separate crates — no core edits.

`Backend` + `Participant` traits live in `lunco-cosim`. See
[`14-simulation-layers.md`](14-simulation-layers.md) for the full
signatures and capability flags.

## Typed connections + island partitioning

Connections carry a kind:

```rust
pub enum ConnectionKind {
    Causal,   // output → input (signal). Our SimConnection today.
    Acausal,  // Modelica connect, FluidPort, Flange, Pin. Kirchhoff-style.
}
```

Acausal connections cannot cross cosim boundaries without losing
accuracy (fake algebraic loops delay one signal by a step). At Run
start, the **IslandPartitioner** (planned for Phase 2/3, not yet implemented) groups participants:

1. Union-find over participants connected by acausal edges.
2. Each island must share a backend that advertises
   `caps.can_absorb_acausal`. Otherwise → scenario-load error.
3. Backend `fuse()` collapses the island into one participant. For
   Modelica this means code-generating a wrapper `.mo` that replicates
   the connections as `connect()` equations and compiling once.
4. Inter-island connections remain as `SimConnection` and are propagated
   by the master each tick (causal only).

Result: three Modelica components wired by `FluidPort`s become one
flattened DAE with one stepper (Dymola's default behaviour). A
Modelica + Python mix becomes two islands bridged by causal signals
(classical cosim). Users can opt out per participant with
`explicit_boundary = true` for debugging.

Balloon case today: Modelica balloon + Avian rigid body, three causal
wires. Two islands, one causal bridge. No acausal edges → no fusion →
identical to today's behaviour. Partitioner lands without regression.

## Dynamic bodies, not Kinematic

Balloon (and other subsystem-driven bodies) are `RigidBody::Dynamic`.
Modelica's `netForce` flows through `SimConnection` into `AvianSim.inputs`,
then `apply_sim_forces` applies it via `Forces::apply_force`. Avian's own
integrator advances velocity and position. Gravity is applied by
[`lunco-environment`](23-domain-environment.md)'s
`apply_gravity_to_rigid_bodies` system — Modelica models no longer subtract
weight; they only produce aerodynamic / buoyancy force.

Historical note: earlier designs used Kinematic bodies with direct `Position`
writes. That caused (a) change-detection conflicts with gizmo drags,
(b) double-integration when `LinearVelocity` was also written, (c) missing
collision response on joints. Current Dynamic-body design avoids all three.

## Pause and time warp

Pause / resume / reset / time-warp are all `TwinCommand` variants
dispatched through the Twin resource. The master-loop pipeline reads
`Run.status` and `Run.rate_factor` each tick:

- **Pause** = `Run.status = Paused`. Master loop skips all pipeline
  steps. Wall time continues; sim time frozen. Parameter edits queue as
  `SetParam` TwinCommands and apply on Resume (Tunable semantics).
- **Resume** = `Run.status = Running`. Master loop resumes at the next
  FixedUpdate tick from the same `current_t`.
- **Reset** = bumps session id (existing mechanism), sends
  `ParticipantCommand::Reset` to every island, clears trace +
  input_log, `current_t = t_start`, `status = Idle`.
- **Time warp** = `Run.rate_factor` scales. Per-tick advance =
  `rate_factor × FixedUpdate.dt`. Clocks with different base dt scale
  proportionally. Global slider → same factor applied to every clock's
  rate (see [`15-adaptive-fidelity.md`](15-adaptive-fidelity.md)).

Historical note (pre-Run model): earlier designs used a per-entity
`SimPaused` marker and ad-hoc `Time<Physics>::pause()`. That remains a
correct low-level mechanism, but the Run-centric model is now the
single source of truth — toolbar / API / scripts all go through
TwinCommand, not direct component mutation.

## Convention: Modelica `output` requirement

Rumoca (our Modelica runtime) eliminates algebraic variables from the
solver during DAE preparation unless they're declared as `output`. This is
a rumoca limitation that has been worked around by convention:

```modelica
model Balloon
  input Real height = 0;
  input Real velocity = 0;
  Real volume(start = 4.0);

  // ALL observable derived values must be `output`
  output Real netForce;
  output Real buoyancy;
  output Real drag;
end Balloon;
```

See [`../../crates/lunco-cosim/README.md#modelica-model-convention`](../../crates/lunco-cosim/README.md)
and [`20-domain-modelica.md`](20-domain-modelica.md) for the full story,
including planned upstream fixes to the rumoca fork.

## USD-driven authoring (`lunco_usd_sim::cosim`)

Cosim programs and wires are declared in USD scenes — no per-scene Rust.
A program is a PRIM, with typed ports that CONNECT — the same shape
`UsdShade` gives a shader. The translator (`lunco-usd-sim/src/cosim.rs`,
registered by `UsdSimPlugin`) reads:

| Property | What it does |
|---|---|
| `uniform asset info:sourceAsset = @models/Balloon.mo@` | Names the program's file. The ENGINE follows from the extension, never from a second attribute: `.mo` opens the source, publishes `ModelicaModel` + `SimComponent` from the PARSE and dispatches `ModelicaCommand::Compile`; `.py` registers a `ScriptDocument` and attaches `ScriptedModel` + `SimComponent`, stepped by `lunco-scripting::run_scripted_models` each `FixedUpdate`. |
| `uniform string info:sourceCode` | The same, for a program authored in place rather than in a file. |
| `uniform bool lunco:program:realtimeSafe` | The author's promise that the program may drive a force on a client-predicted body (see [`28-modelica-realtime-physics.md`](28-modelica-realtime-physics.md)). |
| `float inputs:<port>` / `float outputs:<port>` | The program's ports. A `.connect` makes one a wire; a constant makes it a parameter. A prim is stepped iff it BOTH binds a program AND declares ports. |

A program that is bolted onto a thing — a guidance law, a battery, a supervisory script
— is a `def LunCoProgram` CHILD prim, so deleting the prim removes the behaviour. A prim
that IS a program — a vessel's own flight-control system, inseparable from the airframe
— authors the `info:*` properties on itself instead.

A wire is a native USD connection, authored on the prim that CONSUMES the value. The
same form serves within one prim (a model's output driving the body's force input) and
*between* prims (the target path simply names another one):

```usda
def LunCoProgram "Amplifier"
{
    uniform asset info:sourceAsset = @models/Amplifier.py@
    float inputs:signal.connect = </Scene/Oscillator.outputs:signal>
}
```

`rewire_usd_connections` resolves each connection to ECS entities
(deferred until both endpoints exist — handles async USD asset loads)
and spawns one `SimConnection` per resolved edge.

The result: a multi-component, multi-language cosim is a USD edit, not
a Rust edit. `cross_entity_cosim_test` exercises the canonical chain
(Modelica oscillator → Python amplifier → Avian sphere) headlessly in
~1.3 s.

### Interface before solution

A model's INTERFACE — its `input Real …` and parameters — is a **declaration**
the parse already yields. Only its SOLUTION (`variables`, the outputs) needs the
solver. So `wrap_modelica_into_simcomponent` publishes the `SimComponent` as
soon as `ModelicaModel` exists, carrying the AST's inputs, with
`SimStatus::Compiling` until variables populate (`can_step()` already refuses to
step that state). `modelica_status()` is the single place that decides
Compiling / Running / Paused, so the bind and the per-tick sync cannot disagree.

It used to wait for `variables`. For the few hundred milliseconds until the
worker answered, the prim existed with **no ports at all** — so every wire into
it hit `write_port → false` and the propagation master reported a *dangling
wire*: a diagnostic that means "your wiring is wrong", raised for wiring that was
correct. On the solar-rover demo that was `sun_azimuth`, `panel_yaw` and
`vehicle_throttle` on every load.

Two lessons generalise beyond Modelica:

- **A not-yet-ready participant must not look like a misconfigured one.** The
  fix is to remove the window (publish what is already known), not to teach the
  master to tolerate it — a tolerance would also swallow the real error.
- **A deduplicated diagnostic must be scoped to what it describes.** That report
  is deduped per port NAME in a `Local`, so one load-time false positive
  silenced the genuine report for that name for the rest of the process. It now
  clears whenever the fabric rewires.

A **Python** program still declares no ports (`SimComponent.inputs` is empty and
`ScriptDocument.inputs` is hardcoded), so dangling-wire reports against one are
genuine — that gap is real, not this race.

## Runtime scene control

The `LoadScene` typed command (registered by `UsdSimPlugin`) reloads or
replaces the active scene without restarting the binary:

```bash
curl -X POST http://127.0.0.1:4101/api/commands \
  -d '{"command":"LoadScene","params":{"path":"scenes/sandbox/sandbox_scene.usda","root_prim":""}}'
```

It despawns every entity carrying `UsdPrimPath`, despawns every
`SimConnection`, force-reads the asset from disk, and spawns a fresh
root under the first `Grid`. Authoring loop: edit `.usda`, curl, see
new scene.

`CosimStatus` (`ApiQueryProvider`) returns a snapshot of every
USD-driven cosim entity (`UsdSourcedCosim`) — position, velocity,
Modelica timing, propagated `force_y` — for live introspection without
log polling.

## See also

- [`../../crates/lunco-cosim/README.md`](../../crates/lunco-cosim/README.md) — engineering docs
- [`../../crates/lunco-usd-sim/README.md`](../../crates/lunco-usd-sim/README.md) — USD translator details (the cosim attributes above)
- [`20-domain-modelica.md`](20-domain-modelica.md) — Modelica-specific design
- [`23-domain-environment.md`](23-domain-environment.md) — environment/gravity integration
- `specs/014-modelica-simulation` — detailed Modelica spec
