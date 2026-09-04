# 22 — Co-Simulation Domain

> Status: Active · Audience: contributors wiring simulation engines together
>
> Connects multiple simulation engines (Modelica, FMU, GMAT, Avian) in a
> single Bevy world via explicit `SimConnection`s between named ports. Its
> master-side transaction is FMI/SSP-shaped; the current Modelica backend is not
> itself an FMI implementation.

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
  `entity_port_infos` adds the same live values with owner-supplied type, unit,
  bounds, source, authority, and writability for `ReadPorts` and the native
  Ports panel.
- **`InputPorts` / `OutputPorts`** — an imperative producer's authored command
  inputs and runtime outputs. `InputPorts` accepts writes; `OutputPorts` is
  read-only and exposes values written by an authored program such as a
  Modelica drivetrain.
  A mobility root is identified separately by `lunco_core::MobilityRoot`;
  `OutputPorts` is never used as a vehicle marker.
  Generated Modelica outputs remain on `SimComponent` and are not copied into
  `OutputPorts`.
- **Avian as a cosim participant** — Avian physics is wired in through a typed-port
  spec table (`AvianGroup`/`AvianPort`) plus a `PendingForces` component, not a
  bespoke `AvianSim` struct.

## The port surface (one telemetry + actuation API)

Every participant's state is exposed as **named scalar ports** through the shared
**`PortRegistry`** — the single surface wires, the HTTP API (`ListPorts`/`ReadPorts`/
`GetPort`/`SetPorts`), the inspector, rhai, and Python all use. Avian rigid bodies, joints,
and sensors are exposed declaratively via the `AVIAN` spec table (an `AvianGroup`
per kind), not a mirror component. The available ports:

| Kind | Ports |
|---|---|
| **Rigid body** | out: `position_{x,y,z}`, `velocity_{x,y,z}`, `quat_{w,x,y,z}`, `yaw`/`pitch`/`roll`, `angvel_{x,y,z}`; in: `force_{x,y,z}`, `force_local_{x,y,z}`, `torque_{x,y,z}`, `mass`, `inertia_{xx,yy,zz}`, `com_{x,y,z}` |
| **Revolute joint** | `angle` (out = measured, in = drives `AngularMotor`) |
| **Prismatic joint** | `displacement` (out = slider offset, in = drives `LinearMotor`) |
| **Avian observations** | Native rigid-body/contact ports plus generic ray ray_distance, ray_hit_valid, hit point/normal, and sample time |
| **Modelica sensor conversions** | IMU, altimeter, attitude estimator, and touchdown models with authored inputs/outputs wires |
| **Modelica / hardware** | model `input`/`output` vars; `value` / `raw` |
| **Imperative producer** | authored `inputs:*` commands and read-only `outputs:*` values through `InputPorts` / `OutputPorts` |

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
  1. sync_modelica_outputs          — completed Modelica results → SimComponent.outputs
  2. CosimSet::Propagate            — propagate_connections: source outputs → target inputs
                                       (force_* → PendingForces; joint angle/displacement → motor)
  3. CosimSet::ApplyForces          — apply_pending_forces: drain PendingForces into Avian Forces
  4. sync_inputs_to_modelica        — SimComponent.inputs → ModelicaModel.inputs
  5. ModelicaSet::SpawnRequests     — send next step command with fixed dt

FixedPostUpdate:
  6. Avian PhysicsSchedule          — integrate_positions, constraint solve, writeback
  7. raw Avian ray observations     — query after writeback; consumed by the next FixedUpdate
                                       (Avian outputs — Position / LinearVelocity — read on demand
                                        via PortRegistry; no separate read_avian_outputs snapshot system)

Update:
  7. ModelicaSet::HandleResponses   — receive async results and release the coupling barrier
