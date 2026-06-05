//! Cross-thread Modelica worker transport for wasm32-unknown-unknown.
//!
//! Why this module exists
//! ----------------------
//! On native, `worker::modelica_worker` runs on its own OS thread and exchanges
//! `ModelicaCommand` / `ModelicaResult` over crossbeam channels with the Bevy
//! main loop. The blocking compile / step work never blocks the UI.
//!
//! On wasm32-unknown-unknown there are no OS threads. Until now the same code
//! path ran *on the main thread* via `worker::inline_worker_process`, so a
//! 20 s rumoca compile froze the page. This module replaces that path with a
//! Web Worker carrying a second wasm instance (`bin/lunica_worker.rs`). Bevy
//! systems still see the same crossbeam channels — only the bridge between
//! them and the worker changes.
//!
//! Wire format
//! ----------
//! Commands and results round-trip through `bincode::serialize` /
//! `bincode::deserialize`. `ModelicaCommand::Compile.stream` is `#[serde(skip)]`
//! because the underlying `Arc<ArcSwap<_>>` only makes sense in a single
//! address space; on wasm we always go through the per-Step result-message
//! path instead of the lock-free shared-snapshot fast path.
//!
//! Lifecycle
//! ---------
//! 1. The main wasm instance constructs a `web_sys::Worker` from the worker
//!    bundle URL and stores it via [`install_worker`] together with the
//!    `Sender<ModelicaResult>` end of the existing channel. JS-side, the
//!    worker's `onmessage` is wired to a wasm-bindgen-exported callback that
//!    pushes deserialized results into that sender.
//! 2. A Bevy system [`pump_commands_to_worker`] drains the existing
//!    `ModelicaChannels.rx_cmd`, bincode-encodes each command, and calls
//!    `Worker::post_message(Uint8Array)`.
//! 3. The worker bundle (`bin/lunica_worker.rs`) decodes the bytes, runs
//!    `worker::process_inline_command` against its local `InlineWorkerInner`,
//!    and posts each `ModelicaResult` back the same way.
//!
//! All wasm-only — `cfg(target_arch = "wasm32")` at the module level.

#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use bevy::prelude::*;
use crossbeam_channel::Sender;
use js_sys::Uint8Array;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{ErrorEvent, MessageEvent, Worker};

use crate::worker::{ModelicaChannels, ModelicaCommand, ModelicaResult};

/// Wire-format envelope for the postMessage transport.
///
/// We can't use the bare `ModelicaCommand` enum for everything because the
/// worker also needs out-of-band setup (notably MSL handoff: the main app
/// fetches and decodes the parsed MSL bundle, then ships the resulting
/// `Vec<(uri, StoredDefinition)>` to the worker so the worker's
/// `GLOBAL_PARSED_MSL` is populated before any compile arrives — without
/// this the worker's compiles would fail with `unresolved reference
/// Modelica.*`).
///
/// Keeping a single envelope means one bincode codec on each end and one
/// postMessage queue for the entire transport; the alternative
/// (multiplexing on a magic-byte prefix) is uglier and harder to extend.
#[derive(serde::Serialize, serde::Deserialize)]
pub enum WireMessage {
    /// Forward a Bevy-side `ModelicaCommand` to the worker for processing.
    /// 99 %+ of traffic is this variant.
    Command(ModelicaCommand),
    /// Install the pre-parsed MSL bundle into the worker's process-wide
    /// `GLOBAL_PARSED_MSL` slot. Sent once shortly after the main app's
    /// own MSL install lands. Worker uses this to seed
    /// `ModelicaCompiler::new`'s session before the first Compile.
    InstallParsedMsl(Vec<(String, rumoca_compile::parsing::StoredDefinition)>),
    /// Diagnostic round-trip — worker echoes back as a `WireResult::Log`.
    /// Used by the test bridge (`window.__lc_test_worker_ping`) to confirm
    /// the worker is alive and responding without sending an actual
    /// Modelica command.
    Ping(String),
    /// Parse a single document's source off the main thread.
    /// `engine_resource::drive_engine_sync` posts this when an open
    /// doc's source advances; the worker runs `parse_source_to_ast`
    /// in its own wasm instance and returns the resulting AST as a
    /// [`WireResult::ParseDocumentDone`]. UI thread receives it and
    /// installs into the engine session via `upsert_document_with_ast`.
    /// Eliminates the ~5 s rumoca freeze on AnnotatedRocketStage.
    ParseDocument {
        doc_id: lunco_doc::DocumentId,
        gen: u64,
        uri: String,
        source: String,
    },
    /// Fast Run request: compile (with overrides) + simulate end-to-end.
    /// Worker posts back a `WireResult::RunUpdate` stream tagged with
    /// `run_id`. See `experiments_runner` and
    /// `docs/architecture/25-experiments.md`.
    RunFast {
        run_id: lunco_experiments::ExperimentId,
        model_name: String,
        source: String,
        filename: String,
        extras: Vec<(String, String)>,
        overrides: std::collections::BTreeMap<
            lunco_experiments::ParamPath,
            lunco_experiments::ParamValue,
        >,
        #[serde(default)]
        inputs: std::collections::BTreeMap<
            lunco_experiments::ParamPath,
            lunco_experiments::ParamValue,
        >,
        bounds: lunco_experiments::RunBounds,
    },
    /// Best-effort cancel of an in-flight Fast Run. Worker observes
    /// the flag between solver steps. v1: cancel granularity is
    /// "between steps in the worker's run loop".
    CancelRun { run_id: lunco_experiments::ExperimentId },
}

