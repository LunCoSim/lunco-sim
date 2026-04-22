# 14 — Simulation Layers

> Four layers — **Model, Run, Scenario, Twin** — plus a dynamic **BackendRegistry**
> that lets simulation crates self-register. Aligns with FMI/SSP patterns,
> scales from single-model MVP to cosim + remote + HIL without refactor.
>
> **Twin is the control surface.** Not just a filesystem container — it's the
> live Bevy resource that owns scenarios, runs, backends, traces, and input
> queues for its slice of the world. Every command (start / pause / reset /
> step / warp / switch-fidelity) dispatches through Twin. UI, HTTP API,
> scripts, remote controllers, and replay all go through the same Twin
> interface.

This doc is the canonical reference for how simulation is organised. Specific
domain details live elsewhere:

- Cosim plumbing (`SimConnection`, propagation order, Avian coupling):
  [`22-domain-cosim.md`](22-domain-cosim.md).
- Twin container + filesystem layout: [`13-twin-and-workflow.md`](13-twin-and-workflow.md).
- Modelica specifics: [`20-domain-modelica.md`](20-domain-modelica.md).
- Adaptive fidelity / LoD: [`15-adaptive-fidelity.md`](15-adaptive-fidelity.md).
- Real-time pacing + HIL: tracked by future domain doc.

## The four layers

Twin is at the top because **Twin controls everything**. It's a live Bevy
resource plus a filesystem artefact — the two faces of the same thing.
Scenario/Run/Model hang off it as controlled children.

```
Twin (resource + on-disk artefact)
  control surface — owns BackendRegistry, dispatches all commands
  persisted as:
    twin.toml          manifest + [scenarios.*]
    models/            source docs (.mo, .fmu, .usd, .sysml, .py)
    scenarios/         named participant graphs (RON)
    runs/              past sessions (trace, checkpoints, verdict)
           │
           ▼
Scenario (runtime child of Twin)
  clocks, participants, connections, verifier, t_span
  partitioned into islands at Run-start by BackendRegistry + causality rules
           │
           ▼
Run (runtime child of Scenario)
  mode, rate_factor, current_t, trace, input_log, status, input queue
           │
           ▼
Model (runtime child of Run, one per island)
  compiled stepper:
    Modelica DAE  |  FMU  |  Avian rigid body  |  Python  |  Remote DCP
```

Each layer owns a clear slice of state:

| Layer     | Owns                                                           | Lifetime                           |
|-----------|----------------------------------------------------------------|------------------------------------|
| Twin      | **control authority** + documents + scenarios + runs archive + registry | app-session (live) + on-disk (persistent) |
| Scenario  | graph shape + initial state + verifier                          | materialised on Run start           |
| Run       | one session's clock + trace + inputs + participants             | from ▶ to ⏹ / Completed / Failed    |
| Model     | compiled stepper per island                                     | rebuilt from source on (re)compile  |

Children don't act autonomously — they act **through Twin**. The toolbar's ▶
button doesn't mutate a `Run` directly; it fires a `TwinCommand::StartRun`
that Twin handles, validates against its state, and acts on. This is what
makes remote control, scripting, and multi-user coexist with local UI:
they're all just alternate Twin-command producers.

### Why four, not three

"Scenario vs Run" sometimes looks redundant. The split exists because:

- **One scenario → many runs** (same wiring, different seeds / parameters /
  environments). Monte Carlo, regression sweeps, A/B tuning.
- **Scenario is the user's authoring concern** (graph, wiring, initial
  state). **Run is the session concern** (trace, inputs captured, timing).

Collapsing them forces re-authoring the graph every time you hit ▶ again.

### Why Twin owns scenarios (not loose files)

Spec 13 calls Twin "what a project is to an IDE — the unit of saving,
versioning, sharing, opening". Scenarios piggyback on that: they're declared
in `twin.toml` under `[scenarios.*]`, the files live under `<twin>/scenarios/`,
past runs go under `<twin>/runs/`. One artefact ships the whole thing.

Loose `.scn.ron` files at arbitrary paths would recreate the ECS-as-
source-of-truth problem that spec 00-overview flags: no clear owner, no
round-trip, no versioning story.

## Twin as a runtime control resource

Twin is not a passive manifest. It's a live Bevy resource that acts as the
single point of control for its slice of the simulation world.