```

The master loop reads outputs, propagates through connections, writes inputs,
then steps all engines — this is the FMI master algorithm.

## The macro-step contract (what step 6 actually promises)

The ordering above is *within* a tick. The other half of an FMI-CS master is the
**communication step** itself: what `dt` each engine is asked for, and who waits
for whom. Stating it explicitly (finding `A3` — it was previously unstated, and
the code did not implement any coherent version of it):

**1. Communication points are explicit and sub-rated.** Every model carries a
`communication_period_secs` and `next_communication_time`. The world still
advances on the fixed 60 Hz clock, but a Modelica participant is not a 60 Hz
render callback. Inputs are sampled and outputs are published only at the next
declared point; the last validated output is held between points. The USD
`LunCoProgramAPI` property `lunco:program:communicationPeriod` is the source of
truth. When the property is omitted, the schema's documented 0.1 s (10 Hz)
value is used. An authored non-finite, sub-tick, over-sized, or non-lattice
value is a terminal model-configuration error and is never replaced by that
semantic default. The live Rumoca participant accepts communication periods
from one master fixed tick through `MAX_MACRO_STEP_DT`, in integer master-tick
units. This master admits at most one asynchronous participant transaction per
fixed tick, so a sub-tick period is rejected instead of being silently
undersampled. A model that needs every physics tick authors one fixed-tick
period explicitly.

**2. The macro step is `communication_point − current_time`, bounded by the
participant contract.**
`current_time` is the model's own clock (`stepper.time()`), reported back by the
worker. A `TimeTransport.rate` burst therefore produces more Avian/Rhai fixed
ticks and proportionally more *declared* Modelica communication points, rather
than multiplying worker callbacks at the render cadence. The requested `dt` is
capped at `MAX_MACRO_STEP_DT` (~0.18 s) for catch-up, so one explicit time jump
cannot hand the solver a ten-second step. Normal authored communication points
are validated to fit that bound and the fixed master-tick lattice; the worker
receives the master's sequence number and `[start_time, stop_time]`, validates
both against its own solver clock, and the main thread accepts only the exact
in-flight response at that endpoint. A worker result is still required before
the shared world crosses the communication point, so no stale value crosses a
causal boundary.

**3. The communication barrier follows the causal topology; worker execution is asynchronous.**
The `Step` dispatched at tick *N* is executed on the worker thread. The
simulation waits before crossing the communication point only for participants
in the graph-derived reverse causal closure of a stateful engine sink. A
telemetry, electrical, or supervisory model with no path to shared physics may
remain in flight while Avian and the rest of the shared world continue using
its last validated output. This is synchronous semantics for causal paths,
with non-blocking implementation for independent paths:

* `SimulationBarrier` is raised at a coupled participant's communication point
  and projected by the time spine onto `Time<Virtual>`. Therefore `SimTick`, Rhai,
  controllers, connection propagation, Modelica inputs, and Avian stop only at
  a real causal exchange boundary. Between boundaries they continue using the
  declared zero-order hold. A result is consumed in `Update`; the next fixed
  tick then reads the fresh output, propagates it, applies forces/motors once,
  and schedules the next point.
* For a coupled request, the current fixed tick is the only tick admitted after
  the request is sent; any remaining same-frame fixed overstep is discarded. A
  slow solver costs wall time, but cannot create stale-force bursts, unbounded
  simulated-time debt, or a Rhai/controller tick that runs ahead of the state
  it observes. An independent request does not raise this barrier and its last
  validated output remains held until its next completed communication point.
* Membership is computed from the composed wiring projection by walking
  backwards from backend-owned `CausalStateSink` capabilities. Avian, joint,
  hardware, and wheel backends mark the actual endpoint that writes state;
  future FMU or other backends join the same rule by marking their own
  state-writing endpoint. This captures intermediate participant chains and
  feedback without classifying every Modelica model as global. While the
  binding epoch or any edge is unresolved, membership remains fail-closed and
  all live Modelica participants retain the barrier.

**4. Script participants use the same fixed-step transaction boundary.**
`sync_script_inputs` runs after causal propagation and physics actuation, then
the public `lunco_scripting::ScriptingSet` executes each Python/Rhai participant
once. Its output is published before the next propagation phase, so the output
is consumed on the following fixed tick. This explicit one-tick delay is the
conservative discrete co-simulation rule; it prevents a script/physics
algebraic cycle from depending on Bevy system insertion order. Modelica input
sampling and script input sampling therefore occur at named schedule edges,
not as unsynchronised per-frame callbacks.

Because the wait is real, it is **surfaced**: `lunco_modelica::worker::CosimLag`
records the communication gap for every live participant every fixed tick, and
`warn!`s (rate-limited) past 0.25 s. An off-thread worker is not a second
simulation clock. An independent model still uses the same authoritative world
time and explicit communication points; it is simply not allowed to stall the
world when its outputs are not on a shared-state causal path.

**5. Steps are never coalesced.** A `Step` is an integration, not a setpoint.
The worker's command-squashing (which correctly collapses redundant
`UpdateParameters`/`Compile`) explicitly does **not** apply to `Step`: dropping
one would delete `dt` of simulated time and ack it as a success. If back-pressure
is ever needed there, `dt`s must be **summed**, never dropped.

**6. The live solver is not the batch solver.** The interactive path integrates a
fixed ladder of `SECS_PER_TICK / 3` micro-steps with an explicit-family solver;
the batch/Fast-Run path keeps its adaptive-implicit BDF. See
[`28-modelica-realtime-physics.md`](28-modelica-realtime-physics.md) §2a — that
doc also states, honestly, how far short of true Tier-A determinism this still
falls.

## FMI boundary and conformance claim

The contract above is deliberately an **FMI-CS-shaped master algorithm**, not a
claim that the current Modelica worker is already an FMI implementation. The
current code has the right master-side invariants for a future FMU backend:

* one authoritative simulation time and explicit communication points;
* input snapshots at a communication point;
* a positive, bounded communication step;
* zero-order hold of the last validated outputs between points;
* conservative waiting only for participants whose outputs can reach a
  state-writing sink; and
* explicit faulting instead of releasing a failed coupled step as if it had
  completed.

An FMU importer still has to be implemented at the participant boundary. It
must load and validate `modelDescription.xml`, preserve FMI value references
and declared types/units, perform the FMI 2.0 or 3.0 lifecycle, call the
version-appropriate co-simulation step function, and handle every returned
status and early-return/event request. FMI 3.0 specifically permits
`fmi3DoStep` to return before the requested communication point and adds Event
Mode and Intermediate Update Mode; an importer that ignores those results is
not robust FMI support. See the [FMI 3.0.2 specification](https://fmi-standard.org/docs/3.0.2/).

The FMU adapter must be a backend participant, not a second clock or a second
wire fabric. It maps FMI variables to the existing port/connection boundary,
marks a state-writing endpoint with `CausalStateSink` when appropriate, and
uses the same conservative scheduler. FMI for Model Exchange is a separate
integration: the FMU does not own the solver step, so it cannot be treated as
an FMI-CS participant. The current generic port currency is scalar `f64`, so
the first adapter scope is FMI Real ports; Boolean, integer, enumeration,
string, arrays, clocks, and typed units require an explicit typed FMI boundary
before claiming broad FMI compatibility.

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
| **Control** — discrete, occasional | `LoadScene`, `CompileModel`, `RunExperiment`, Pause/Resume/Reset, time-warp | typed `#[Command]` / `TwinCommand`. May return an `Ack` ("launched"); a long-running run then reports **completion/progress via domain state** (`Run.status`, `CompileStatus`, `RunStatus`), not a per-tick result endpoint. |
| **Data** — continuous, per-tick | the FMI master loop, the solver step, `run_scripted_models` | plain `FixedUpdate` systems. No command, no id, no result store. |
| **Live inputs** — high-frequency, latest-wins | joystick/throttle (`SetPorts`) | the **`ControlStream`** channel ([`01-ontology.md`](01-ontology.md)), applied through the shared port command observer. The receiver latches each named vehicle command until replacement or explicit `ReleasePort`/`ReleaseControl`; the reflected command is the same path for API, Rhai, UI, and network input. |
| **Modelica input injection** — discrete | `SetModelInput` | the reflected Modelica command, registered by the UI-free Modelica core and applied through the shared input helper. |