/// Wire-format envelope from worker → main. Same multiplexing principle as
/// `WireMessage`: lets the worker emit out-of-band log lines that surface
/// in the main page's console (Web Workers have a separate console context
/// that's invisible to the page DevTools, so without this any worker
/// panic/error is silent).
#[derive(serde::Serialize, serde::Deserialize)]
pub enum WireResult {
    /// A normal `ModelicaResult` produced by `process_inline_command`.
    Result(ModelicaResult),
    /// Free-form diagnostic line — surfaced as `bevy::log::info!` on main.
    /// Used by the worker to expose its progress (which command arrived,
    /// how long it took, panic/recover) since the worker's own console is
    /// inaccessible from the page.
    Log(String),
    /// Parsed-AST result for a previously-sent
    /// [`WireMessage::ParseDocument`] request.
    ///
    /// `ast` is the lenient parser's best-effort result (always
    /// produced, even on broken sources). `errors` is the diagnostic
    /// list emitted by rumoca's recovery — empty when the source is
    /// well-formed. `gen` is the doc's generation at parse-spawn time
    /// so main can drop stale results.
    ///
    /// Both fields together replace the previous strict-style
    /// `Option<AST>`; merging them lets the receiver reconstruct the
    /// dual-cache state (now collapsed into a single `SyntaxCache`)
    /// in one shot.
    ParseDocumentDone {
        doc_id: lunco_doc::DocumentId,
        gen: u64,
        ast: rumoca_compile::parsing::StoredDefinition,
        errors: Vec<crate::document::ParseDiag>,
    },
    /// Lifecycle update for a Fast Run started via
    /// `WireMessage::RunFast`. The `run_id` lets the main thread
    /// demux to the right `RunHandle` receiver.
    RunUpdate {
        run_id: lunco_experiments::ExperimentId,
        update: lunco_experiments::RunUpdate,
    },
    /// The worker is reporting that its own wasm linear memory has grown past
    /// the recycle watermark (`payload` = current size in MB). wasm linear
    /// memory is GROW-ONLY — it never shrinks back, so a heavy run's footprint
    /// persists and accumulates across runs until the next one OOM-traps. The
    /// only way to reclaim it is to discard the whole worker instance, so the
    /// main thread respawns this worker once it's idle (see `handle_worker_error`
    /// / `respawn_worker`). Sent by the worker after a run completes.
    RecycleRequest { mem_mb: u32 },
}

/// One JS `Worker`, each carrying its own second wasm instance running the
/// off-thread Modelica pipeline.
///
/// `Worker` is `!Send + !Sync` because it carries a `JsValue`, but
/// wasm32-unknown-unknown is single-threaded so this is vacuously safe — we
/// only ever touch it from the main thread. The newtype lets us
/// `unsafe impl Send + Sync` so the pool can live in a `OnceLock<Mutex<_>>`.
struct WorkerHandle(Worker);
// SAFETY: wasm32-unknown-unknown has no threads. JsValue (and Worker) only
// live on the main thread; the Mutex/OnceLock require Send+Sync but we never
// cross threads in practice.
unsafe impl Send for WorkerHandle {}
unsafe impl Sync for WorkerHandle {}

/// Process-wide pool of Modelica workers (step 3 of the parallel-experiments
/// plan, `docs/architecture/26-parallel-experiments.md`).
///
/// Worker 0 is the *primary*: it always handles the compile / parse / MSL
/// path ([`pump_commands_to_worker`], [`dispatch_parse_to_worker`]). Every
/// worker — including 0 — can run a Fast Run, so a parameter sweep fans out
/// across the pool. To keep the primary responsive for compiles,
/// [`dispatch_run_fast`] prefers a free *non-primary* worker and only falls
/// back to worker 0 when all others are busy.
///
/// Pool size is fixed at [`install_worker`] from the persisted
/// `experiments.max_parallel` setting (auto = 1 on wasm); each extra worker
/// is a full wasm instance with its own MSL copy, so it's clamped hard.
struct WorkerPool {
    workers: Vec<WorkerHandle>,
    /// Per-worker Fast Run occupant (`None` = free for a Fast Run). The
    /// compile/parse traffic on worker 0 does NOT mark it occupied here.
    running: Vec<Option<lunco_experiments::ExperimentId>>,
    /// `run_id → worker index`, for cancel routing. Set on every dispatch
    /// (including the fall-back-to-0 case, where `running` isn't reassigned),
    /// cleared on the run's terminal update.
    run_to_worker: HashMap<lunco_experiments::ExperimentId, usize>,
}

/// Hard ceiling on pool size. Each worker is a full wasm instance + ~MSL
/// copy, so a runaway `max_parallel` setting can't exhaust browser memory.
const MAX_WORKERS: usize = 8;

static POOL: OnceLock<Mutex<WorkerPool>> = OnceLock::new();

/// The worker script URL, retained so a crashed worker can be respawned
/// in place (see [`respawn_worker`]). Set once at [`install_worker`].
static WORKER_URL: OnceLock<String> = OnceLock::new();

/// The serialized `InstallParsedMsl` wire bytes, retained so a respawned
/// worker can be re-seeded with the MSL index without re-fetching/parsing
/// the bundle. Set once when [`install_msl_in_worker`] first runs.
static MSL_WIRE: OnceLock<Vec<u8>> = OnceLock::new();

/// Respawned workers awaiting MSL re-seed, with the instant they respawned.
/// The MSL bundle (~165 MB) is deliberately NOT re-allocated on the crash
/// stack: right after a worker OOM the renderer is memory-starved and the
/// allocation throws `RangeError: Array buffer allocation failed`. We defer
/// it here and let [`pump_worker_respawns`] post it once the dead worker's
/// memory has been reclaimed (after a short settle delay).
static PENDING_RESEED: OnceLock<Mutex<Vec<(usize, web_time::Instant)>>> = OnceLock::new();

