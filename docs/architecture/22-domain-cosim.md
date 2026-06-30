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

- **`SimComponent`** — wraps a model instance; exposes named inputs / outputs
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
start, the **IslandPartitioner** groups participants:

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

Cosim entities and wires are declared in USD scenes — no per-scene Rust.
The translator (`lunco-usd-sim/src/cosim.rs`, registered by
`UsdSimPlugin`) reads three attribute kinds from any USD prim that
participates in a cosim:

| Attribute | What it does |
|---|---|
| `string lunco:modelicaModel = "models/Balloon.mo"` | Opens the source, dispatches `ModelicaCommand::Compile`, populates `ModelicaModel` + `SimComponent` once the worker returns. |
| `string lunco:pythonModel = "models/Amplifier.py"` | Registers a `ScriptDocument`, attaches `ScriptedModel` + `SimComponent`. Stepped by `lunco-scripting::run_scripted_models` each `FixedUpdate`. |
| `string lunco:simWires = "from:to,from:to:scale,..."` | Comma-separated **self-loop** wires (same entity, different ports). Each entry spawns one `SimConnection`. Empty string is legal for cross-entity-only entities. |

For wires *between* entities, declare a typeless prim with two rels:

```usda
def "OscToAmp"
{
    rel    lunco:wireFrom = </Scene/Oscillator>
    string lunco:fromPort = "signal"
    rel    lunco:wireTo   = </Scene/Amplifier>
    string lunco:toPort   = "signal"
    double lunco:scale    = 1.0
}
```

`process_usd_cosim_wires` resolves rels to ECS entities each tick
(deferred until both endpoints exist — handles async USD asset loads)
and spawns one `SimConnection` per resolved wire.

The result: a multi-component, multi-language cosim is a USD edit, not
a Rust edit. `cross_entity_cosim_test` exercises the canonical chain
(Modelica oscillator → Python amplifier → Avian sphere) headlessly in
~1.3 s.

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