```rust
#[derive(Resource)]
pub struct Twin {
    // Identity + persistence
    pub id: TwinId,
    pub root_path: PathBuf,          // filesystem root
    pub manifest: TwinManifest,      // parsed twin.toml

    // Runtime children
    pub backends: BackendRegistry,   // per-twin backend view; registered on load
    pub scenarios: HashMap<ScenarioId, Scenario>,
    pub runs: HashMap<RunId, Run>,
    pub active_run: Option<RunId>,

    // Control plane
    pub input_queue: InputQueue,
    pub pending_commands: VecDeque<TwinCommand>,
}
```

### Every command is a TwinCommand

```rust
pub enum TwinCommand {
    // Run lifecycle
    StartRun  { scenario: ScenarioId, seed: Option<u64>, mode: RunMode },
    PauseRun  { run: RunId },
    ResumeRun { run: RunId },
    ResetRun  { run: RunId },
    StopRun   { run: RunId },
    StepRun   { run: RunId, n: u32 },

    // Time + fidelity
    SetRateFactor   { run: RunId, rate: f64 },
    SwitchFidelity  { run: RunId, participant: ParticipantId, target: FidelityId },

    // Inputs (go into InputQueue, drained on next tick)
    SetInput { run: RunId, participant: ParticipantId, var: String, value: f64 },
    SetParam { run: RunId, participant: ParticipantId, var: String, value: f64 },

    // Scenario editing (live)
    InsertParticipant { scenario: ScenarioId, participant: ParticipantSpec },
    RemoveParticipant { scenario: ScenarioId, id: ParticipantId },
    WireConnection    { scenario: ScenarioId, connection: Connection },

    // Persistence
    SaveTwin,                        // flush manifest + any dirty state
    ArchiveRun { run: RunId },       // finalise into <twin>/runs/<id>/

    // Replay
    LoadRun { run_dir: PathBuf } -> RunId,
    ReplayRun { run: RunId, speed: f64 },
}
```

Every command dispatches through Twin, so every command is:

- **Observable** — an audit log can snapshot `pending_commands`.
- **Routable** — remote HTTP, scripting, and local UI push to the same queue.
- **Validatable** — Twin checks RBAC (spec 010), run status, backend
  support before applying.
- **Replayable** — the command stream is itself a reproducibility artefact.

### Who fires TwinCommands

Three equally-legitimate sources:

1. **Local UI** — toolbar buttons, Welcome learning-path clicks, Graphs
   drag-variable-to-plot — all publish TwinCommands through Bevy events,
   one observer converts them into `pending_commands.push_back(...)`.