fn pending_reseed() -> &'static Mutex<Vec<(usize, web_time::Instant)>> {
    PENDING_RESEED.get_or_init(|| Mutex::new(Vec::new()))
}

/// True while worker `idx` has been respawned but not yet re-seeded with MSL.
/// Such a worker can't compile (its MSL index is empty), so the Fast Run
/// dispatcher skips it until `pump_worker_respawns` re-seeds it. A different
/// mutex from `pool()`, so it's safe to call while holding the pool lock.
fn is_reseed_pending(idx: usize) -> bool {
    pending_reseed()
        .lock()
        .map(|q| q.iter().any(|(i, _)| *i == idx))
        .unwrap_or(false)
}

fn pool() -> &'static Mutex<WorkerPool> {
    POOL.get_or_init(|| {
        Mutex::new(WorkerPool {
            workers: Vec::new(),
            running: Vec::new(),
            run_to_worker: HashMap::new(),
        })
    })
}

/// Post raw bytes to worker `idx`. No-op (with a warning) if the index is
/// out of range. Caller must NOT hold the pool lock (this takes it).
fn post_bytes_to(idx: usize, bytes: &[u8], label: &str) {
    let p = pool().lock().unwrap();
    let Some(WorkerHandle(worker)) = p.workers.get(idx) else {
        bevy::log::warn!("[worker_transport] {label}: worker {idx} not installed");
        return;
    };
    let array = Uint8Array::new_with_length(bytes.len() as u32);
    array.copy_from(bytes);
    if let Err(e) = worker.post_message(&array) {
        bevy::log::error!("[worker_transport] {label}: post_message failed: {e:?}");
    }
}

/// Serialize and post a `WireMessage` to worker `idx`.
fn post_msg_to(idx: usize, msg: &WireMessage, label: &str) {
    let bytes = match bincode::serialize(msg) {
        Ok(b) => b,
        Err(e) => {
            bevy::log::error!("[worker_transport] {label}: serialize failed: {e}");
            return;
        }
    };
    post_bytes_to(idx, &bytes, label);
}

/// Process-wide sender for `ModelicaResult` values arriving from the worker.
/// Set once at startup; drained by the existing
/// `worker::handle_modelica_responses` system through `ModelicaChannels.rx`.
static RESULT_TX: OnceLock<Sender<ModelicaResult>> = OnceLock::new();
/// Process-wide sender for `ModelicaCommand`s — same handle the Bevy
/// systems write to via `ModelicaChannels.tx`. Used by the
/// `__lc_test_dispatch_compile` JS bridge to fire commands without going
/// through the UI (for autonomous test loops).
static COMMAND_TX: OnceLock<crossbeam_channel::Sender<ModelicaCommand>> = OnceLock::new();

/// Process-wide channel carrying parse-done results back from the
/// worker into the main thread. The JS `onmessage` handler decodes
/// [`WireResult::ParseDocumentDone`] and pushes here; the Bevy system
/// `drain_worker_parse_results` (engine_resource.rs) drains each tick
/// and installs the AST into the engine session.
///
/// Crossbeam unbounded — parse-completion rate is well below tab-open
/// rate so it never grows.
pub struct ParseDoneEnvelope {
    pub doc_id: lunco_doc::DocumentId,
    pub gen: u64,
    pub ast: rumoca_compile::parsing::StoredDefinition,
    pub errors: Vec<crate::document::ParseDiag>,
}
static PARSE_DONE_TX: OnceLock<crossbeam_channel::Sender<ParseDoneEnvelope>> = OnceLock::new();
static PARSE_DONE_RX: OnceLock<crossbeam_channel::Receiver<ParseDoneEnvelope>> = OnceLock::new();

/// Per-run sender table for RunUpdate demux.
///
/// `WireResult::RunUpdate { run_id, update }` arrives at the JS
/// `onmessage` boundary; we look up the sender registered when
/// `dispatch_run_fast` was called and forward the update so the
/// `RunHandle.progress_rx` consumer (the experiments runner)
/// receives it transparently.
static RUN_SENDERS: OnceLock<
    std::sync::Mutex<
        std::collections::HashMap<
            lunco_experiments::ExperimentId,
            crossbeam_channel::Sender<lunco_experiments::RunUpdate>,
        >,
    >,
> = OnceLock::new();

fn run_senders()
    -> &'static std::sync::Mutex<
        std::collections::HashMap<
            lunco_experiments::ExperimentId,
            crossbeam_channel::Sender<lunco_experiments::RunUpdate>,
        >,
    >
{
    RUN_SENDERS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Register a sender for a Fast Run. Called by `ModelicaRunner`
/// before posting `WireMessage::RunFast` so the result demux can
/// route updates to the matching `RunHandle.progress_rx`.
pub fn register_run_sender(
    run_id: lunco_experiments::ExperimentId,
    tx: crossbeam_channel::Sender<lunco_experiments::RunUpdate>,
) {
    if let Ok(mut map) = run_senders().lock() {
        map.insert(run_id, tx);
    }
}

fn forward_run_update(run_id: lunco_experiments::ExperimentId, update: lunco_experiments::RunUpdate) {
    let tx = match run_senders().lock().ok().and_then(|m| m.get(&run_id).cloned()) {
        Some(tx) => tx,
        None => {
            bevy::log::warn!("[worker_transport] RunUpdate for unknown run_id");
            return;
        }
    };
    let terminal = matches!(
        update,
        lunco_experiments::RunUpdate::Completed(_)
            | lunco_experiments::RunUpdate::Failed { .. }
            | lunco_experiments::RunUpdate::Cancelled
    );
    let _ = tx.send(update);
    if terminal {
        if let Ok(mut map) = run_senders().lock() {
            map.remove(&run_id);
        }
        // Free the worker that ran this Fast Run so the next queued run can
        // be dispatched to it. (For the fall-back-to-0 case `running[idx]`
        // holds a different run, so only clear it when it matches.)
        if let Ok(mut p) = pool().lock() {
            if let Some(idx) = p.run_to_worker.remove(&run_id) {
                if p.running.get(idx).copied().flatten() == Some(run_id) {
                    p.running[idx] = None;
                }
            }
        }
    }
}

fn ensure_parse_done_channel() -> &'static crossbeam_channel::Sender<ParseDoneEnvelope> {
    PARSE_DONE_TX.get_or_init(|| {
        let (tx, rx) = crossbeam_channel::unbounded();
        let _ = PARSE_DONE_RX.set(rx);
        tx
    })
}