Rule of thumb: **commands start/stop/configure a run and one-shot actions;
the simulation runs directly once started; live continuous inputs ride
ControlStream.** The result/request correlation machinery (the original deferred response,
`CommandResults`) stays on the discrete control surface and never enters
the per-tick loop. Async completion of long-running runs is reported via
domain state, so it is an explicit **non-goal** of the command-result store.

A result-returning command puts its command-specific payload in `Ack.data` and
returns it once on the command response (`{ id, ok, data?, error? }`). This is
the general response shape for allocated ids, queued status, generated text,
stdout, and similar request results. It is not a telemetry channel: a value
that changes during the simulation remains an authored `outputs:<name>` port
and is read through the port registry or a USD connection.

For authored simulation signals, use the complementary rule: a command enters
a producer through `inputs:<name>`, and a produced setpoint leaves through
`outputs:<name>`. The USD connection is the topology authority. `OutputPorts`
only stores values for an imperative producer; it is not a second signal graph
and must not shadow a generated Modelica output.

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

Current invariant: subsystem-driven bodies are Dynamic and receive forces
through the Avian force path. Direct `Position` writes are not a co-simulation
mechanism; this preserves change detection, single integration, and joint
collision response.

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

The Run resource is the single source of truth for pause, reset, and rate
changes. Toolbar, API, and scripts dispatch `TwinCommand`; they do not mutate
per-entity pause markers or physics time directly.

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
| `uniform token info:implementationSource` | Selects exactly one implementation arm. The selected source asset's extension dispatches to Modelica or Python; a populated non-selected arm is invalid. |
| `uniform asset info:sourceAsset = @models/Balloon.mo@` | Names the program's file. The ENGINE follows from the extension, never from a second attribute: `.mo` opens the source, publishes `ModelicaModel` + `SimComponent` from the PARSE and dispatches `ModelicaCommand::Compile`; `.py` registers a `ScriptDocument` and attaches `ScriptedModel` + `SimComponent`, stepped by `lunco-scripting::run_scripted_models` each `FixedUpdate`. |
| `uniform string info:sourceCode` | The same, for a program authored in place rather than in a file. |
| `uniform bool lunco:program:realtimeSafe` | The author's promise that the program may drive a force on a client-predicted body (see [`28-modelica-realtime-physics.md`](28-modelica-realtime-physics.md)). |
| `float inputs:<port>` / `float outputs:<port>` | The program's ports. A `.connect` makes one a wire; a constant makes it a parameter. A prim is stepped iff it BOTH binds a program AND declares ports. |

