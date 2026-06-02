# Parallel experiment execution — implementation plan

Status: COMPLETE. Steps 1–5 DONE; step 4's rumoca rayon pin dropped as
unnecessary (see step 4 and Open questions). Companion to `25-experiments.md`.

## What already existed (do not rebuild)

- **Compile-once sweep.** `experiments_runner.rs` caches the compiled `Dae`
  keyed by source hash (`dae_cache`, `dae_cache_key`) and applies parameter
  overrides at the DAE level (`apply_overrides_to_dae`) instead of
  reflattening per run. A sweep that varies only top-level scalar params
  recompiles **zero** times after the first point. This is the main
  efficiency win and it is DONE (commit 6997). Parallelism is additive.
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

## Work breakdown

1. **[DONE] Scheduler in `RunnerState`** — `busy_with` reject-gate replaced by
   `{max_parallel, in_flight, pending}`; `pump_scheduler`/`finish_run` + a
   platform `start_job`. Dropped the unused `cancel_flag`. *Native parallel
   works.* (`experiments_runner.rs`.)
2. **[DONE] `max_parallel` from settings** — `ExperimentSettings` section
   (`settings.json` key `experiments`, `max_parallel: Option<usize>`, None/0 =
   auto). `default_max_parallel()` native = `available_parallelism()-1` clamped
   `1..=4`. Reactive `apply_experiment_settings` system applies on startup +
   edits. `set_max_parallel`/`max_parallel`/`in_flight_count`/`queued_count`
   exposed. Also fixed a pre-existing modifier-`=` override regex bug in
   `replace_param_literal`.
3. **[DONE] Wasm worker pool** — `worker_transport.rs`:
   `WORKER: OnceLock<WorkerHandle>` → `POOL: OnceLock<Mutex<WorkerPool>>`
   (`workers: Vec`, per-worker `running` occupant, `run_to_worker` map).
   `install_worker` sizes the pool from `experiments.max_parallel`
   (`load_section_from_disk`, clamped `1..=MAX_WORKERS=8`). Worker 0 = primary;
   `dispatch_run_fast` prefers a free non-primary worker, falls back to 0 when
   saturated; `forward_run_update` frees the slot on terminal;
   `dispatch_cancel_run` routes by `run_to_worker`; `install_msl_in_worker`
   installs into ALL workers (single keeps the zero-copy transfer, pool copies
   per worker). **Prereq fix:** the storage-crate merge dropped the wasm branch
   of `lunco_settings::load_section_from_disk` (hit `FileStorage`'s native-only
   `File` arm → `Default`); restored the `localStorage` branch to match
   `Settings::load_from_disk`. Verified by the wasm gate (`scripts/check_wasm.sh`
   flags). *Web parallel works* (runtime cap changes still need a reload to
   resize the pool — documented limitation).
4. **wasm auto-cap [DONE]; rayon pin [DROPPED — unnecessary]** —
   `default_max_parallel` wasm branch reads `navigator.hardwareConcurrency`
   (`-1`, clamped `1..=4`; tighter than native because each worker holds its own
   MSL copy). The rayon pin was dropped: rumoca uses a **single process-wide
   global** rayon pool (`rumoca-compile/src/parse.rs` `init_rayon_pool`:
   `ThreadPoolBuilder::new().num_threads(available_parallelism()-2).build_global().ok()`),
   not one pool per compile — so N concurrent compiles share that one bounded
   pool (no N×cores explosion), and work-stealing self-limits CPU use. The
   `.ok()` deliberately defers to any pre-existing pool, so if tuning is ever
   wanted we can `build_global()` from the app at startup (downstream, no rumoca
   edit). Compile-once caching makes concurrent cold compiles rare anyway (a
   sweep cold-compiles only the first point). No change shipped or needed.
5. **[DONE] Panel UI** — added `RunStatus::Queued` (registry-only, NOT on the
   `RunUpdate` wire → no worker changes); dispatch marks each run `Queued`,
   `drain_pending_handles` flips it to `Running` on first progress (or
   `Cancelled` if cancelled while queued). Experiments panel: `status_label`
   shows "⏳ Queued"; the setup header shows a live "▶ running/limit · ⏳
   queued" chip and the Run button *queues* instead of being disabled when
   busy. model_view ⏩ Fast toolbar button likewise no longer disabled when
   busy. Cancel-queued works via the per-row context-menu Cancel (Queued is
   non-terminal, and exempt from registry eviction). api_queries gained the
   `"queued"` state label. Exhaustive matches updated: api_queries ×2, panel
   ×1. Tests green: lunco-experiments 5/5, scheduler 8/8.

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
  pool from the app (no rumoca edit). Resolved 2026-06-02.