/// Drain a single completed parse result, if any. Bevy system on the
/// main thread polls this each tick; returns `None` when the queue
/// is empty.
pub fn try_recv_parse_done() -> Option<ParseDoneEnvelope> {
    let _ = ensure_parse_done_channel();
    PARSE_DONE_RX.get()?.try_recv().ok()
}

/// Stash the result-side sender so the JS `onmessage` callback can push
/// decoded results into the same crossbeam channel that
/// `worker::handle_modelica_responses` drains. Called by the
/// `ModelicaPlugin` setup; idempotent (later calls are silently dropped).
pub fn register_result_sender(tx_res: Sender<ModelicaResult>) -> bool {
    RESULT_TX.set(tx_res).is_ok()
}

/// Stash the command-side sender so the dev-test JS bridge can post
/// commands directly without going through the UI. Same handle as
/// `ModelicaChannels.tx`. Idempotent.
pub fn register_command_sender(
    tx_cmd: crossbeam_channel::Sender<ModelicaCommand>,
) -> bool {
    COMMAND_TX.set(tx_cmd).is_ok()
}

/// `true` once a JS Worker has been attached via [`install_worker`]. The
/// inline worker checks this and bails out so the two paths can't race
/// for the same `rx_cmd` queue.
pub fn is_worker_active() -> bool {
    pool().lock().map(|p| !p.workers.is_empty()).unwrap_or(false)
}

/// Wire up the JS Worker to the Rust result channel.
///
/// `worker_url` is the absolute or origin-relative URL to the worker JS
/// shim (typically `./worker/lunica_worker.js`, generated by `wasm-bindgen
/// --target web`). The shim is started as `type=module` so it can `import`
/// the worker wasm and run `wasm_bindgen(start)`.
///
/// Call exactly once on startup, after `register_result_sender` (which
/// `ModelicaPlugin::build` does for you), and before any commands fire.
///
/// Creates a pool of workers sized from the persisted
/// `experiments.max_parallel` setting (auto = 1 on wasm), clamped to
/// [`MAX_WORKERS`]. All workers share one set of result channels — each
/// worker's `onmessage` routes through the same global handlers, demuxing
/// Fast Runs by `run_id`. Tolerant of partial failure: as long as worker 0
/// starts it returns `Ok`; a later worker failing just shrinks the pool.
pub fn install_worker(worker_url: &str) -> Result<(), JsValue> {
    let want = lunco_settings::load_section_from_disk::<
        crate::experiments_runner::ExperimentSettings,
    >()
    .resolved_max_parallel()
    .clamp(1, MAX_WORKERS);

    let mut workers: Vec<WorkerHandle> = Vec::with_capacity(want);
    let mut first_err: Option<JsValue> = None;
    for i in 0..want {
        match make_worker(i, worker_url) {
            Ok(worker) => workers.push(WorkerHandle(worker)),
            // Worker 0 failing is fatal (caller falls back to the inline
            // path); a later one failing just caps the pool smaller.
            Err(e) if i == 0 => return Err(e),
            Err(e) => {
                first_err = Some(e);
                break;
            }
        }
    }

    let n = workers.len();
    {
        let mut p = pool().lock().unwrap();
        if !p.workers.is_empty() {
            // Already installed — keep the existing pool (idempotent).
            return Ok(());
        }
        p.running = vec![None; n];
        p.workers = workers;
    }
    // Retain the script URL so a crashed worker can be respawned in place
    // (the callbacks are `.forget()`-leaked inside `make_worker`, so there's
    // no Rust-side closure storage to keep alive).
    let _ = WORKER_URL.set(worker_url.to_string());

    if let Some(e) = first_err {
        bevy::log::warn!(
            "[worker_transport] requested {want} workers but only {n} started: {e:?}"
        );
    }
    bevy::log::info!(
        "[worker_transport] worker pool installed: {n} worker(s) at {worker_url}"
    );
    Ok(())
}

