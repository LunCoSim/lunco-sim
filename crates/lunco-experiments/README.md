# lunco-experiments

Backend-agnostic **experiment / batch-run registry**.

An `Experiment` is one batch run of a model: a set of parameter overrides, a
`RunBounds` window, and (once finished) a `RunResult`. Experiments live in an
`ExperimentRegistry`, scoped per `TwinId` — the registry caps each twin at 20
runs and evicts the oldest finished run on overflow.

See `docs/architecture/25-experiments.md` for the design rationale.

## Key types

| Type | Role |
|------|------|
| `Experiment` / `ExperimentId` | one batch run + its stable id (`live()` = the interactive realtime cosim) |
| `ExperimentRegistry` | per-`TwinId` store of runs (capped at 20, LRU-evicts finished) |
| `RunBounds` / `RunStatus` / `RunResult` / `RunMeta` | the run window, lifecycle state, results (`merge_delta` for streaming), metadata |
| `ParamPath` / `ParamValue` | typed parameter overrides |
| `SolverChoice` | typed solver enum (`canonical` / `label` / `hover`) |
| `RuntimeMode` | realtime vs. as-fast-as-possible |
| `ModelRef` / `TwinId` | model + twin references |
| `ExperimentRunner` | trait — the pluggable simulation backend seam |

## Backend-agnostic

This crate has **no rumoca / Modelica dependency**. The simulation backend
plugs in via the `ExperimentRunner` trait; the Modelica binding lives in
`lunco-modelica` (`experiments_runner.rs`). Future backends (FMU, codegen,
remote) plug in the same way.

## Features

- `bevy` — exposes the registry as a Bevy `Resource`. Off → plain data types
  (serde-serializable; uses `web_time` so it builds on wasm).
