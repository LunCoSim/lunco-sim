# 25 — Experiments — Spec

> Status: Active · Audience: contributors running parameter sweeps & batch simulations
>
> **TL;DR.** `lunco-experiments` runs a model many times over swept parameters
> and collects results — the engine behind lunica's batch/sweep runs. Parallel
> execution is [§ Parallel execution](#parallel-execution) below.

**Implemented.** `lunco-experiments` shipped — `Experiment`, `RunResult`,
`RunStatus`, `ExperimentRegistry`, `ExperimentRunner` (trait), `ExperimentsPlugin`,
with `lunco-modelica-runner` providing the `ModelicaRunner` backend,
`lunco-modelica-worker` providing the stateful worker engine, and
`lunco-modelica-execution` providing host lifecycle and transport.
Owner: lunica/modelica.
Related: `13-twin-and-workflow.md`, `14-simulation-layers.md`, `22-domain-cosim.md`, `30-wasm-web-worker.md`.

## Goal

The experiments framework supports:

1. Running a model from `t_start` to `t_end` as fast as possible (batch / "Fast Run"), in addition to the existing realtime-stepped Interactive run.
2. Treating each run as a first-class artifact with its own parameters, bounds, and trajectory.
3. Comparing trajectories from multiple runs on a shared plot.

## Rationale

### Why two run modes
Live cosim drives 3D viz, possession, and twin coupling at wall-clock pace. That's the right model for inspection and physics-in-the-loop work. It is the wrong model for parametric study, regression checks, and "what changes if I bump this constant?" — those need batch execution that finishes in seconds, not minutes. Rumoca's `simulate()` is already this. Lunica just hasn't surfaced it.

### Why experiments as a first-class object
Dymola and OMEdit treat results as `.mat` files keyed by model name; comparison happens by opening multiple files. Wolfram SystemModeler and Simulink-SDI treat each run as a named entity with its own parameters. The latter scales better for iterative engineering work because (a) the user doesn't manage filenames, (b) parameter overrides live next to results, (c) the comparison UI is the default view rather than a side door.

### Why backend-agnostic
Today the only execution backend is rumoca + diffsol. The crate boundary should not assume that. FMU import, codegen, hardware-in-the-loop, and remote workers are all plausible v2+ extensions. Putting `Experiment` and `RunResult` in a backend-agnostic crate keeps the door open without committing to any of those.

### Why DAE-level overrides
The runner applies scalar parameter and input values to the compiled DAE after
one clean compile. A sweep therefore reuses the same compiled artifact without
mutating authored source or reflattening every point. Unsupported bindings are
reported as run errors at the runner boundary.

### Why the Web Worker uses postMessage, not SAB
The wasm host has no COOP/COEP headers and the worker is intentionally a separate wasm instance (see `30-wasm-web-worker.md`). Adding SAB requires header changes and nightly atomics. Cancellation latency of <100 ms via message polling is acceptable for human-driven Fast Runs.

## Crate layout

```
lunco-experiments/        (backend-agnostic)
  Experiment, RunResult, RunBounds, ParamValue, ParamPath
  ExperimentRegistry  (Resource, presentation-grouped history)
  ExperimentOrigins   (Resource, immutable runtime admission attribution)
  ExperimentRunner    (trait)
  messages: RunRequested, RunProgress, RunCompleted, RunFailed, RunCancelled, ExperimentRemoved

lunco-experiments-ui/     (backend-agnostic view state)
  ExperimentVisibility, PlotPanelStates, ActivePlot
  ExperimentsViewModel and change-gated trajectory cache

lunco-modelica-compiler/
  ModelicaCompiler and source-root admission

lunco-modelica-core/
  document and compiler-engine lifecycle synchronization

lunco-modelica-runner/
  ModelicaRunner: ExperimentRunner
  owner-scoped compile-once DAE cache and DAE-level value bindings
  shared batch/interactive run paths and run-bound resolution

lunco-modelica-worker/
  stateful Modelica worker engine and live co-simulation bridge
  native worker loop, command dispatch, and worker-local caches

lunco-modelica-execution/
  native worker launch and wasm WebWorkerTransport
  typed callback installation for the runner's wasm dispatch seam

lunco-modelica-ui/
  Run buttons + experiment table + bounds inline UI

lunco-modelica-execution/src/bin/lunica_worker.rs
  + WireMessage::RunFast / CancelRun
  + WireResult::RunUpdate
  MSL/compile readiness gate extended

lunco-twin/, lunco-twin-journal/      unchanged in v1
lunco-cosim/                          unchanged (Interactive path)
lunco-modelica-ui/                   Modelica experiment adapters and panel
lunco-viz/                            Shared multi-series trajectory renderer
lunco-viz-core/                       Render-free visualization identifiers
```

`lunco-modelica-runner` depends on `lunco-experiments` and
`lunco-modelica-compiler`;
`lunco-modelica-execution` composes the runner and worker engine with platform
transport. The generic
`lunco-experiments` package does not depend on either Modelica package or
`rumoca-*`.

### Why a new crate (vs. inside lunco-twin)
`lunco-twin` today is folder + manifest + file classification. It has no simulation deps. Pulling rumoca-sim deps in to host experiments would expand its scope significantly. A sibling crate keeps lunco-twin lean and lets future twin work (possession, scenarios) compose with experiments rather than nesting under them.

## Data shapes

```rust
pub struct ExperimentId(Uuid);

pub struct Experiment {
    pub id: ExperimentId,
    pub twin_id: TwinId,
    pub model_ref: ModelRef,            // opaque to lunco-experiments
    pub name: String,                   // auto: "<model> — N", user-editable
    pub overrides: BTreeMap<ParamPath, ParamValue>,
    pub inputs: BTreeMap<ParamPath, ParamValue>,
    pub bounds: RunBounds,
    pub status: RunStatus,
    pub result: Option<Arc<RunResult>>,
    pub created_at: SystemTime,
    pub color_hint: u8,
}

pub struct RunBounds {
    pub t_start: f64,
    pub t_end: f64,
    pub dt: Option<f64>,                // output interval; None -> derived grid
    pub n_intervals: Option<u32>,        // output interval count; wins over dt
    pub tolerance: Option<f64>,
    pub solver: Option<SolverId>,        // registered solver demand
    pub h0: Option<f64>,                 // solver initial step hint
    pub runtime: RuntimeMode,            // Batch (default) or Interactive
}

pub enum RunStatus {
    Pending,
    Queued,
    Running { t_current: f64 },
    Done { wall_time_ms: u64 },
    Failed { error: String, partial: bool },
    Cancelled,
}

pub struct RunResult {
    pub times: Vec<f64>,
    pub series: BTreeMap<String, Vec<f64>>,   // dotted Modelica path -> samples
    pub meta: RunMeta,
}

pub struct ParamPath(pub String);             // "rocket.engine.thrust"

pub enum ParamValue {
    Real(f64),
    Int(i64),
    Bool(bool),
    String(String),
    Enum(String),                              // enumeration literal name
    RealArray(Vec<f64>),
}
```

The registry retains each trajectory through one immutable `Arc<RunResult>`.
Readers clone the Arc to capture the exact result without copying its series;
streaming merges use copy-on-write, so an admitted snapshot stays unchanged.
Complete trajectories enter the registry through `set_complete_result`, which
validates finite, nondecreasing times, matching series/metadata lengths, finite
series values, and the configured scalar-value budget. Equal event timestamps
remain valid. Failed partial trajectories use `set_partial_result`; streaming
hole padding remains explicit and is not mistaken for a complete result.

`lunco-experiments` owns the persisted Bevy `ExperimentSettings` section
(`experiments`). Its `result_limits` defaults to 8,000,000 scalar values and
256 MiB of artifact bytes. `RunResultLimits` is also available to Bevy-free
hosts. Zero limits and dimension overflow are errors. The completed-run channel uses the scalar limit captured in its immutable
source and pending handle at run admission; later settings edits do not alter
that result boundary. The artifact loader validates against its read admission
limits; optional invalid artifacts emit a diagnostic without
publishing their trajectory as a successful result.

Each native run and browser `RunFast` request carries those captured limits.
Batch execution lowers once, admits the actual `SolveModel.visible_names`
dimensions against the maintained output-grid iterator, then passes that model
to `simulate_solve_model`. The scoped solver budget checks every central
visible-sample append, including additional event samples, and reserves storage
fallibly before changing column lengths. The recorded time vector shares the
same boundary. Interactive runs admit their actual state columns and check
each retained output sample. Budget failure is a terminal Failed update and
releases the scheduler slot; nested solves restore the caller's budget.


Registry: `BTreeMap<TwinId, Vec<Experiment>>` retains at most 20 terminal runs per presentation group, evicting the oldest terminal row. Pending, queued, and running rows are retained. Removal publishes `ExperimentRemoved` so source attribution and plot selections retire together.

### Runtime attribution and retained history

`ExperimentOrigins` owns the origin of each registered UUID. Local work records
`PinnedDocumentRuntimeOwner`; replicated work records the exact connection and
scene lifetime. Registration and definition replay validate row and origin
together. A conflicting origin or execution definition is rejected before
mutation. An identical `Create` is a no-op and preserves later presentation
edits, status, and results. Bounds, overrides, inputs, and class references may
change only while a row is `Pending` and has no result; API/UI admission marks
the row `Queued` before dispatching its immutable job. Identical bounds or
parameter replay remains a no-op in every state. Names and colors are
presentation metadata and may change independently of execution history.

`PendingRun` captures the origin alongside its handle. Progress, completion,
failure, and cancellation messages carry that immutable origin, and consumers
validate it before changing the registry or publishing plots and playback.
Retained rows do not regain runtime ownership when another Twin becomes active.
Closing or removing their owner retires unfinished work and its receivers;
explicit deletion and bounded history eviction also remove origin records.

### Why BTreeMap for overrides and series
Deterministic ordering for display, plot legend stability, and reproducible result hashes. Cost is negligible at the volumes involved.

## Runner trait

```rust
pub trait ExperimentRunner: Send + Sync {
    type Source: Send + 'static;
    fn run_fast(&self, exp: &Experiment, source: Self::Source) -> RunHandle;
}

pub struct RunHandle {
    pub progress_rx: crossbeam_channel::Receiver<RunUpdate>,
    pub run_id: ExperimentId,
    pub cancel: Box<dyn Fn() + Send + Sync>,
}

pub enum RunUpdate {
    Progress { t_current: f64, delta: Option<RunResult> },
    Completed(RunResult),
    Failed { error: String, partial: Option<RunResult> },
    Cancelled,
}
```

Fast Runs use the bounded scheduler and `experiments.max_parallel` setting described in [Parallel execution](#parallel-execution).

### Bounds admission

`lunco-modelica-core::sim_target::validate_run_bounds` validates the output
grid before UI/API registration, scheduler admission, or solver allocation.
The horizon must be finite and strictly increasing, and explicit `dt`,
`tolerance`, and `h0` must be finite and positive. Explicit interval counts
must be positive and at most the existing `SAMPLE_CAP` of 200,000 intervals
(200,001 inclusive points). An interval must advance time at the requested
endpoints. The browser's derived internal solver cadence passes the same
advancement guard before stepping. Invalid or over-limit requests return the owning diagnostic; the
runner never substitutes or enlarges an explicit interval.

The current AST annotation reader is fallible. `Interval=0` means omitted
spacing; other invalid values fail visibly. A used `NumberOfIntervals` must
be a finite positive integer within the same limit. An explicit annotation
`Interval` takes precedence, so an unused count is ignored. Absent spacing
uses the Modelica default of 500 intervals. `RunExperiment` returns rejection
without inserting an experiment, and direct runner callers receive a terminal
`Failed` update. The shared native/wasm execution boundary validates again
before invoking the solver.

## Web Worker protocol

`lunica_worker.rs` consumes the immutable admitted source through the transport-owned envelope:

```rust
WireMessage::RunFast {
    run_id: ExperimentId,
    model_name: String,
    source: String,
    filename: String,
    extras: Vec<(String, String)>,
    overrides: BTreeMap<ParamPath, ParamValue>,
    inputs: BTreeMap<ParamPath, ParamValue>,
    bounds: RunBounds,
    result_limits: RunResultLimits,
}
WireMessage::CancelRun { run_id: ExperimentId }

WireResult::RunUpdate { run_id: ExperimentId, update: RunUpdate }
```

Encoding: bincode, same as existing messages. Progress throttled to ~10 Hz wall clock. Cancellation polled between solver steps.

### Why reuse the worker instead of spawning a sim worker
Compiler and DAE state already live in this worker. A second worker would duplicate compile cache, double the WASM bundle, and require routing logic. The trade-off is that other worker commands queue behind a long Fast Run, which is managed via UI busy indicators.

## UI

### Build / model toolbar

```
[ Interactive ▶ ]   [ Fast ⏩  0 → 10s, dt=auto ⚙ ]
```

Bounds beside the Fast button reflect the current document AST annotation; when no
annotation provides a horizon, the documented run-bound defaults are used.
Inline-editable. Gear opens override editor.

### Experiments panel (new dock)

```
┌ Experiments ──────────────────────────────┐
│ ☑ ● rocket — 1     0..10s   Done    1.2s  │
│ ☑ ● rocket — 2     0..10s   Done    1.3s  │
│ ☐ ● rocket — 3     0..30s   Failed       ⓘ│
│ ☑ ● rocket — 4     0..10s   ▮▮▮▮▱▱ 4.2s   │
└───────────────────────────────────────────┘
```

Checkbox toggles plot visibility. Color dot is locked to run id. Loading or rerunning a row requires its pinned source document and runtime owner to remain in the active scope; its overrides and bounds apply only to that document's model draft. Cancel is available for queued and running rows.

### Override editor

Table of detected top-level literal parameters with current values + override fields. Params with non-literal bindings appear greyed with "complex binding — override unsupported in v1" tooltip.

### Graphs panel

Existing variable picker is shared across experiments. Each picked variable plots once per checked experiment. Legend: `<exp name> · <var path>`.

The Modelica adapter prepares the completed-run buffers and labels; the
reusable multi-series trajectory renderer lives in `lunco-viz` alongside the
live `LinePlot` kind. It owns legends, run/variable stroke styles, log-Y
formatting, fit/reset, overlays, and scrub interaction. This keeps those UI
semantics available to other experiment or co-simulation backends without
making `lunco-viz` depend on Modelica.

The shared selection state and trajectory cache live in
`lunco-experiments-ui`. They are intentionally separate from the Modelica
panel: Telemetry, Graphs, canvas snapshots, and future experiment backends
can use the same state without making the reusable package depend on document
resolution or Modelica setup. The host resolves its current document to a
`TwinId` and calls the cache producer; the package never guesses that scope.

The experiment view model also retains the sorted variable groups and each
series' positivity summary. The plot widget converts immutable experiment or
overlay samples to shared `PlotPoint` buffers on the async-compute pool and
reuses them while the source identity, log-Y mode, and display width stay
unchanged. Long time-series are min-max decimated to about one sample per
logical display point before drawing, preserving narrow peaks. Cached
full-data bounds keep egui's auto-fit pass from rescanning every sample on
every repaint. While a replacement buffer builds for the same stable series
and log-Y mode, the plot keeps drawing its last completed buffer. Plot items
borrow those buffers; they do not clone the full sample vectors per frame.
For time-sorted series, pointer hover searches only segments whose X interval
can beat the nearest segment found so far; phase-space plots retain the general
search because their X values may reverse. Screen-space line geometry is still
rebuilt for display.

## Durable completed history

The optional artifact adapter in `lunco-luncosim-services` admits work from
`RunCompleted`, `TwinAdded`, and the actual materialized remote Twin manifest.
Each operation captures its exact runtime owner, canonical storage root, result
limits, immutable definition and trajectory. Application runs have no Twin
artifact destination. Closing that owner retires its queued work and callbacks;
late outcomes cannot populate a replacement Twin. A durable write announces a
new manifest only after Storage reports success.

`lunco-experiments` owns the versioned JSON envelope, decoded scalar validation,
and bounded serializer. Neither raw Workspace Twin IDs nor filesystem roots are
persisted. A compiler-owned CID describes the actual successful strict target
and participating source contributions before user overlays are cleared.
Host paths and mount IDs do not contribute. A participating parsed-only library
without source bytes is explicitly nonpersistable; the valid simulation result
remains available. Unused libraries do not affect the CID.

Native codec and Storage work run through bounded background admission. Browser
JSON encoding and decoding run in the existing execution Web Worker. A bounded,
once-consumed lease retains the already received transferable completion buffer;
the UI sends only its small typed header back for encoding. The worker checks
the exact run, source CID, byte budget and scalar budget. Existing typed worker
transport still copies and decodes numerical buffers when receiving a result;
this adapter does not remove that transport cost.
The runner's exact runtime and removed-run retirement also drops these leases
in standalone Modelica hosts that do not compose the artifact writer.

Storage lists at most the registry's 20 retained lexical entries and reads each
candidate under the captured byte cap. Malformed or missing optional history
warns without failing authored loading or a valid run. Rehydration uses the same
bounded registry, an explicit archived runtime origin, and `restored_history`
metadata; it emits no live completion or playback. Same-UUID definition, owner
or compiled-source conflicts reject. Archived history is queryable without
pretending it came from the currently edited document. A remote Application
manifest without an actual admitted Twin mount has no history storage owner.
An existing replicated row must already be terminal before its artifact can
attach. Replaying a historical Create as Pending does not prove execution;
without an admitted terminal status that row remains unavailable as history.
Private runtime-state persistence retains its independent load/save policy.
Native background operations reuse the shared asset root-confinement check for
the directory and file; existing symlinks cannot route optional reads or writes
outside the admitted root. This is a confinement check at preparation, not a
race-free guarantee against concurrent hostile filesystem replacement.

Verify the pure codec, compiler contribution CID, bounded directory iterator
and asynchronous route seams, then run `modelica_artifact_history.rhai` in two
fresh owned sessions: seed a completed run and verify its UUID artifact is
durable before restart. Seed uses `CreateTwin` on a fresh root with an existing
API-authored entry scene; restore uses `OpenTwin` on the saved Twin. Restore
with a corrupt optional neighbor, then run
the same class name from changed source and compare CIDs and values.

## Future enhancements

- Parameter sweep grid UI
- Diff metrics (RMS, max-error)
- Solver picker UI
- Variable include/exclude UI
- Interactive runs archiving into Experiments
- Override of inherited / expression-bound / array / record parameters

## Parallel execution

A sweep runs many points at once, bounded by one scheduler. Two things carry
most of the win, and both already exist — do not rebuild them.

**Compile once, sweep many.** `lunco-modelica-runner` caches the compiled `Dae`
keyed by source hash (`dae_cache`, `dae_cache_key`) and applies parameter
overrides at the DAE level (`apply_value_bindings_to_dae`) rather than reflattening
per run. A sweep that varies only top-level scalar parameters recompiles
**zero** times after the first point.

**Per-run demux.** Results route by exact `run_id` and admitted origin — native: one `crossbeam` channel
per `RunHandle`, drained by `drain_pending_handles`; wasm: the `RUN_SENDERS`
map in `worker_transport.rs`, forwarded by `forward_run_update`. Cancel is
per-run (native `AtomicBool`, wasm `CancelRun{run_id}`).

Replicated experiment definitions are replayed after the authenticated inbox is
drained. Run status is applied after that replay so a same-frame definition and
status cannot race their ownership admission.

### One bounded scheduler, one platform-specific spawn

`RunnerState` holds `{max_parallel, in_flight: HashSet, pending: VecDeque<QueuedJob>}`.
`run_fast` snapshots a `QueuedJob`, pushes it to `pending`, and calls
`pump_scheduler`, which starts jobs while `in_flight < max_parallel` — outside
the lock. On a terminal update `finish_run` frees the slot and re-pumps.
Cancellation withdraws queued jobs immediately. Each job owns the immutable
source text, document URI, extras, and exact typed runtime owner captured
at admission; execution never looks up a mutable class-name source map.
Compiled DAE entries include that owner; `TwinClosed` removes its entries,
and cancelled workers cannot republish them after retirement. User overlays
are released after compilation while admitted application libraries stay installed. The panel shows
"⏳ Queued"; the Run button queues rather than disabling.

Native solves install a scoped thread-local cancellation guard backed by the
admitted run flag. The pinned Rumoca solver and evaluator crates check
that flag at evaluation and simulation-driver boundaries. An executing numerical
kernel returns before its next checkpoint; cancellation prevents trajectory
publication and worker exit releases the scheduling slot.

Native thread admission is fallible. A worker panic becomes a terminal
`RunFailed` diagnostic, and its completion guard releases the scheduler slot
on every exit. A poisoned compiler fails subsequent compilation visibly.
The handle drain distinguishes a connected empty channel from a disconnected
worker: disconnect without a terminal result fails and retires the handle;
disconnect after completion or cancellation preserves that terminal result.
Poisoned scheduler state rejects queued and newly submitted runs with terminal
failures. The poison guard is used only to cancel and drain pending work; the
mutex stays poisoned and scheduling does not resume.

Spawning is the **only** `#[cfg]` split:

| | Native | Wasm |
|---|---|---|
| Primitive | fallible `std::thread::Builder::spawn` per run (fresh rumoca thread-locals) | persistent `WorkerPool`, reused across runs |
| Cap | `max_parallel`, default `available_parallelism() - 1` clamped `1..=4` | `max_parallel` clamped `1..=8` (`MAX_WORKERS = 8`) |
| Note | — | worker 0 is primary (parse/compile/MSL); Fast Runs prefer a free non-primary worker |

**Rayon does not oversubscribe.** rumoca uses one process-wide global rayon
pool (`available_parallelism() - 2`) shared by every concurrent compile, so it
self-bounds. Keep `max_parallel` modest anyway — each native run also carries
orchestration and result buffers. If profiling ever shows compile contention,
pre-initialise rayon's global pool from the app; do not patch rumoca.

### Known limits

- **Cold-sweep cache race** — two cache-miss runs of the same model can compile
  the same DAE concurrently. Harmless double work; dedup is optional.
- **The cap is global**, not per-model — one `max_parallel` across all sweeps.
- **Memory** — N concurrent runs hold N result buffers and N DAE clones; on
  wasm each worker also holds an MSL copy. The 20-run registry cap bounds
  retained results.
- **Changing the cap at runtime on wasm needs a page reload** to resize the
  pool — there is no retained MSL bundle to backfill newly installed workers.

## Future design considerations

- Determining if interactive runs should produce an experiment entry upon stopping.
- Journaling experiment definitions as undoable operations in `lunco-twin-journal`.