/// Construct one `Worker`, wired with both an `onmessage` (results) and an
/// `onerror` (crash) handler. The message handler routes every message kind
/// through the process-global handlers, so all pooled workers are
/// interchangeable; Fast Run updates demux by `run_id`. The error handler
/// (`idx`-tagged) turns a worker crash (wasm `unreachable`/OOM, panic) into a
/// graceful run failure plus an in-place respawn — see [`handle_worker_error`].
///
/// Both closures are `.forget()`-leaked into the JS runtime rather than
/// stored Rust-side. That's deliberate: the error handler can fire and
/// respawn *this same* worker, and dropping a closure while it is executing
/// is undefined behaviour. Leaking makes the callbacks permanent; respawns
/// are crash-only and rare, so the few-KB-per-respawn leak is negligible.
fn make_worker(idx: usize, worker_url: &str) -> Result<Worker, JsValue> {
    let mut opts = web_sys::WorkerOptions::new();
    opts.set_type(web_sys::WorkerType::Module);
    let worker = Worker::new_with_options(worker_url, &opts)?;

    let onmessage = Closure::wrap(Box::new(move |event: MessageEvent| {
        let data = event.data();
        let bytes: Vec<u8> = match Uint8Array::new(&data).to_vec() {
            v if !v.is_empty() => v,
            _ => return,
        };
        match bincode::deserialize::<WireResult>(&bytes) {
            Ok(WireResult::Result(result)) => {
                if let Some(tx) = RESULT_TX.get() {
                    let _ = tx.send(result);
                }
            }
            Ok(WireResult::ParseDocumentDone { doc_id, gen, ast, errors }) => {
                let tx = ensure_parse_done_channel();
                let _ = tx.send(ParseDoneEnvelope { doc_id, gen, ast, errors });
            }
            Ok(WireResult::RunUpdate { run_id, update }) => {
                forward_run_update(run_id, update);
            }
            Ok(WireResult::Log(line)) => {
                // Surface worker-side diagnostics in the main page's
                // Console panel — the worker has its own console context
                // that page-level DevTools can't see.
                bevy::log::info!("[worker] {line}");
            }
            Ok(WireResult::RecycleRequest { mem_mb }) => {
                // The worker's grow-only wasm memory has climbed past the
                // watermark. It's idle now (this arrives after the run's
                // terminal update), so retire + respawn it to reset its linear
                // memory — the only way to reclaim grow-only wasm memory and
                // the fix for cross-run accumulation that otherwise OOMs after
                // a few heavy runs. `respawn_worker` defers the MSL re-seed.
                bevy::log::info!(
                    "[worker_transport] worker {idx} requested recycle at {mem_mb} MB — respawning"
                );
                respawn_worker(idx);
            }
            Err(e) => {
                bevy::log::error!("[worker_transport] failed to decode result: {e}");
            }
        }
    }) as Box<dyn FnMut(MessageEvent)>);
    worker.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
    onmessage.forget();

    let onerror = Closure::wrap(Box::new(move |e: ErrorEvent| {
        bevy::log::error!(
            "[worker_transport] worker {idx} crashed: {} ({}:{})",
            e.message(),
            e.filename(),
            e.lineno()
        );
        handle_worker_error(idx);
    }) as Box<dyn FnMut(ErrorEvent)>);
    worker.set_onerror(Some(onerror.as_ref().unchecked_ref()));
    onerror.forget();

    Ok(worker)
}

/// Recover from a worker crash without ever wedging the main thread. Called
/// from the worker's `onerror` handler (so the calculation never affects main
/// code beyond this controlled recovery):
///   1. Fail the run that worker was executing — synthesize a terminal
///      `RunUpdate::Failed`, which frees the run sender and the pool slot via
///      the normal terminal path. Without this the run would hang "running"
///      forever, since a dead worker never posts its own terminal update.
///   2. Respawn a fresh worker in that slot and re-seed it with MSL, so pool
///      capacity self-heals (critical for the wasm default single-worker pool).
fn handle_worker_error(idx: usize) {
    let crashed_run = {
        let p = pool().lock().unwrap();
        p.running.get(idx).copied().flatten()
    };
    if let Some(run_id) = crashed_run {
        bevy::log::warn!(
            "[worker_transport] failing run {run_id:?} after worker {idx} crash"
        );
        forward_run_update(
            run_id,
            lunco_experiments::RunUpdate::Failed {
                error: "simulation worker crashed — likely out of memory or a solver \
                        abort. The model is too heavy for the browser; try a shorter \
                        StopTime or a looser Tolerance."
                    .to_string(),
                partial: None,
            },
        );
    }
    // `forward_run_update` already cleared `running[idx]`/`run_to_worker` for
    // the failed run; the respawn below resets the slot regardless.
    respawn_worker(idx);
}

/// Replace the (dead) worker at `idx` with a fresh one and re-install MSL into
/// it. Best-effort: logs and leaves the slot empty if the URL/MSL aren't
/// cached yet (can only happen before first MSL install, when no run exists).
fn respawn_worker(idx: usize) {
    let Some(url) = WORKER_URL.get() else {
        bevy::log::error!("[worker_transport] cannot respawn worker {idx}: no URL cached");
        return;
    };
    let worker = match make_worker(idx, url) {
        Ok(w) => w,
        Err(e) => {
            bevy::log::error!("[worker_transport] respawn of worker {idx} failed: {e:?}");
            return;
        }
    };
    {
        let mut p = pool().lock().unwrap();
        if let Some(slot) = p.workers.get_mut(idx) {
            *slot = WorkerHandle(worker);
        }
        if let Some(r) = p.running.get_mut(idx) {
            *r = None;
        }
    }
    // Defer the MSL re-seed. Re-allocating the ~165 MB bundle right now —
    // on the crash stack, microseconds after a worker exhausted ~4 GB —
    // throws `RangeError: Array buffer allocation failed` because the dead
    // worker's linear memory hasn't been reclaimed yet. `pump_worker_respawns`
    // posts it on a later frame, after a settle delay.
    if let Ok(mut q) = pending_reseed().lock() {
        q.push((idx, web_time::Instant::now()));
    }
    bevy::log::info!("[worker_transport] respawned worker {idx}; MSL re-seed deferred");
}

