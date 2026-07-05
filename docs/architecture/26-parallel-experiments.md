# 26 — Parallel experiment execution

> Status: Historical · Audience: contributors on batch/parallel experiment execution

Parallel experiment execution is companion to `25-experiments.md` and details the implementation of parallel fast run execution.

## What already existed (do not rebuild)

- **Compile-once sweep.** `experiments_runner.rs` caches the compiled `Dae`
  keyed by source hash (`dae_cache`, `dae_cache_key`) and applies parameter
  overrides at the DAE level (`apply_overrides_to_dae`) instead of
  reflattening per run. A sweep that varies only top-level scalar params
  recompiles **zero** times after the first point. This is the main
  efficiency win.
- **Per-run demux.** Results route by `run_id`:
  - native: one `crossbeam` channel per `RunHandle`, drained by
    `drain_pending_handles` (`PendingHandles` is a `Vec` — multi-handle).
  - wasm: `RUN_SENDERS` map (`run_id → Sender`) in `worker_transport.rs`,
    forwarded by `forward_run_update`.
- **Cancel** is per-run (native `AtomicBool`; wasm `CancelRun{run_id}`).

## Design: one bounded scheduler + a platform spawn primitive

### A. Shared bounded scheduler (platform-neutral) — the cap

`RunnerState` holds `{max_parallel, in_flight: HashSet, pending:
VecDeque<QueuedJob>}`. `run_fast` snapshots a `QueuedJob`, pushes to
`pending`, and calls `pump_scheduler` (starts while `in_flight < max_parallel`,
outside the lock). On terminal, `finish_run` frees the slot and re-pumps.
Queued-cancel is checked at `start_job`.

### B. Platform spawn primitive (the only `#[cfg]` split)

- Native: `std::thread::spawn` per run (fresh rumoca thread-locals), capped by
  `in_flight`.
- Wasm: a persistent pool of workers reused across runs; worker 0 is primary
  (compile/parse/MSL), Fast Runs prefer a free non-primary worker.

## Implementation details

Parallel experiment execution has the following characteristics:

### A. Bounded scheduler
The scheduler limits concurrency via `RunnerState` (`max_parallel`, `in_flight` set, and `pending` queue). It manages queued runs and processes them as slots become available.

### B. Native parallelism
On native platforms, parallel runs are spawned as separate threads (`std::thread::spawn`), capped by settings (`max_parallel`, default `available_parallelism() - 1` clamped to `1..=4`).

### C. Web Worker pool
On WASM, a persistent pool of web workers (`WorkerPool`) is size-configured based on `max_parallel` (clamped `1..=8`). Worker 0 acts as primary for parses and compiler operations, while Fast Runs prefer free non-primary workers.

### D. Rayon pool behavior
The execution engine shares a single process-wide global Rayon pool, avoiding CPU thread explosion. Concurrent compiles queue and cooperate on the single global pool, eliminating compile contention issues.

### E. UI Integration
The experiments panel displays a queued state ("⏳ Queued") and the toolbar/Run button queue additional runs instead of being disabled.


## Open questions / known limits

- **Cold-sweep cache race:** two cache-miss runs of the same model can compile
  the same DAE concurrently (harmless double work). Optional dedup later.
- **Per-model vs global cap:** global. One `max_parallel` across sweeps/models.
- **Memory:** N concurrent runs hold N result buffers + N DAE clones; on wasm,
  N workers each hold an MSL copy. The 20-run registry cap bounds retained
  results; `MAX_WORKERS=8` + the setting bound the worker count.
- **Runtime cap change on wasm** needs a page reload to resize the pool (no
  retained MSL bundle to backfill new workers post-install).
- **rumoca rayon oversubscription** — NOT an issue: rumoca's single global
  rayon pool (`available_parallelism()-2`) is shared across all concurrent
  compiles, so it self-bounds. Keep `max_parallel` modest anyway (default
  clamps `1..=4`) since each native run also carries orchestration + result
  buffers. If profiling ever shows compile contention, pre-init rayon's global
  pool from the app (no rumoca edit).
