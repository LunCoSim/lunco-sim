# Parallel experiment execution — implementation plan

Status: PLAN (not started). Companion to `25-experiments.md`.

## What already exists (do not rebuild)

- **Compile-once sweep.** `experiments_runner.rs` caches the compiled `Dae`
  keyed by source hash (`dae_cache`, `dae_cache_key`) and applies parameter
  overrides at the DAE level (`apply_overrides_to_dae`) instead of
  reflattening per run. A sweep that varies only top-level scalar params
  recompiles **zero** times after the first point. This is the main
  efficiency win and it is DONE (commit 6997). Parallelism is additive on
  top of it.
- **Per-run demux.** Results route by `run_id`:
  - native: one `crossbeam` channel per `RunHandle`, drained by
    `drain_pending_handles` (`PendingHandles` already a `Vec` — multi-handle
    capable today).
  - wasm: `RUN_SENDERS` map (`run_id → Sender`) in `worker_transport.rs`,
    forwarded by `forward_run_update`. Already multiplexed.
- **Cancel** is per-run (native `AtomicBool`; wasm `CancelRun{run_id}`).

So the substrate is parallel-ready. Three things block it:

1. The **artificial serial gate** (`busy_with: Option<ExperimentId>`,
   runner.rs:82,154-176) rejects the 2nd in-flight run instead of queueing.
2. **Native** spawns an unbounded thread per run (runner.rs:261) — no cap.
3. **Wasm** has a **single** worker (`WORKER: OnceLock<WorkerHandle>`,
   worker_transport.rs:181) — runs serialize inside it.

## Design: one bounded scheduler + a platform spawn primitive

### A. Shared bounded scheduler (platform-neutral) — the "limit parallel spawns"

Replace the boolean busy gate with a small scheduler living in `RunnerState`:

```
max_parallel: usize           // the cap
in_flight:    HashSet<ExperimentId>
pending:      VecDeque<QueuedJob>   // snapshotted run inputs
```

- `run_fast` → build the job; if `in_flight.len() < max_parallel`, **start**
  it; else **enqueue** and return a `RunHandle` whose `progress_rx` stays
  silent until it starts (registry status = `Queued`).
- On any terminal `RunUpdate` (the existing busy-clear sites:
  native thread-end runner.rs:263, wasm `pump_wasm_forwarders` runner.rs:351),
  remove from `in_flight`, pop `pending`, **start next**.
- Cancelling a *queued* job = drop from `pending` (free, never started).
  In-flight cancel unchanged.

This queue+cap logic is identical on both platforms. New registry status
`RunStatus::Queued` so the panel shows "3 running, 5 queued".

### B. Platform spawn primitive (the only `#[cfg]` split)

`start(job)` is the one thing that differs:

- **Native** (`#[cfg(not(wasm32))]`): `std::thread::spawn` per job, exactly
  as today — but the scheduler cap bounds live threads to `max_parallel`.
  Thread-per-run (not a reused pool) is the right call: a fresh thread gives
  fresh rumoca `thread_local` caches (clock/timeout) and avoids cross-run
  state reuse; spawn cost is trivial vs a multi-second sim. The shared
  `Arc<Mutex<RunnerState>>` already lets all run threads share `dae_cache`.
- **Wasm** (`#[cfg(wasm32)]`): a **persistent pool** of `max_parallel`
  workers. Extend `worker_transport.rs`:
  - `WORKER: OnceLock<WorkerHandle>` → `WORKERS: OnceLock<Vec<WorkerHandle>>`,
    each instantiating the worker wasm **once** at install and **reused**
    across runs (re-spawning a worker per run is the big web waste).
  - MSL is installed into **every** worker (loop `install_msl_in_worker` over
    the pool); the boot-race gate (`MSL_INSTALLED`, `PENDING_*`) becomes
    per-worker or "all-ready".
  - `dispatch_run_fast` picks an **idle** worker. The scheduler only
    dispatches when `in_flight < pool size`, so a free worker always exists;
    track which worker owns which `run_id` for routing + cancel.
  - `pump_commands_to_worker` (compile/parse path) keeps using worker[0] —
    pool is for RunFast fan-out only, to keep that change small.

### C. Picking `max_parallel` (auto, per platform, configurable)

- native: `std::thread::available_parallelism()` → `clamp(n-1, 1..=8)`.
- wasm: `navigator.hardwareConcurrency` (web-sys) → `clamp(n-1, 1..=4)`
  — lower, each worker is a full wasm instance (memory + browser budget).
- Expose as a `lunco-settings` value (`experiments.max_parallel`, default
  `auto`). **Default conservative** (this machine struggles → start at 2).

### D. Don't oversubscribe rumoca's inner rayon

Each rumoca compile uses rayon (`RAYON_INIT`). `max_parallel` concurrent
*first*-points (cache-cold) × rayon = N×cores threads → thrash. After
compile-once warms the cache, sweep points don't recompile, so the window is
small — but a cold sweep or distinct-model batch hits it. Mitigation: run the
inner compile single-threaded under the parallel executor (pin rumoca to a
1-thread rayon pool / `RAYON_NUM_THREADS=1` for run threads) and let the
**outer scheduler own core-level parallelism**. Outer = N models, inner = 1
thread ⇒ total ≈ cores. (rumoca change — coordinate; see
`feedback_no_unsolicited_rumoca_edits`. Until then: keep `max_parallel` low.)

## Work breakdown

1. **Scheduler in `RunnerState`** — replace `busy_with` with
   `{max_parallel, in_flight, pending}`; `run_fast` start-or-queue; terminal
   sites pop the queue. Add `RunStatus::Queued`. (`experiments_runner.rs`,
   `lunco-experiments/src/lib.rs`.) *Native parallel works after this step
   alone — thread-per-run already capped by `in_flight`.*
2. **`max_parallel` from settings** — `available_parallelism` /
   `hardwareConcurrency`, `lunco-settings` key, conservative default.
3. **Wasm worker pool** — `WORKERS: Vec`, install N, MSL into all, idle-worker
   pick in `dispatch_run_fast`, per-run→worker routing + cancel.
   (`worker_transport.rs`.) *Web parallel works after this step.*
4. **rayon pin** (rumoca, gated by ask) — inner compile single-threaded so
   outer parallelism doesn't thrash.
5. **Panel UI** — show running/queued counts; remove the "Fast Run busy"
   disable; allow cancelling queued jobs.

Steps 1–2 unlock desktop parallelism with the least risk; 3 unlocks web; 4 is
the efficiency polish; 5 is UX. Each is independently shippable.

## Open questions

- **Cold-sweep cache race:** two cache-miss runs of the same model can compile
  the same DAE concurrently (harmless double work). Optional: an "in-progress
  compile" dedup keyed by `dae_cache_key`. Skip for v1.
- **Per-model vs global cap:** start global. A distinct-model batch and a
  same-model sweep both honor one `max_parallel`.
- **Memory:** N concurrent runs hold N result buffers + N DAE clones. The
  20-run registry cap bounds retained results; in-flight RAM ≈ N × one run.