/// Re-seed MSL into respawned workers, deferred off the crash stack. Posts at
/// most one worker's MSL per call, and only after a short settle delay so the
/// crashed worker's ~4 GB linear memory has been reclaimed first — allocating
/// the ~165 MB bundle too soon throws `RangeError: Array buffer allocation
/// failed`. Bevy `Update` system (wasm only); a cheap no-op when nothing is
/// pending (the overwhelmingly common case).
pub fn pump_worker_respawns() {
    let Some(bytes) = MSL_WIRE.get() else {
        return;
    };
    const SETTLE: core::time::Duration = core::time::Duration::from_millis(1500);
    let ready = {
        let mut q = match pending_reseed().lock() {
            Ok(q) => q,
            Err(_) => return,
        };
        match q.iter().position(|(_, t)| t.elapsed() >= SETTLE) {
            Some(pos) => Some(q.remove(pos).0),
            None => None,
        }
    };
    if let Some(idx) = ready {
        post_bytes_to(idx, bytes, "respawn MSL reinstall (deferred)");
        bevy::log::info!("[worker_transport] re-seeded MSL into respawned worker {idx}");
    }
}

/// Drain `ModelicaChannels.rx_cmd` and ship each command to the JS worker.
///
/// Bevy system. Runs every `Update`. Cheap when the queue is empty; when it
/// isn't, each command is bincode-encoded and posted as a `Uint8Array`. The
/// worker's `process_inline_command` runs in its own thread and posts results
/// back asynchronously via `onmessage` (see [`install_worker`]).
pub fn pump_commands_to_worker(channels: Res<ModelicaChannels>) {
    if !is_worker_active() {
        // install_worker hasn't run yet — main app is mid-bootstrap.
        // Commands stay in the channel; we'll catch them next tick.
        return;
    }

    while let Ok(cmd) = channels.rx_cmd.try_recv() {
        // Boot-race gate: Compile / UpdateParameters need the worker's
        // MSL index to be populated, otherwise rumoca emits silent
        // "unresolved reference Modelica.*" failures. Queue them until
        // install_msl_in_worker drains the queue. Other commands
        // (Step/Reset/Despawn) pass through unchanged.
        if command_needs_msl(&cmd) && !msl_installed() {
            PENDING_COMMANDS.with(|q| q.borrow_mut().push(cmd));
            continue;
        }
        // Compile / parse / step traffic always goes to the primary worker
        // (0) so a Fast Run fanned out to other workers can't reorder it.
        post_msg_to(0, &WireMessage::Command(cmd), "command");
    }
}

/// Post a Fast Run request to the pool. Gated behind MSL install just like
/// compiles — without MSL the worker's compile would emit silent
/// "unresolved Modelica.*" failures. Once MSL is up, the run is assigned to
/// a free worker (see [`assign_and_post_run_fast`]).
pub fn dispatch_run_fast(
    run_id: lunco_experiments::ExperimentId,
    model_name: String,
    source: String,
    filename: String,
    extras: Vec<(String, String)>,
    overrides: std::collections::BTreeMap<
        lunco_experiments::ParamPath,
        lunco_experiments::ParamValue,
    >,
    inputs: std::collections::BTreeMap<
        lunco_experiments::ParamPath,
        lunco_experiments::ParamValue,
    >,
    bounds: lunco_experiments::RunBounds,
) -> bool {
    if !is_worker_active() {
        return false;
    }
    let msg = WireMessage::RunFast {
        run_id,
        model_name,
        source,
        filename,
        extras,
        overrides,
        inputs,
        bounds,
    };
    if !msl_installed() {
        // Queue whole-message; worker assignment happens at flush time.
        PENDING_RUN_FAST.with(|q| q.borrow_mut().push(msg));
        return true;
    }
    assign_and_post_run_fast(run_id, &msg);
    true
}

/// Pick a worker for `run_id` and post the (already-built) RunFast message
/// to it. Prefers a free non-primary worker so worker 0 stays available for
/// compiles; falls back to worker 0 (serializing behind its current work)
/// when every worker is busy. Records the `run_id → worker` mapping for
/// cancel routing; the slot is freed in [`forward_run_update`] on terminal.
fn assign_and_post_run_fast(run_id: lunco_experiments::ExperimentId, msg: &WireMessage) {
    let bytes = match bincode::serialize(msg) {
        Ok(b) => b,
        Err(e) => {
            bevy::log::error!("[worker_transport] run_fast: serialize failed: {e}");
            return;
        }
    };
    let mut p = pool().lock().unwrap();
    let n = p.workers.len();
    if n == 0 {
        bevy::log::warn!("[worker_transport] run_fast: no workers installed");
        return;
    }
    // Prefer a free worker that isn't the primary (1..n), else the primary.
    // Skip workers awaiting an MSL re-seed after a recycle (their MSL index is
    // empty so they'd fail the compile); fall back to any free worker, then to
    // the primary, so a single-worker pool still makes progress.
    let free_ready = |p: &WorkerPool| {
        (1..n)
            .chain(std::iter::once(0))
            .find(|&i| p.running[i].is_none() && !is_reseed_pending(i))
    };
    let idx = free_ready(&p)
        .or_else(|| (1..n).chain(std::iter::once(0)).find(|&i| p.running[i].is_none()))
        .unwrap_or(0);
    if p.running[idx].is_none() {
        p.running[idx] = Some(run_id);
    }
    p.run_to_worker.insert(run_id, idx);
    // Post inside the lock — wasm is single-threaded, so there's no
    // re-entrancy and no other code can observe a half-updated pool.
    let WorkerHandle(worker) = &p.workers[idx];
    let array = Uint8Array::new_with_length(bytes.len() as u32);
    array.copy_from(&bytes);
    if let Err(e) = worker.post_message(&array) {
        bevy::log::error!("[worker_transport] run_fast: post to worker {idx} failed: {e:?}");
    }
}