A program that is bolted onto a thing — a guidance law, a battery, a supervisory script
— is a a `Scope` applying `LunCoProgramAPI` CHILD prim, so deleting the prim removes the behaviour. A prim
that IS a program — a vessel's own flight-control system, inseparable from the airframe
— authors the `info:*` properties on itself instead.

A wire is a native USD connection, authored on the prim that CONSUMES the value. The
same form serves within one prim (a model's output driving the body's force input) and
*between* prims (the target path simply names another one):

```usda
def Scope "Amplifier" (prepend apiSchemas = ["LunCoProgramAPI"]) {
    uniform asset info:sourceAsset = @models/Amplifier.py@
    float inputs:signal.connect = </Scene/Oscillator.outputs:signal>
}
```

`rewire_usd_connections` resolves each connection to ECS entities and spawns one
`SimConnection` per resolved edge. A generated domain root's `inputs:` boundary
is deferred until projection has installed its `ModelicaModel`: the model-arrival
event explicitly rebuilds the derived wire cache. The connection system therefore
waits for both entities **and** the target's runtime contract; it never creates an
edge merely to discover on a later fixed tick that the port surface was absent.

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
correct. On older solar-rover scenes that included `sun_azimuth`, `panel_yaw`
and `vehicle_throttle` on every load. The current solar-rover scene has no
sun-to-light Modelica wire: ephemeris owns the semantic sun sample, and the
rover's `SunTracker` consumes the explicit `EnvironmentProbe` outputs.

Two lessons generalise beyond Modelica:

- **A not-yet-ready participant must not look like a misconfigured one.** The
  composer defers an edge until its target contract exists; the propagation
  master classifies a compiling endpoint as `pending`, not failed. Once the
  contract is running or errored, an unknown input becomes one terminal fault.
  This keeps load ordering out of both logs and test verdicts without swallowing
  a real typo.
- **A deduplicated diagnostic must be scoped to what it describes.** That report
  is deduped per port NAME in a `Local`, so one load-time false positive
  silenced the genuine report for that name for the rest of the process. It now
  clears whenever the fabric rewires.

A **Python** program has the same explicit USD interface contract as a Modelica
program. `lunco-usd-sim` derives the declared scalar ports from the program prim
and the Python runtime consumes those values when the Python feature is enabled.
An asset with no authored `inputs:`/`outputs:` is source-only: it can be attached
and inspected, but it is not a runnable cosim participant and a wire to it must
be rejected as an undeclared port. The author adds the contract through
`AttachProgram` or an authored USD edit; the engine never guesses ports from
Python source.

### Attach a source-backed program

`AttachProgram` is the shared authoring command for Modelica, Python, Rhai, and
behaviour-tree source assets. It validates one explicit `ProgramAttachSpec`,
lowers it to one USD change set, journals it, and lets normal USD projection
create the runtime participant. Its `info:implementationSource = "sourceAsset"`
arm selects the runtime adapter by extension; its declared inputs and outputs
remain USD facts.

Rhai authors can use the `assembly_edit` tool without constructing the map by hand:

```rhai
assembly_edit::attach_program(doc, #{
    edit_target: "@runtime@",
    host_path: "/Vessel",
    name: "Guidance",
    source_asset: "lunco://models/Guidance.mo",
    inputs: [
        assembly_edit::program_input_connection("altitude", "/Vessel.outputs:position_y"),
        assembly_edit::program_input_default("gravity", 1.62),
    ],
    outputs: [assembly_edit::program_output("thrust", ["/Vessel.inputs:force_y"])],
    realtime_safe: true,
});
```

The palette uses the same command and only offers document-backed scene
primitives. A raw scene has no editable USD document layer, so it must be
promoted to a Twin before authoring. Program edits stay in USD; continuous
control math stays in Modelica or the owning Rust mechanism; Rhai remains
orchestration and test policy.

## Runtime scene control

The `LoadScene` typed command (registered by `UsdSimPlugin`) reloads or
replaces the active scene without restarting the binary:

```bash
curl -X POST http://127.0.0.1:4101/api/commands \
  -d '{"type":"ExecuteCommand","command":"LoadScene","params":{"path":"lunco://scenes/luncosim/sandbox_scene.usda","root_prim":""}}'
```

It despawns every entity carrying `UsdPrimPath`, despawns every
`SimConnection`, force-reads the asset from disk, and spawns a fresh
root directly under the canonical `WorldGrid`. Authoring loop: edit `.usda`, curl, see
the new scene. Invalid or duplicate world-shell state is reported rather than
selecting an arbitrary grid.

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