2. **HTTP / gRPC / DCP / remote clients** — the existing `--api PORT`
   surface (spec 022's headless server) accepts serialised TwinCommands.
   Scripts, agents, and remote workbench sessions all drive simulations
   via network.
3. **Scripts / scenarios** — a scenario file can embed a sequence of
   TwinCommands (`at t=5.0: SetInput { throttle: 0.0 }`) — this is how
   deterministic regression tests, scripted rehearsals, and spec 020
   playback work.

### Twin systems (Bevy)

```
drain_twin_commands          // FixedPreUpdate: pending_commands → state mutations
  ↓
materialise_scenario          // if StartRun: run partitioner, spawn participants
  ↓
advance_active_runs           // FixedUpdate: per-run tick pipeline (step 1-9)
  ↓
archive_completed_runs        // if Run.status == Completed/Failed: flush to disk
  ↓
save_dirty_twin_state         // on SaveTwin or auto-save interval
```

The existing cosim pipeline in [`22-domain-cosim.md`](22-domain-cosim.md)
becomes the body of `advance_active_runs` — unchanged mechanically, now
scoped per-Run inside Twin.

### One Twin resource per open Twin

An app instance can hold multiple Twins (e.g., opening two workspaces side
by side). Each Twin is its own resource; its entities carry a
`TwinAffiliation(TwinId)` marker; queries filter by it so participants of
Twin A don't leak into Twin B's cosim.

For MVP, a single Twin is enough — the one backing the currently open
workspace folder. Multi-Twin is an unblocked follow-up when the UI surfaces
multiple workspaces.

### Headless + remote

Because Twin is a resource and all control flows through TwinCommands, a
headless server is *exactly* a Bevy app with:

- WorkbenchPlugin minus rendering
- one or more Twin resources loaded from disk
- an HTTP/gRPC adapter translating incoming requests into TwinCommands

No special "server mode" in Twin itself — it's the same resource running in
a different app shell. This is how spec 022's "FMU runs on headless server,
clients receive updates" is realised: server has the full Twin; clients run
a stub Twin with replica backends and mirror state via replication.

## BackendRegistry — dynamic, plugin-driven

Simulation backends **self-register** at app build time. Each domain crate
ships a Bevy plugin that adds itself to the registry:

```rust
// in lunco-modelica
pub struct ModelicaBackendPlugin;
impl Plugin for ModelicaBackendPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BackendRegistry>(); // idempotent
        app.world_mut().resource_mut::<BackendRegistry>()
            .register(Arc::new(ModelicaBackend));
    }
}
```

Dropping a crate from `Cargo.toml` removes its backend cleanly. Scenarios
referencing a missing backend surface a friendly error at load, not a
crash. Adding FMU / Python / GMAT support = one new crate, zero core edits.

### Traits (in `lunco-cosim`)

```rust
pub trait Backend: Send + Sync + 'static {
    fn id(&self) -> &'static str;             // "modelica", "fmu-cs", "avian", …
    fn caps(&self) -> BackendCaps;
    fn load(&self, source: &ModelSource) -> Result<Box<dyn Participant>>;
    fn can_fuse(&self, others: &[&dyn Participant]) -> bool;
    fn fuse(&self, participants: Vec<Box<dyn Participant>>,
            connections: &[Connection]) -> Result<Box<dyn Participant>>;
}

pub trait Participant: Send + Sync {
    fn backend_id(&self) -> &str;
    fn step(&mut self, dt: f64) -> Result<StepOutcome>;
    fn get(&self, var: &str) -> Option<f64>;
    fn set(&mut self, var: &str, value: f64);
    fn checkpoint(&self) -> Result<ParticipantState>;
    fn restore(&mut self, state: &ParticipantState) -> Result<()>;
    fn ports(&self) -> &[PortDef];
}

pub struct BackendCaps {
    pub can_absorb_acausal: bool,       // Modelica: true. FMU-CS: false.
    pub supports_events: bool,
    pub supports_rollback: bool,
    pub supports_live_swap: bool,       // true = hot fidelity swap without respawn
    pub supports_dynamic_insert: bool,  // true = add/remove mid-run
    pub native_solver: bool,            // false = needs external integrator (FMU-ME)
}
```

### Participants are ECS entities

A `Participant` trait instance maps to a Bevy entity tagged with a backend
marker component (`ModelicaModel`, `AvianSim`, `FmuParticipant`, …). The
trait is the *contract*; ECS queries + domain-owned FixedUpdate systems do
the work. The trait exists so scenario loaders can materialise participants
without knowing which domain crate implements them.

This means the BackendRegistry is for **discovery and fusion**, not per-step
dispatch. Each backend's step systems already iterate their own components
on their own schedule — same as today.

## Island partitioning + acausal connections

Scenario connections are typed:

```rust
pub enum ConnectionKind {
    Causal,              // output → input (signal)
    Acausal,             // Modelica connect, FluidPort, Flange, Pin, Flange
}
```

Acausal connections are Kirchhoff-style — flows sum to zero at the node,
potentials equate. They **cannot cross cosim boundaries** without introducing
a fake algebraic loop and losing accuracy.

At Run start, the **IslandPartitioner** groups participants into islands:

1. Union-find over participants connected by acausal edges.
2. For each island, require all members share a backend that advertises
   `can_absorb_acausal`. If not, scenario load fails with a clear error.
3. Ask the backend to `fuse()` the island into one `Participant` (Modelica
   code-generates a wrapper `.mo` that replicates the connections as
   `connect()` equations, then compiles once).
4. Inter-island connections must be causal; they remain as `SimConnection`
   at runtime and are propagated by the master loop.

Result: three Modelica components wired by acausal `FluidPort`s become one
flattened DAE with one stepper (Dymola's default). A Modelica + Python
mix becomes two islands bridged by causal signals (classical cosim). Users
can opt a participant out of fusion with `explicit_boundary = true` for
debugging.

## Multi-clock + fidelity (forward-compatible)

A Scenario may declare multiple named `Clock`s (fast / slow / wall) and
pin each participant to one. A Participant may declare a `fidelities[]`
bundle — multiple implementations sharing a `StatePortrait`. A
`FidelityPolicy` picks between them based on user focus, time-warp, CPU
budget.

**MVP uses exactly one clock, one fidelity per participant.** The scenario
schema reserves `clock_id: "main"` and `fidelities: [default]` from v1 so
adopting either isn't a migration. Full design in
[`15-adaptive-fidelity.md`](15-adaptive-fidelity.md).

## Run — the session abstraction

```rust
pub struct Run {
    pub id: RunId,
    pub scenario_id: ScenarioId,
    pub status: RunStatus,     // Idle | Running | Paused | Completed | Failed
    pub mode: RunMode,         // Live | Batch | Stepped | Replay
    pub rate_factor: f64,      // time-warp; 1.0 = real-time
    pub t_start: f64,
    pub t_end: Option<f64>,    // None = open-ended live session
    pub current_t: f64,
    pub seed: u64,
    pub trace: TraceHandle,
    pub input_log: Vec<TimestampedInput>,
    pub wall_start: Instant,
}
```

### Lifecycle

```
Idle → [▶] → Running ⇌ [⏸ / ▶] Paused
              │
              ├── [⏹ or t ≥ t_end] → Completed
              ├── [verifier FAIL] → Failed
              └── [error]         → Failed
                       │
                       ▼
               archived as <twin>/runs/<id>/
```

### Master tick (FixedUpdate, `status == Running`)

1. Drain `InputQueue` → apply to participants at `current_t` (log each input).
2. Collect outputs from every island.
3. Propagate inter-island `SimConnection`s (causal only).
4. Set inputs on every island.
5. `for each island: participant.step(dt)` where `dt = rate_factor * FixedUpdate.dt`.
6. `current_t += dt`.
7. Append sample to trace.
8. Evaluate verifier; if FAIL → `Failed`.
9. If `t_end.is_some() && current_t >= t_end` → `Completed`.

### Modes

Only **when step 1–9 fires** differs between modes:

| Mode    | Step trigger                          | Trace               | Inputs                      |
|---------|---------------------------------------|---------------------|-----------------------------|
| Live    | Every FixedUpdate tick                | Ring + checkpoints  | Real-time user / HIL / net  |
| Batch   | Tight loop, as fast as CPU allows     | Full in memory      | Pre-recorded log only       |
| Stepped | One click → one tick → Paused         | Full in memory      | User per-click              |
| Replay  | Every FixedUpdate tick                | Full, recomputed    | From stored `input_log`     |

Same `Run` struct, same participants, same pipeline. Mode picks who
produces inputs and when steps trigger.

### Time-warp is not a new Run

Changing `rate_factor` mid-session continues the same Run with different
pacing. If fidelity can't keep up at high warp, `FidelityPolicy` swaps
participants to coarser models. Scenario + Run unchanged.

### Pause vs Reset

**Pause** freezes sim time. Participants stop stepping. Wall time continues.
Parameter edits queue and apply on Resume (Tunable semantics). Camera, UI,
inspection all work.

**Reset** bumps session id, sends `ParticipantCommand::Reset` to every
island, clears `trace` and `input_log`, resets `current_t` to `t_start`,
moves status to Idle. Hit ▶ again to start from zero.

### Trace storage

Realtime Runs can last hours. Three-tier scheme:

1. **In-memory ring buffer** — last ~10 min, for live plots.
2. **Periodic checkpoints** — every ~30s under `<twin>/runs/<id>/checkpoints/`.
   Enables scrub-back + replay from arbitrary t.
3. **Streaming MCAP** — optional continuous log for full recordings
   (training, flight tests). Spec 020 describes the format.

MVP uses only (1), in memory, as `Vec<Sample>`. (2)+(3) come with spec 020
implementation.

### Input log = deterministic replay

All mid-flight interventions (`SetInput`, `SetParam`, `SwitchFidelity`,
fault injection, admin commands) land as timestamped events in
`InputQueue`. Master drains at step 1 of each tick and appends to
`input_log`. Replay mode feeds the stored log back into an empty Run with
the same seed → bit-identical trace (spec 006 FR-002).

## MVP default: one implicit Twin + one implicit Run

Minimum viable scope for the Modelica MVP:

- **One implicit Twin** materialised on workspace open (the workspace folder
  becomes the Twin root). Holds no explicit scenarios yet.
- On first successful Compile, Twin spawns one implicit **Scenario** (the
  open doc + its Avian body, if any) and one implicit **Run** for it:
  `mode = Live, t_end = None` (or set from `experiment(...)` annotation —
  task #74), `rate_factor = 1.0`, `status = Running`.
- Toolbar ▶ / ⏸ / ⟲ emit Bevy events that the Twin observer translates
  into `TwinCommand::{PauseRun,ResumeRun,ResetRun}` in the
  `pending_commands` queue. `drain_twin_commands` applies them; observers
  short-circuit to `ModelicaModel.paused` under the hood — the existing
  stepper keeps working.
- Trace is an in-memory `Vec<Sample>` on the Run, appended on each worker
  response.
- Telemetry + Graphs read the live trace. No disk persistence yet.
- No explicit Scenario artefact on disk — the implicit scenario is
  regenerated from the open doc each time.
- No BackendRegistry refactor — one backend, hard-coded, but the
  TwinCommand surface is real and forward-compatible.

All forward-compatible: every command the UI fires is already a TwinCommand
conceptually; BackendRegistry, persisted scenarios, partitioner, FMU, DCP,
multi-Twin each land as additive extensions without changing the command
surface or the MVP Run shape.

## Networking — server-authoritative, per spec 022

Multi-user use is out of scope for MVP but the shape is fixed:

- **Server** runs the authoritative `Run`. Full set of backends registered.
- **Client** registers lightweight replica backends (Avian interpolation,
  optional Modelica shadow). Renders replicated components; injects input
  events over the network.
- `SetInput` from any client networks to the server's `InputQueue`. Spec 005
  (multiplayer-core) owns the transport.
- Late join: server streams nearest checkpoint + tail of `input_log` +
  `current_t`; client's replicas catch up.
- Per-participant input RBAC lives in spec 010.

## Implementation status

- **Done today**: single implicit Run (unnamed), `FixedUpdate` stepping,
  `ModelicaModel.paused` = Pause, `SimConnection` = causal-only wires,
  Avian + Modelica share a schedule, worker thread for rumoca.
- **Task #75**: Run-control toolbar — surfaces the implicit Run as
  ▶ / ⏸ / ⟲ + time pill.
- **Task #74**: read `experiment(StopTime=…)` → feeds `Run.t_end`.
- **Task #70 / #59**: named runs + comparison — introduces persistent
  `<twin>/runs/<id>/`.
- **Task #94**: four-layer formalisation (this doc).
- **Task #95**: BackendRegistry + Twin-owned scenarios.
- **Task #96**: multi-clock + fidelity (LoD) — design in
  [`15-adaptive-fidelity.md`](15-adaptive-fidelity.md).

Each of these is additive. The MVP Run struct lives in `lunco-modelica`
for now; extracting to `lunco-cosim` happens when FMU / other backends
arrive.

## Decision log

Summarised from the architecture discussion — capture the reasoning so
future contributors don't relitigate:

1. **Twin is the control surface, not a passive container.** Every
   command (start, pause, reset, step, warp, switch-fidelity, set-input)
   is a `TwinCommand` dispatched through Twin. UI, HTTP, scripts, remote
   clients, replay all go through the same command queue.
2. **Scenario + Run are separate, not merged**. One scenario → many runs.
3. **Twin owns scenarios + past runs**. No loose scenario files.
4. **Backends self-register** via Bevy plugins + `BackendRegistry`. Adding
   FMU / Python / GMAT is a crate, not a core patch.
5. **Participants are ECS entities** with backend marker components. Trait
   is a discovery contract, not a dispatch layer.
6. **Acausal connections must stay in one island**. No silent fallback to
   causal — hard error.
7. **Partitioner runs at Run-start**, caches the flattened DAE per scenario
   revision. Matches Dymola's "translate then simulate" workflow.
8. **Same Run, same mode, same pipeline regardless of realtime / batch /
   stepped / replay**. Mode picks who triggers steps and who produces
   inputs — nothing else.
9. **Time-warp is a `rate_factor` on the Run, not a new Run**. LoD picks
   up the slack when CPU-bound.
10. **USD is the 3D scene, not the scenario**. Linkage via
    `customData.luncosim:participant_id` (or a future USD schema).
11. **Server is authoritative**. Clients register replica backends; remote
    control is just another TwinCommand producer.
12. **MVP has one implicit Twin + one implicit Run per document**. The
    command surface is real from day one; persisted scenarios, partitioner,
    FMU, multi-Twin arrive as additive extensions.