/// Cancel an in-flight Fast Run. Best-effort; latency depends on the
/// worker's poll cadence. Routed to the worker that owns the run; if the
/// mapping is unknown (e.g. still queued behind MSL install) it broadcasts
/// to every worker (a no-op in the ones not running it).
pub fn dispatch_cancel_run(run_id: lunco_experiments::ExperimentId) {
    let (target, n) = {
        let p = pool().lock().unwrap();
        (p.run_to_worker.get(&run_id).copied(), p.workers.len())
    };
    if n == 0 {
        return;
    }
    match target {
        Some(idx) => post_msg_to(idx, &WireMessage::CancelRun { run_id }, "cancel_run"),
        None => {
            for i in 0..n {
                post_msg_to(i, &WireMessage::CancelRun { run_id }, "cancel_run(bcast)");
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
thread_local! {
    static PENDING_RUN_FAST: std::cell::RefCell<Vec<WireMessage>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(target_arch = "wasm32")]
fn flush_pending_run_fast() {
    let drained: Vec<WireMessage> = PENDING_RUN_FAST.with(|q| q.borrow_mut().drain(..).collect());
    if drained.is_empty() {
        return;
    }
    bevy::log::info!(
        "[worker_transport] flushing {} RunFast request(s) queued during MSL install",
        drained.len()
    );
    for msg in &drained {
        if let WireMessage::RunFast { run_id, .. } = msg {
            assign_and_post_run_fast(*run_id, msg);
        }
    }
}

/// Drain any compile-path commands queued by `pump_commands_to_worker`
/// while the MSL install was still pending. Called from
/// `install_msl_in_worker` after MSL is shipped to the worker.
#[cfg(target_arch = "wasm32")]
fn flush_pending_commands() {
    let drained: Vec<ModelicaCommand> =
        PENDING_COMMANDS.with(|q| q.borrow_mut().drain(..).collect());
    if drained.is_empty() {
        return;
    }
    bevy::log::info!(
        "[worker_transport] flushing {} compile-path command(s) queued during MSL install",
        drained.len()
    );
    for cmd in drained {
        let envelope = WireMessage::Command(cmd);
        let bytes = match bincode::serialize(&envelope) {
            Ok(b) => b,
            Err(e) => {
                bevy::log::error!("[worker_transport] flushed encode failed: {e}");
                continue;
            }
        };
        post_to_worker_bytes(&bytes, "command(flushed)");
    }
}

/// Post raw bytes to the primary worker (0). Compile/parse/ping traffic.
fn post_to_worker_bytes(bytes: &[u8], label: &str) {
    post_bytes_to(0, bytes, label);
}

/// Serialize and post a `WireMessage` to the primary worker (0).
fn post_to_worker(msg: &WireMessage, label: &str) {
    post_msg_to(0, msg, label);
}

/// JS-callable bridge for the dev test loop. Sends a `WireMessage::Ping`
/// to the worker and expects a `[worker] pong` line on the main page
/// console. Use from DevTools: `await window.__lc_test_worker_ping('hi')`.
#[wasm_bindgen]
pub fn __lc_test_worker_ping(tag: &str) {
    bevy::log::info!("[worker_transport] sending ping: {tag}");
    post_to_worker(&WireMessage::Ping(tag.to_string()), "ping");
}

// ── Boot-race gate for parse requests ──
//
// Empirically the worker can be bootstrapped (the JS shim has loaded
// and we can `postMessage` to it) seconds before its WASM module is
// initialised AND seconds before the MSL bundle has landed. A parse
// request that arrives during that window is delivered to the worker
// in *some* order — sometimes ahead of the MSL install, sometimes
// after — and either way we've seen the request silently dropped /
// produce no `ParseDocumentDone` reply. The user-visible symptom is
// "Loading resource…" forever for whichever doc was unlucky enough
// to fire on the boot frame (most often the first restored autosave
// doc).
//
// Fix: queue parses on the host side until `install_msl_in_worker`
// has run. After that point the worker has its ready ack out *and*
// has the MSL index, so parses can resolve imports against it. Drain
// the queue right after we ship MSL so the gap is invisible to
// callers.
//
// `wasm32-unknown-unknown` is single-threaded so a `RefCell` in a
// `thread_local!` is enough — no `Mutex` needed.
struct PendingParse {
    doc_id: lunco_doc::DocumentId,
    gen: u64,
    uri: String,
    source: String,
}

#[cfg(target_arch = "wasm32")]
thread_local! {
    static MSL_INSTALLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static PENDING_PARSES: std::cell::RefCell<Vec<PendingParse>> =
        const { std::cell::RefCell::new(Vec::new()) };
    // Commands that need MSL resolved (Compile, UpdateParameters, future RunFast)
    // queued until install_msl_in_worker drains them. Step/Reset/Despawn pass
    // through unconditionally — they don't recompile.
    static PENDING_COMMANDS: std::cell::RefCell<Vec<ModelicaCommand>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Commands that depend on a populated MSL index in the worker (compile-path
/// commands). Sent before MSL install lands → silent "unresolved Modelica.*"
/// failures. Gate them; drain on `install_msl_in_worker`.
fn command_needs_msl(cmd: &ModelicaCommand) -> bool {
    matches!(
        cmd,
        ModelicaCommand::Compile { .. } | ModelicaCommand::UpdateParameters { .. }
    )
}

#[cfg(not(target_arch = "wasm32"))]
fn msl_installed() -> bool { true }
#[cfg(target_arch = "wasm32")]
fn msl_installed() -> bool {
    MSL_INSTALLED.with(|c| c.get())
}

/// Send a doc to the worker for off-thread parsing. Used by
/// `engine_resource::drive_engine_sync` on wasm in place of the
/// main-thread parse spawn. The result lands via the parse-done
/// channel ([`try_recv_parse_done`]).
///
/// Returns `false` when the worker isn't installed (very early boot
/// or worker init failed); callers fall back to local parsing.
/// Returns `true` when the request has been posted *or queued*
/// behind the MSL-install gate (see the boot-race note above) — in
/// both cases the host should consider it accepted.
pub fn dispatch_parse_to_worker(
    doc_id: lunco_doc::DocumentId,
    gen: u64,
    uri: String,
    source: String,
) -> bool {
    if !is_worker_active() {
        return false;
    }
    if !msl_installed() {
        #[cfg(target_arch = "wasm32")]
        PENDING_PARSES.with(|q| {
            q.borrow_mut().push(PendingParse { doc_id, gen, uri, source });
        });
        #[cfg(not(target_arch = "wasm32"))]
        let _ = (doc_id, gen, uri, source);
        return true;
    }
    post_to_worker(
        &WireMessage::ParseDocument { doc_id, gen, uri, source },
        "parse",
    );
    true
}

/// Drain any parse requests queued by `dispatch_parse_to_worker`
/// while the MSL install was still pending. Called from
/// `install_msl_in_worker` after MSL is shipped to the worker.
#[cfg(target_arch = "wasm32")]
fn flush_pending_parses() {
    let drained: Vec<PendingParse> = PENDING_PARSES.with(|q| q.borrow_mut().drain(..).collect());
    if drained.is_empty() {
        return;
    }
    bevy::log::info!(
        "[worker_transport] flushing {} parse request(s) queued during MSL install",
        drained.len()
    );
    for p in drained {
        post_to_worker(
            &WireMessage::ParseDocument {
                doc_id: p.doc_id,
                gen: p.gen,
                uri: p.uri,
                source: p.source,
            },
            "parse(flushed)",
        );
    }
}

/// JS-callable bridge that synthesizes a `ModelicaCommand::Compile` and
/// pushes it through the same channel the UI uses. Bypasses the canvas
/// click pathway — synthetic mouse events don't reach egui reliably from
/// the page, so this is the autonomous test path.
///
/// Uses `Entity::PLACEHOLDER` so the result stream lands on no model entity
/// — the result still surfaces in console via `[worker] done:` so we know
/// compile finished + how long it took.
#[wasm_bindgen]
pub fn __lc_test_dispatch_compile(model_name: &str, source: &str) {
    let Some(tx) = COMMAND_TX.get() else {
        bevy::log::error!("[worker_transport] dispatch_compile: command sender not registered");
        return;
    };
    bevy::log::info!(
        "[worker_transport] dispatching test Compile: model={model_name} src={}B",
        source.len()
    );
    let cmd = ModelicaCommand::Compile {
        entity: bevy::prelude::Entity::PLACEHOLDER,
        session_id: 1,
        model_name: model_name.to_string(),
        source: source.to_string(),
        extra_sources: Vec::new(),
        stream: None,
    };
    if let Err(e) = tx.send(cmd) {
        bevy::log::error!("[worker_transport] dispatch_compile: send failed: {e}");
    }
}

/// Ship the pre-parsed MSL bundle to the off-thread worker so its own
/// `GLOBAL_PARSED_MSL` slot is populated before any Compile arrives.
///
/// Called from `msl_remote::drain_msl_load_slot` after the main app's
/// install lands. No-op if the worker isn't installed (we'd be the only
/// side that needed MSL anyway).
///
/// Every pooled worker compiles, so MSL is installed into ALL of them.
/// The single-worker case keeps the zero-copy `postMessage(_, [transfer])`
/// fast path (the `ArrayBuffer` is *moved* into the worker, avoiding a
/// ~1–2 s memcpy of the 165 MB bundle on first load). With a pool we must
/// hand the bytes to each worker, so a fresh structured-clone copy is sent
/// per worker — the cost the extra workers pay for parallelism.
pub fn install_msl_in_worker(
    parsed: &[(String, rumoca_compile::parsing::StoredDefinition)],
) {
    let envelope = WireMessage::InstallParsedMsl(parsed.to_vec());
    let bytes = match bincode::serialize(&envelope) {
        Ok(b) => b,
        Err(e) => {
            bevy::log::error!("[worker_transport] encode MSL install failed: {e}");
            return;
        }
    };
    let len = bytes.len();
    // Retain a copy so a respawned worker can be re-seeded with MSL without
    // re-fetching/parsing the bundle. Set-once; the bundle is identical for
    // every worker. (Cloned before the single-worker transfer path below
    // detaches its `ArrayBuffer`.)
    if MSL_WIRE.get().is_none() {
        let _ = MSL_WIRE.set(bytes.clone());
    }

    let n = {
        let p = pool().lock().unwrap();
        let n = p.workers.len();
        if n == 0 {
            return;
        }
        let single = n == 1;
        for (i, WorkerHandle(worker)) in p.workers.iter().enumerate() {
            // Fresh array per worker — the transfer path detaches the
            // buffer, so it's only valid when there's exactly one worker.
            let array = Uint8Array::new_with_length(len as u32);
            array.copy_from(&bytes);
            let res = if single {
                let transfer = js_sys::Array::new();
                transfer.push(&array.buffer());
                worker.post_message_with_transfer(&array, &transfer)
            } else {
                worker.post_message(&array)
            };
            if let Err(e) = res {
                bevy::log::error!(
                    "[worker_transport] MSL install to worker {i} failed: {e:?}"
                );
            }
        }
        n
    };

    bevy::log::info!(
        "[worker_transport] installed MSL into {n} worker(s): {} docs ({} bytes wire each)",
        parsed.len(),
        len
    );
    // Open the gate now that every worker has its index; drain anything
    // that queued behind it (parses, compile-path commands, Fast Runs).
    #[cfg(target_arch = "wasm32")]
    {
        MSL_INSTALLED.with(|c| c.set(true));
        flush_pending_parses();
        flush_pending_commands();
        flush_pending_run_fast();
    }
}
