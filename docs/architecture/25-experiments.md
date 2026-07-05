# 25 — Experiments — Spec

> Status: Active · Audience: contributors running parameter sweeps & batch simulations
>
> **TL;DR.** `lunco-experiments` runs a model many times over swept parameters
> and collects results — the engine behind lunica's batch/sweep runs. Parallel
> execution lives in `26-parallel-experiments.md`.

**Implemented.** `lunco-experiments` shipped — `Experiment`, `RunResult`,
`RunStatus`, `ExperimentRegistry`, `ExperimentRunner` (trait), `ExperimentsPlugin`,
with `lunco-modelica` providing the `ModelicaRunner` backend. Parallel execution
is covered in `26-parallel-experiments.md`.
Owner: lunica/modelica.
Related: `13-twin-and-workflow.md`, `14-simulation-layers.md`, `22-domain-cosim.md`, `26-parallel-experiments.md`, `30-wasm-web-worker.md`.

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

### Why string-injection overrides
String injection covers the parameter override surface (top-level literal `parameter` declarations) without requiring upstream modifications to rumoca. This is encapsulated behind the runner trait and can be updated as the API evolves.

### Why the Web Worker uses postMessage, not SAB
The wasm host has no COOP/COEP headers and the worker is intentionally a separate wasm instance (see `30-wasm-web-worker.md`). Adding SAB requires header changes and nightly atomics. Cancellation latency of <100 ms via message polling is acceptable for human-driven Fast Runs.

## Crate layout

```
lunco-experiments/        (backend-agnostic)
  Experiment, RunResult, RunBounds, ParamValue, ParamPath
  ExperimentRegistry  (Resource, per-twin)
  ExperimentRunner    (trait)
  events: RunRequested, RunProgress, RunCompleted, RunFailed

lunco-modelica/
  ModelicaRunner: ExperimentRunner
    cfg(target_arch="wasm32") -> WebWorkerTransport
    cfg(not(...))             -> ThreadTransport
  source-string override injector
  Run buttons + Experiments panel + bounds inline UI

lunco-modelica/src/bin/lunica_worker.rs
  + ModelicaCommand::RunFast / CancelRun
  + ModelicaResult::RunProgress / RunCompleted / RunFailed
  MSL/compile readiness gate extended

lunco-twin/, lunco-twin-journal/      unchanged in v1
lunco-cosim/                          unchanged (Interactive path)
lunco-viz/                            Graphs panel: multi-series from registry
```

`lunco-modelica` depends on `lunco-experiments`. `lunco-experiments` does not depend on `lunco-modelica` or `rumoca-*`.

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
    pub bounds: RunBounds,
    pub status: RunStatus,
    pub result: Option<RunResult>,
    pub created_at: SystemTime,
}

pub struct RunBounds {
    pub t_start: f64,
    pub t_end: f64,
    pub dt: Option<f64>,                // None -> adaptive
    pub tolerance: Option<f64>,
    pub solver: Option<String>,         // backend-defined
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

Registry: `HashMap<TwinId, Vec<Experiment>>` capped at 20 per twin, oldest-evicted on overflow (Done/Failed only; Pending/Running never evicted).

### Why per-twin scoping
Experiments tied to a workspace are expected. Switching twins should filter the list. Retrofitting later costs more than getting it right at the type level now.

### Why BTreeMap for overrides and series
Deterministic ordering for display, plot legend stability, and reproducible result hashes. Cost is negligible at the volumes involved.

## Runner trait

```rust
pub trait ExperimentRunner: Send + Sync {
    fn run_fast(&self, exp: &Experiment) -> RunHandle;
    fn default_bounds(&self, model: &ModelRef) -> Option<RunBounds>;
    fn cancel(&self, run_id: ExperimentId);
}

pub struct RunHandle {
    pub progress_rx: crossbeam_channel::Receiver<RunUpdate>,
    pub run_id: ExperimentId,
}

pub enum RunUpdate {
    Progress { t_current: f64 },
    Completed(RunResult),
    Failed { error: String, partial: Option<RunResult> },
}
```

The runner limits execution to one in-flight Fast Run per runner instance. Subsequent requests queue (FIFO) to keep native and WASM semantics identical, simplify UI state, and avoid resource contention.

## Web Worker protocol

Existing `lunica_worker.rs` is reused. New variants:

```rust
ModelicaCommand::RunFast {
    run_id: ExperimentId,
    model_ref: ModelRef,
    overrides: BTreeMap<ParamPath, ParamValue>,
    bounds: RunBounds,
}
ModelicaCommand::CancelRun { run_id: ExperimentId }

ModelicaResult::RunProgress { run_id, t_current, t_end }
ModelicaResult::RunCompleted { run_id, result: RunResult }
ModelicaResult::RunFailed   { run_id, error, partial: Option<RunResult> }
```

Encoding: bincode, same as existing messages. Progress throttled to ~10 Hz wall clock. Cancellation polled between solver steps.

### Why reuse the worker instead of spawning a sim worker
Compiler and DAE state already live in this worker. A second worker would duplicate compile cache, double the WASM bundle, and require routing logic. The trade-off is that other worker commands queue behind a long Fast Run, which is managed via UI busy indicators.

## UI

### Build / model toolbar

```
[ Interactive ▶ ]   [ Fast ⏩  0 → 10s, dt=auto ⚙ ]
```

Bounds beside the Fast button reflect annotation defaults from `CompilationResult.experiment_*` after the model's first compile, fallback `0..1, dt=auto` otherwise. Inline-editable. Gear opens override editor.

### Experiments panel (new dock)

```
┌ Experiments ──────────────────────────────┐
│ ☑ ● rocket — 1     0..10s   Done    1.2s  │
│ ☑ ● rocket — 2     0..10s   Done    1.3s  │
│ ☐ ● rocket — 3     0..30s   Failed       ⓘ│
│ ☑ ● rocket — 4     0..10s   ▮▮▮▮▱▱ 4.2s   │
└───────────────────────────────────────────┘
```

Checkbox toggles plot visibility. Color dot is locked to run id. Click row → load its overrides+bounds into the active model's draft. Cancel button on Running rows.

### Override editor

Table of detected top-level literal parameters with current values + override fields. Params with non-literal bindings appear greyed with "complex binding — override unsupported in v1" tooltip.

### Graphs panel

Existing variable picker is shared across experiments. Each picked variable plots once per checked experiment. Legend: `<exp name> · <var path>`.

## Future enhancements

- Disk persistence of experiments or definitions
- Parameter sweep grid UI
- Diff metrics (RMS, max-error)
- Solver picker UI
- Variable include/exclude UI
- Multiple concurrent runs
- Interactive runs archiving into Experiments
- Override of inherited / expression-bound / array / record parameters

## Future design considerations

- Determining if interactive runs should produce an experiment entry upon stopping.
- Journaling experiment definitions as undoable operations in `lunco-twin-journal`.
- Utilizing JavaScript transferable buffers (`Float64Array`) for WASM execution to optimize large results transfer.
