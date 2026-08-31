//! Cross-thread Modelica worker transport for wasm32-unknown-unknown.
//!
//! Why this module exists
//! ----------------------
//! On native, `worker::modelica_worker` runs on its own OS thread and exchanges
//! `ModelicaCommand` / `ModelicaResult` over crossbeam channels with the Bevy
//! main loop. The blocking compile / step work never blocks the UI.
//!
//! On wasm32-unknown-unknown there are no OS threads. Modelica work therefore
//! runs in a Web Worker carrying a second wasm instance
//! (`bin/lunica_worker.rs`). Bevy systems still see the same crossbeam
//! channels — only the bridge between them and the worker changes.
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
//!    `worker::process_worker_command` against its local `ModelicaWorkerState`,
//!    and posts each `ModelicaResult` back the same way.
//!
//! All wasm-only — `cfg(target_arch = "wasm32")` at the module level.

#![cfg(target_arch = "wasm32")]

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Mutex, OnceLock};

use bevy::prelude::*;
use crossbeam_channel::Sender;
use js_sys::Uint8Array;
use lunco_worker_transport::{Callbacks, WorkerPool as WorkerTransport};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use crate::lock_ext::LockExt;
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
    /// Install the MSL bundle as the raw **compressed** `parsed-*.bin.zst`
    /// bytes (zstd-wrapped bincode of `Vec<(uri, StoredDefinition)>`). The
    /// worker decompresses + bincode-decodes off the main thread, then signals
    /// readiness with [`WireResult::MslReady`].
    ///
    /// This is the boot path: shipping the ~19 MB compressed blob (vs the
    /// ~173 MB decoded representation) avoids decompression on the main thread.
    /// The worker decompresses once, then — *if* `provide_to_main` — ships the
    /// decoded bincode bytes back to the main thread as a transferred
    /// `ArrayBuffer`, so the main thread's resolution/autocomplete heap is filled
    /// by *deserialize only* (see `msl_remote::ingest_worker_decoded_msl`).
    ///
    /// With a worker pool, only the **primary** (worker 0) gets
    /// `provide_to_main = true` — the main thread needs exactly one decoded copy,
    /// so the other workers decode for their own compiles but skip the ~173 MB
    /// transfer the main thread would just dedupe away.
    InstallParsedMslCompressed {
        bytes: Vec<u8>,
        provide_to_main: bool,
    },
    /// Decode the generated editor index from the retained source bundle.
    /// The worker sends the result back in bounded chunks so the browser UI
    /// never parses the 20 MB JSON artifact in one main-thread turn.
    InstallMslIndexFromSource { bytes: Vec<u8> },
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
        overrides:
            std::collections::BTreeMap<lunco_experiments::ParamPath, lunco_experiments::ParamValue>,
        #[serde(default)]
        inputs:
            std::collections::BTreeMap<lunco_experiments::ParamPath, lunco_experiments::ParamValue>,
        bounds: lunco_experiments::RunBounds,
    },
    /// Best-effort cancel of an in-flight Fast Run. Worker observes
    /// the flag between solver steps. v1: cancel granularity is
    /// "between steps in the worker's run loop".
    CancelRun {
        run_id: lunco_experiments::ExperimentId,
    },
}

/// Wire-format envelope from worker → main. Same multiplexing principle as
/// `WireMessage`: lets the worker emit out-of-band log lines that surface
/// in the main page's console (Web Workers have a separate console context
/// that's invisible to the page DevTools, so without this any worker
/// panic/error is silent).
#[derive(serde::Serialize, serde::Deserialize)]
pub enum WireResult {
    /// A normal `ModelicaResult` produced by `process_worker_command`.
    Result(ModelicaResult),
    /// The worker finished decoding the compressed MSL bundle (from
    /// [`WireMessage::InstallParsedMslCompressed`]) into its own
    /// `GLOBAL_PARSED_MSL`, and is ready to resolve `Modelica.*` references.
    /// The main thread opens the compile gate (drains queued compiles/parses/
    /// Fast Runs) on receipt. `docs` is the decoded class count, for logging.
    MslReady { docs: usize },
    /// A bounded part of the generated editor index decoded by the worker.
    /// `done` closes the current assembly on the main side.
    MslIndexChunk {
        components: Vec<crate::index::ClassEntry>,
        bundled: Vec<crate::package_tree::types::PackageNode>,
        done: bool,
    },
    /// The required MSL runtime artifact could not be decoded. This is a
    /// terminal packaging/runtime error; no source reparse path exists.
    MslFailed { error: String },
    /// The generated editor index could not be decoded. This is a terminal
    /// metadata error; source readiness is reported independently.
    MslIndexFailed { error: String },
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
        errors: Vec<lunco_doc::Diagnostic>,
    },
    /// Terminal failure for a document parse. The main thread records the
    /// diagnostic and clears the document's in-flight slot; it does not invent
    /// an AST or retry a failed worker dispatch indefinitely.
    ParseDocumentFailed {
        doc_id: lunco_doc::DocumentId,
        gen: u64,
        error: String,
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

// The pooled `Worker`s live in a `lunco_worker_transport::WorkerPool` (composed
// into `WorkerPool::inner` below); that generic crate owns the spawn /
// boot-handshake / byte + Transferable post / crash-respawn plumbing and the
// `unsafe impl Send + Sync` that lets the pool sit in a static `Mutex` on
// single-threaded wasm. This file keeps only the Modelica-specific state
// (per-worker Fast-Run occupancy, run→worker routing, MSL readiness) on top.

/// Process-wide pool of Modelica workers (step 3 of the parallel-experiments
/// plan, `docs/architecture/25-experiments.md`).
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
    /// The generic Web Worker pool (spawn / handshake / post / respawn). `None`
    /// until [`install_worker`] builds it; `Some` once the pool is up.
    inner: Option<WorkerTransport>,
    /// Per-worker Fast Run occupant (`None` = free for a Fast Run). The
    /// compile/parse traffic on worker 0 does NOT mark it occupied here.
    running: Vec<Option<lunco_experiments::ExperimentId>>,
    /// `run_id → worker index`, for cancel routing. Set on every dispatch
    /// (including the fall-back-to-0 case, where `running` isn't reassigned),
    /// cleared on the run's terminal update.
    run_to_worker: HashMap<lunco_experiments::ExperimentId, usize>,
    /// Per-worker MSL-bundle state. The boot MSL decode is shipped to worker 0
    /// *only* (so the rest stay free to parse — broadcasting it saturates the
    /// whole pool and starves the parse, the reported boot bug); the
    /// secondaries are seeded once worker 0 is ready. A worker can only run a
    /// compile/Fast Run once its state is `Ready`; parsing needs no MSL and
    /// runs on any non-`Decoding` worker.
    msl: Vec<MslState>,
}

/// Per-worker MSL-bundle readiness. `Absent` → never seeded; `Decoding` → the
/// compressed bundle was posted and the worker is decompressing+deserializing
/// it off-thread (a multi-second job — keep parses off it); `Ready` → the
/// worker has MSL in its session and can compile / run.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MslState {
    Absent,
    Decoding,
    Ready,
}

/// Hard ceiling on pool size. Each worker is a full wasm instance + ~MSL
/// copy, so a runaway `max_parallel` setting can't exhaust browser memory.
const MAX_WORKERS: usize = 8;

static POOL: OnceLock<Mutex<WorkerPool>> = OnceLock::new();

/// The worker script URL, retained so a crashed worker can be respawned
/// in place (see [`respawn_worker`]). Set once at [`install_worker`].
static WORKER_URL: OnceLock<String> = OnceLock::new();

// The serialized MSL-install wire bytes, retained so a worker can be (re)seeded
// without re-fetching/parsing the bundle. Reused for crash-respawn re-seed
// (`pump_worker_respawns`) and secondary-worker boot seeding
// (`seed_secondary_workers`). It contains an
// `InstallParsedMslCompressed { provide_to_main: false }` envelope.
thread_local! {
    /// Current compressed MSL install envelope. It is replaced by an explicit
    /// reinstall so a worker respawn can never receive bytes from an older
    /// artifact generation.
    static MSL_WIRE: std::cell::RefCell<Option<Vec<u8>>> =
        const { std::cell::RefCell::new(None) };
}

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
            inner: None,
            running: Vec::new(),
            run_to_worker: HashMap::new(),
            msl: Vec::new(),
        })
    })
}

impl WorkerPool {
    /// Number of spawned workers (0 before `install_worker`).
    fn worker_count(&self) -> usize {
        self.inner.as_ref().map(|p| p.len()).unwrap_or(0)
    }

    /// Whether the pool is up (≥1 worker spawned).
    fn is_active(&self) -> bool {
        self.inner.as_ref().is_some_and(|p| !p.is_empty())
    }
}

/// Post raw `bytes` (a fresh `Uint8Array` copy) to worker `idx`. No-op (with a
/// warning) if the pool isn't up or the index is out of range. Caller must NOT
/// hold the pool lock (this takes it).
fn post_bytes_to(idx: usize, bytes: &[u8], label: &str) -> Result<(), String> {
    let p = pool().lock_or_recover();
    let Some(inner) = p.inner.as_ref() else {
        let error = format!("{label}: worker pool not installed");
        bevy::log::warn!("[worker_transport] {error}");
        return Err(error);
    };
    inner.post(idx, bytes).map_err(|e| {
        let error = format!("{label}: post to worker {idx} failed: {e:?}");
        bevy::log::error!("[worker_transport] {error}");
        error
    })
}

/// Wire-protocol fingerprint baked in by `build.rs` (hash of the workspace
/// `Cargo.lock` + this crate's `src/`). The `lunica_worker` binary links this
/// same crate, so a worker built from identical source carries an identical id.
/// A stale worker carries a different one — caught by the boot handshake below.
pub const WIRE_BUILD_ID: &str = env!("LUNCO_WIRE_BUILD_ID");

/// Prefix of the plain-string message the worker posts once on boot
/// (`"LUNCO_WIRE:<id>"`). Deliberately NOT bincode: its framing must survive
/// the exact `WireMessage` layout drift it exists to detect.
pub const WIRE_HANDSHAKE_PREFIX: &str = "LUNCO_WIRE:";

thread_local! {
    /// Set true if any worker announced a `WIRE_BUILD_ID` that disagrees with
    /// ours — i.e. the shipped worker wasm is stale. Surfaced loudly; the
    /// worker pool is useless in this state (every bincode message mis-decodes).
    static WIRE_MISMATCH: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// True once a stale-worker wire mismatch has been detected this session.
pub fn wire_protocol_mismatch() -> bool {
    WIRE_MISMATCH.with(|c| c.get())
}

/// Serialize and post a `WireMessage` to worker `idx`.
fn post_msg_to(idx: usize, msg: &WireMessage, label: &str) -> Result<(), String> {
    let bytes = bincode::serde::encode_to_vec(msg, bincode::config::standard()).map_err(|e| {
        let error = format!("{label}: serialize failed: {e}");
        bevy::log::error!("[worker_transport] {error}");
        error
    })?;
    post_bytes_to(idx, &bytes, label)
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
    pub errors: Vec<lunco_doc::Diagnostic>,
}
pub struct ParseFailedEnvelope {
    pub doc_id: lunco_doc::DocumentId,
    pub gen: u64,
    pub error: String,
}
static PARSE_DONE_TX: OnceLock<crossbeam_channel::Sender<ParseDoneEnvelope>> = OnceLock::new();
static PARSE_DONE_RX: OnceLock<crossbeam_channel::Receiver<ParseDoneEnvelope>> = OnceLock::new();
static PARSE_FAILED_TX: OnceLock<crossbeam_channel::Sender<ParseFailedEnvelope>> = OnceLock::new();
static PARSE_FAILED_RX: OnceLock<crossbeam_channel::Receiver<ParseFailedEnvelope>> =
    OnceLock::new();

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

fn run_senders() -> &'static std::sync::Mutex<
    std::collections::HashMap<
        lunco_experiments::ExperimentId,
        crossbeam_channel::Sender<lunco_experiments::RunUpdate>,
    >,
> {
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

fn forward_run_update(
    run_id: lunco_experiments::ExperimentId,
    update: lunco_experiments::RunUpdate,
) {
    let tx = match run_senders()
        .lock()
        .ok()
        .and_then(|m| m.get(&run_id).cloned())
    {
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

fn ensure_parse_failed_channel() -> &'static crossbeam_channel::Sender<ParseFailedEnvelope> {
    PARSE_FAILED_TX.get_or_init(|| {
        let (tx, rx) = crossbeam_channel::unbounded();
        let _ = PARSE_FAILED_RX.set(rx);
        tx
    })
}

pub fn try_recv_parse_failed() -> Option<ParseFailedEnvelope> {
    let _ = ensure_parse_failed_channel();
    PARSE_FAILED_RX.get()?.try_recv().ok()
}

fn queue_parse_failure(doc_id: lunco_doc::DocumentId, gen: u64, error: impl Into<String>) {
    let _ = ensure_parse_failed_channel().send(ParseFailedEnvelope {
        doc_id,
        gen,
        error: error.into(),
    });
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
pub fn register_command_sender(tx_cmd: crossbeam_channel::Sender<ModelicaCommand>) -> bool {
    COMMAND_TX.set(tx_cmd).is_ok()
}

/// `true` once a JS Worker has been attached via [`install_worker`].
pub fn is_worker_active() -> bool {
    pool().lock().map(|p| p.is_active()).unwrap_or(false)
}

/// Wire up the JS Worker to the Rust result channel.
///
/// `worker_url` is the absolute or origin-relative URL to the worker JS
/// module (typically `./worker/lunica_worker.js`, generated by `wasm-bindgen
/// --target web`). The module is started as `type=module` so it can `import`
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
    let want =
        lunco_settings::load_section_from_disk::<crate::experiments_runner::ExperimentSettings>()
            .resolved_max_parallel()
            .clamp(1, MAX_WORKERS);

    let n = {
        let mut p = pool().lock_or_recover();
        if p.is_active() {
            // Already installed — keep the existing pool (idempotent).
            return Ok(());
        }
        // Build the generic pool with the Modelica handlers + boot-handshake wire
        // id, then spawn `want` workers (worker 0 fatal, later failures cap it).
        let mut inner = WorkerTransport::new(
            worker_url,
            Some(WIRE_BUILD_ID.to_string()),
            WIRE_HANDSHAKE_PREFIX,
            worker_callbacks(),
        );
        inner.ensure(want)?;
        let n = inner.len();
        p.running = vec![None; n];
        p.msl = vec![MslState::Absent; n];
        p.inner = Some(inner);
        n
    };
    // Retain the script URL so a crashed worker can be respawned in place.
    let _ = WORKER_URL.set(worker_url.to_string());

    if n < want {
        bevy::log::warn!("[worker_transport] requested {want} workers but only {n} started");
    }
    bevy::log::info!("[worker_transport] worker pool installed: {n} worker(s) at {worker_url}");
    Ok(())
}

/// The Modelica handlers wired into every worker of the [`WorkerTransport`] pool.
/// `on_message` routes each non-handshake message (a `WireResult` `Uint8Array`, or
/// the transferred decoded-MSL `ArrayBuffer`); `on_error` fails the worker's run +
/// respawns it; `on_wire_mismatch` flags a stale worker wasm; `on_ready` logs the
/// successful boot handshake.
fn worker_callbacks() -> Callbacks {
    Callbacks {
        on_message: Rc::new(|idx, data| route_wire_result(idx, data)),
        on_ready: Rc::new(|idx| {
            bevy::log::info!("[worker_transport] worker {idx} wire handshake OK ({WIRE_BUILD_ID})");
        }),
        on_error: Rc::new(|idx| handle_worker_error(idx)),
        on_wire_mismatch: Rc::new(|idx, got| {
            WIRE_MISMATCH.with(|c| c.set(true));
            let error = format!(
                "worker {idx} wire id {got} does not match main {WIRE_BUILD_ID}; rebuild the browser worker bundle in lockstep"
            );
            bevy::log::error!(
                "[worker_transport] STALE WORKER: worker {idx} wire id {got} != main \
                 {WIRE_BUILD_ID}. The shipped lunica_worker wasm was built from different \
                 source than this bundle — Modelica compile/run is BROKEN until it is rebuilt \
                 in lockstep. Run `WORKER_REBUILD=force ./scripts/build_web.sh sandbox`."
            );
            fail_worker_pipeline(error);
        }),
    }
}

/// Route one non-handshake worker message into the Modelica pipeline. The only bare
/// `ArrayBuffer` in the protocol is the worker shipping decompressed MSL bincode
/// back (transferred zero-copy); every other message is a `Uint8Array` of a bincode
/// [`WireResult`].
fn route_wire_result(idx: usize, data: JsValue) {
    if data.is_instance_of::<js_sys::ArrayBuffer>() {
        let buf: js_sys::ArrayBuffer = data.unchecked_into();
        let decoded = Uint8Array::new(&buf).to_vec();
        crate::msl_remote::ingest_worker_decoded_msl(decoded);
        return;
    }
    let bytes: Vec<u8> = match Uint8Array::new(&data).to_vec() {
        v if !v.is_empty() => v,
        _ => return,
    };
    match bincode::serde::decode_from_slice::<WireResult, _>(&bytes, bincode::config::standard())
        .map(|(m, _)| m)
    {
        Ok(WireResult::Result(result)) => {
            if let Some(tx) = RESULT_TX.get() {
                let _ = tx.send(result);
            }
        }
        Ok(WireResult::MslReady { docs }) => {
            // The worker decoded the compressed bundle off-thread and now has MSL in
            // its own session. Open the compile gate and drain everything queued
            // behind it. (Idempotent — a respawned worker re-seeded posts this again.)
            bevy::log::info!("[worker_transport] worker {idx} reports MSL ready: {docs} docs");
            mark_worker_msl_ready(idx);
            // Worker 0 just finished the boot decode → seed the secondary workers now.
            if idx == 0 {
                seed_secondary_workers();
            }
            flush_pending_commands();
            flush_pending_run_fast();
        }
        Ok(WireResult::MslIndexChunk {
            components,
            bundled,
            done,
        }) => {
            crate::msl_remote::ingest_worker_msl_index_chunk(components, bundled, done);
        }
        Ok(WireResult::MslIndexFailed { error }) => {
            crate::msl_remote::fail_worker_msl_index(error);
        }
        Ok(WireResult::MslFailed { error }) => {
            fail_worker_pipeline(error.clone());
            crate::msl_remote::fail_worker_msl(error);
        }
        Ok(WireResult::ParseDocumentDone {
            doc_id,
            gen,
            ast,
            errors,
        }) => {
            let tx = ensure_parse_done_channel();
            let _ = tx.send(ParseDoneEnvelope {
                doc_id,
                gen,
                ast,
                errors,
            });
        }
        Ok(WireResult::ParseDocumentFailed { doc_id, gen, error }) => {
            queue_parse_failure(doc_id, gen, error);
        }
        Ok(WireResult::RunUpdate { run_id, update }) => {
            forward_run_update(run_id, update);
        }
        Ok(WireResult::Log(line)) => {
            // Surface worker-side diagnostics in the main page's Console panel — the
            // worker has its own console context page DevTools can't see.
            bevy::log::info!("[worker] {line}");
        }
        Ok(WireResult::RecycleRequest { mem_mb }) => {
            // The worker's grow-only wasm memory climbed past the watermark. It's idle
            // now (this arrives after the run's terminal update), so retire + respawn
            // it to reset its linear memory. `respawn_worker` defers the MSL re-seed.
            bevy::log::info!(
                "[worker_transport] worker {idx} requested recycle at {mem_mb} MB — respawning"
            );
            respawn_worker(idx);
        }
        Err(e) => {
            bevy::log::error!("[worker_transport] failed to decode result: {e}");
        }
    }
}

/// Register the worker-script URL for later lazy spawning, WITHOUT starting any
/// worker. Called once at boot. The pool itself is spawned on first demand by
/// [`ensure_pool_spawned`] — see its doc for why the diagram must not wait on
/// the pool's startup.
pub fn register_worker_url(worker_url: &str) {
    let _ = WORKER_URL.set(worker_url.to_string());
}

/// Spawn the worker pool on first demand (a parse, compile, or Fast Run) if it isn't up
/// yet, and seed it with MSL. The pool is deliberately NOT spawned at boot:
/// parsing-for-diagram uses the same worker boundary as compile/run, so the
/// diagram never performs source parsing on the main thread. Worker startup is
/// asynchronous; the caller keeps the document pending until the worker is
/// ready. No-op once the pool is up.
pub fn ensure_pool_spawned() {
    if is_worker_active() {
        return;
    }
    let Some(url) = WORKER_URL.get() else {
        bevy::log::warn!("[worker_transport] ensure_pool_spawned: no worker URL registered");
        return;
    };
    if let Err(e) = install_worker(url) {
        bevy::log::error!("[worker_transport] lazy worker pool spawn failed: {e:?}");
        return;
    }
    // Seed the freshly-spawned pool when the generated MSL envelope is already
    // retained. Otherwise the MSL loader owns the first handoff once its fetch
    // completes. Worker 0 receives the main-copy transfer and later seeds any
    // configured secondary workers.
    let seeded = MSL_WIRE.with(|wire| {
        let wire = wire.borrow();
        let Some(env) = wire.as_deref() else {
            return false;
        };
        let mut p = pool().lock_or_recover();
        let posted = match p.inner.as_ref() {
            Some(inner) => inner.post(0, env),
            None => Err(JsValue::from_str("pool not installed")),
        };
        match posted {
            Ok(()) => {
                p.msl[0] = MslState::Decoding;
                true
            }
            Err(e) => {
                bevy::log::error!("[worker_transport] lazy MSL seed to worker 0 failed: {e:?}");
                false
            }
        }
    });
    bevy::log::info!(
        "[worker_transport] lazy worker pool spawned (msl_seeded={seeded}) — for compile/run"
    );
}

/// Pre-warm the worker pool the moment MSL is resident, so the user's FIRST
/// compile/run doesn't pay the full lazy cold start. Profiling (2026-06-24)
/// showed a first compile taking ~46 s wall-clock of which only ~4.5 s was the
/// actual compile — the rest was this pool's cold start (4× ~58 MB worker wasm
/// instantiate + worker-0 MSL decode) happening *after* the user clicked. The
/// diagram is rendered on the main thread and is already up by the time MSL is
/// ready, so warming the pool here overlaps that startup with the user reading
/// the diagram instead of blocking their first compile. The heavy work runs in
/// the worker threads; the main-thread cost here is just posting the seed.
///
/// SAFETY: only acts when the MSL seed envelope (`MSL_WIRE`) is already
/// retained. If it isn't, this is a no-op and the normal lazy path on the first
/// compile still applies — so this can never spawn an unseeded worker 0 that
/// would then never become `Ready` (which would hang every compile). Idempotent
/// via `ensure_pool_spawned`'s `is_worker_active` guard.
pub fn prewarm_pool_on_msl_ready() {
    let has_wire = MSL_WIRE.with(|wire| wire.borrow().is_some());
    if !has_wire {
        // Seed not retained yet — leave it to the lazy first-compile path,
        // which seeds worker 0 itself. Never spawn an unseeded pool here.
        return;
    }
    ensure_pool_spawned();
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
    if let Some(handle) = crate::engine_resource::global_engine_handle() {
        handle.clear_all_pending();
    }
    let crashed_run = {
        let p = pool().lock_or_recover();
        p.running.get(idx).copied().flatten()
    };
    if let Some(run_id) = crashed_run {
        bevy::log::warn!("[worker_transport] failing run {run_id:?} after worker {idx} crash");
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
    {
        let mut p = pool().lock_or_recover();
        let Some(inner) = p.inner.as_mut() else {
            bevy::log::error!("[worker_transport] cannot respawn worker {idx}: pool not installed");
            return;
        };
        if let Err(e) = inner.respawn(idx) {
            bevy::log::error!("[worker_transport] respawn of worker {idx} failed: {e:?}");
            return;
        }
        if let Some(r) = p.running.get_mut(idx) {
            *r = None;
        }
        // The fresh worker has no MSL until `pump_worker_respawns` re-seeds it;
        // run dispatch skips non-`Ready` workers, so it won't be handed a run
        // before its `MslReady` arrives.
        if let Some(s) = p.msl.get_mut(idx) {
            *s = MslState::Absent;
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
    let Some(bytes) = MSL_WIRE.with(|wire| wire.borrow().clone()) else {
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
        let _ = post_bytes_to(idx, &bytes, "respawn MSL reinstall (deferred)");
        // The re-seed envelope is `provide_to_main = false` (main already holds
        // the decoded bundle), and its `MslReady` flips this worker back to
        // `Ready`.
        if let Ok(mut p) = pool().lock() {
            if let Some(s) = p.msl.get_mut(idx) {
                *s = MslState::Decoding;
            }
        }
        bevy::log::info!("[worker_transport] re-seeded MSL into respawned worker {idx}");
    }
}

/// Drain `ModelicaChannels.rx_cmd` and ship each command to the JS worker.
///
/// Bevy system. Runs every `Update`. Cheap when the queue is empty; when it
/// isn't, each command is bincode-encoded and posted as a `Uint8Array`. The
/// worker's `process_worker_command` runs off the UI thread and posts results
/// back asynchronously via `onmessage` (see [`install_worker`]).
pub fn pump_commands_to_worker(channels: Res<ModelicaChannels>) {
    if channels.rx_cmd.is_empty() {
        return;
    }
    if let Some(error) = pipeline_failure() {
        while let Ok(cmd) = channels.rx_cmd.try_recv() {
            send_command_failure(&cmd, &format!("Modelica worker pipeline failed: {error}"));
        }
        return;
    }
    ensure_pool_spawned();
    if !is_worker_active() {
        let error = "Modelica Web Worker is unavailable; rebuild the browser worker bundle";
        while let Ok(cmd) = channels.rx_cmd.try_recv() {
            send_command_failure(&cmd, error);
        }
        return;
    }
    while let Ok(cmd) = channels.rx_cmd.try_recv() {
        post_command_or_queue(cmd);
    }
}

/// Post a command to the primary worker (0) — compile / parse / step traffic
/// always goes there so a Fast Run fanned out to other workers can't reorder
/// it — or re-queue it when that worker can't serve it yet.
///
/// The gate is worker 0's OWN [`MslState`], mirroring the Fast Run dispatch in
/// [`assign_and_post_run_fast`]: readiness is a per-worker fact. A command is
/// sent only when the primary worker has its MSL session populated, so a
/// crash-respawn cannot expose an empty session to Compile or UpdateParameters.
/// Commands that need no MSL (Step / Reset / Despawn) pass through while the
/// worker is healthy.
///
/// Re-queued commands leave on the next `MslReady` event; this cannot recurse
/// because [`flush_pending_commands`] iterates a drained snapshot.
fn post_command_or_queue(cmd: ModelicaCommand) {
    if let Some(error) = pipeline_failure() {
        send_command_failure(&cmd, &format!("Modelica worker pipeline failed: {error}"));
        return;
    }
    if command_needs_msl(&cmd) && !primary_msl_ready() {
        bevy::log::debug!(
            "[worker_transport] primary worker MSL not Ready — queued compile-path command"
        );
        PENDING_COMMANDS.with(|q| q.borrow_mut().push(cmd));
        return;
    }
    let message = WireMessage::Command(cmd);
    if let Err(error) = post_msg_to(0, &message, "command") {
        if let WireMessage::Command(cmd) = message {
            send_command_failure(
                &cmd,
                &format!("failed to dispatch Modelica command: {error}"),
            );
        }
    }
}

/// Whether worker 0 — the only worker compile-path traffic is posted to — has
/// the MSL bundle live in its session *right now*.
fn primary_msl_ready() -> bool {
    let p = pool().lock_or_recover();
    p.worker_count() > 0 && matches!(p.msl.first(), Some(MslState::Ready))
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
    inputs: std::collections::BTreeMap<lunco_experiments::ParamPath, lunco_experiments::ParamValue>,
    bounds: lunco_experiments::RunBounds,
) -> bool {
    if let Some(error) = pipeline_failure() {
        send_run_failure(run_id, format!("Modelica worker pipeline failed: {error}"));
        return true;
    }
    // A Fast Run is the heavy work the worker pool exists for — spawn it now if
    // it isn't up yet. The run then queues behind MSL installation below.
    ensure_pool_spawned();
    if !is_worker_active() {
        send_run_failure(
            run_id,
            "Modelica Web Worker is unavailable; rebuild the browser worker bundle",
        );
        return true;
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
    if !primary_msl_ready() {
        // Queue whole-message; worker assignment happens at flush time.
        PENDING_RUN_FAST.with(|q| q.borrow_mut().push(msg));
        return true;
    }
    assign_and_post_run_fast(run_id, msg);
    true
}

/// Pick a worker for `run_id` and post the (already-built) RunFast message
/// to it. Prefers a free non-primary worker so worker 0 stays available for
/// compiles; falls back to worker 0 (serializing behind its current work)
/// when every worker is busy. Records the `run_id → worker` mapping for
/// cancel routing; the slot is freed in [`forward_run_update`] on terminal.
fn assign_and_post_run_fast(run_id: lunco_experiments::ExperimentId, msg: WireMessage) {
    let bytes = match bincode::serde::encode_to_vec(&msg, bincode::config::standard()) {
        Ok(b) => b,
        Err(e) => {
            bevy::log::error!("[worker_transport] run_fast: serialize failed: {e}");
            send_run_failure(run_id, format!("failed to serialize Modelica run: {e}"));
            return;
        }
    };
    let mut p = pool().lock_or_recover();
    let n = p.worker_count();
    if n == 0 {
        drop(p);
        send_run_failure(run_id, "Modelica Web Worker pool is unavailable");
        return;
    }
    // Prefer a free, MSL-`Ready` worker that isn't the primary (1..n), else the
    // primary. A worker is eligible only when its MSL is `Ready`: that skips
    // `Absent`/`Decoding` secondaries (seeded only after worker 0 finishes the
    // boot decode — until then their session is empty and a compile/run would
    // fail with unresolved `Modelica.*`) AND recycle-respawn re-seeds (a
    // reseed-pending worker is Absent/Decoding, never Ready — so no separate
    // `is_reseed_pending` check is needed). If every Ready worker is busy,
    // serialize behind a Ready one (incl. the primary).
    let ready_free = |p: &WorkerPool| {
        (1..n)
            .chain(std::iter::once(0))
            .find(|&i| p.running[i].is_none() && p.msl[i] == MslState::Ready)
    };
    let chosen = ready_free(&p).or_else(|| {
        (1..n)
            .chain(std::iter::once(0))
            .find(|&i| p.msl[i] == MslState::Ready)
    });
    let Some(idx) = chosen else {
        // No MSL-`Ready` worker right now — e.g. worker 0 mid-recycle re-decode.
        // Dispatching to a
        // non-Ready worker would compile against an empty session and fail with
        // unresolved `Modelica.*`. Re-queue instead; the next worker's
        // `MslReady` re-runs `flush_pending_run_fast`. (Cannot recurse: flush
        // iterates a drained snapshot, so a re-queued run waits for the next
        // readiness event.)
        drop(p);
        PENDING_RUN_FAST.with(|q| q.borrow_mut().push(msg));
        bevy::log::info!(
            "[worker_transport] run_fast: no MSL-Ready worker yet — re-queued run {run_id:?}"
        );
        return;
    };
    if p.running[idx].is_none() {
        p.running[idx] = Some(run_id);
    }
    p.run_to_worker.insert(run_id, idx);
    // Post inside the lock — wasm is single-threaded, so there's no
    // re-entrancy and no other code can observe a half-updated pool.
    let post_error = p
        .inner
        .as_ref()
        .ok_or_else(|| "worker pool disappeared".to_string())
        .and_then(|inner| inner.post(idx, &bytes).map_err(|e| format!("{e:?}")))
        .err();
    if let Some(error) = post_error {
        bevy::log::error!("[worker_transport] run_fast: post to worker {idx} failed: {error}");
        p.run_to_worker.remove(&run_id);
        if p.running.get(idx).copied().flatten() == Some(run_id) {
            p.running[idx] = None;
        }
        drop(p);
        send_run_failure(run_id, format!("failed to dispatch Modelica run: {error}"));
    }
}

/// Cancel an in-flight Fast Run. Best-effort; latency depends on the
/// worker's poll cadence. Routed to the worker that owns the run; if the
/// mapping is unknown (e.g. still queued behind MSL install) it broadcasts
/// to every worker (a no-op in the ones not running it).
pub fn dispatch_cancel_run(run_id: lunco_experiments::ExperimentId) {
    let (target, n) = {
        let p = pool().lock_or_recover();
        (p.run_to_worker.get(&run_id).copied(), p.worker_count())
    };
    if n == 0 {
        return;
    }
    match target {
        Some(idx) => {
            let _ = post_msg_to(idx, &WireMessage::CancelRun { run_id }, "cancel_run");
        }
        None => {
            for i in 0..n {
                let _ = post_msg_to(i, &WireMessage::CancelRun { run_id }, "cancel_run(bcast)");
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
    for msg in drained {
        if let WireMessage::RunFast { run_id, .. } = &msg {
            let run_id = *run_id;
            assign_and_post_run_fast(run_id, msg);
        }
    }
}

/// Drain any compile-path commands queued by `pump_commands_to_worker`
/// while the MSL install was still pending. Called from
/// the worker reports MSL ready.
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
        // Same readiness gate as the live path: a flush triggered by a
        // secondary's `MslReady` says nothing about worker 0, so a command can
        // legitimately go straight back on the queue.
        post_command_or_queue(cmd);
    }
}

/// Serialize and post a `WireMessage` to the primary worker (0).
fn post_to_worker(msg: &WireMessage, label: &str) {
    let _ = post_msg_to(0, msg, label);
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
// Empirically the worker can be bootstrapped (the generated JS module has loaded
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
// `wasm32-unknown-unknown` is single-threaded so a `RefCell` in a
// `thread_local!` is enough — no `Mutex` needed.
#[cfg(target_arch = "wasm32")]
thread_local! {
    /// Terminal MSL failure for this app lifetime. Generated MSL artifacts are
    /// part of the runtime contract; once they fail, requests complete with a
    /// visible error instead of being retried or silently reparsed.
    static PIPELINE_FAILURE: std::cell::RefCell<Option<String>> =
        const { std::cell::RefCell::new(None) };
    // Commands that need MSL resolved (Compile and UpdateParameters) queue
    // until the primary worker is Ready.
    static PENDING_COMMANDS: std::cell::RefCell<Vec<ModelicaCommand>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Commands that depend on a populated MSL index in the worker (compile-path
/// commands). Sent before MSL install lands → silent "unresolved Modelica.*"
/// failures. Gate them; drain on the worker's MSL-ready event.
fn command_needs_msl(cmd: &ModelicaCommand) -> bool {
    matches!(
        cmd,
        ModelicaCommand::Compile { .. } | ModelicaCommand::UpdateParameters { .. }
    )
}

#[cfg(target_arch = "wasm32")]
fn pipeline_failure() -> Option<String> {
    PIPELINE_FAILURE.with(|failure| failure.borrow().clone())
}

#[cfg(target_arch = "wasm32")]
fn send_command_failure(cmd: &ModelicaCommand, error: &str) {
    if let Some(tx) = RESULT_TX.get() {
        let _ = tx.send(crate::worker::failed_result_for_command(cmd, error));
    }
}

#[cfg(target_arch = "wasm32")]
fn send_run_failure(run_id: lunco_experiments::ExperimentId, error: impl Into<String>) {
    forward_run_update(
        run_id,
        lunco_experiments::RunUpdate::Failed {
            error: error.into(),
            partial: None,
        },
    );
}

/// Fail the MSL pipeline and every request waiting for it. This is the single
/// terminal owner for worker-side artifact failures; no caller may fall back to
/// source reparsing or leave a request pending forever.
#[cfg(target_arch = "wasm32")]
pub fn fail_worker_pipeline(error: String) {
    let is_new = PIPELINE_FAILURE.with(|failure| {
        let mut failure = failure.borrow_mut();
        if failure.is_some() {
            false
        } else {
            *failure = Some(error.clone());
            true
        }
    });
    if !is_new {
        return;
    }

    let pending_commands = PENDING_COMMANDS.with(|q| q.borrow_mut().drain(..).collect::<Vec<_>>());
    for cmd in pending_commands {
        send_command_failure(&cmd, &error);
    }
    let pending_runs = PENDING_RUN_FAST.with(|q| q.borrow_mut().drain(..).collect::<Vec<_>>());
    for msg in pending_runs {
        if let WireMessage::RunFast { run_id, .. } = msg {
            send_run_failure(run_id, format!("Modelica worker pipeline failed: {error}"));
        }
    }
}

/// Start a new explicit MSL installation attempt after the user requests a
/// reinstall. The next artifact handoff replaces the retained worker envelope
/// and returns every worker to the pre-install state.
#[cfg(target_arch = "wasm32")]
pub fn reset_worker_pipeline() {
    PIPELINE_FAILURE.with(|failure| *failure.borrow_mut() = None);
    if let Ok(mut pool) = pool().lock() {
        for state in &mut pool.msl {
            *state = MslState::Absent;
        }
    }
    MSL_WIRE.with(|wire| *wire.borrow_mut() = None);
}

// Send a doc to the worker for off-thread parsing. Used by
// `engine_resource::drive_engine_sync` on wasm in place of the
// main-thread parse spawn. The result lands via the parse-done
// channel ([`try_recv_parse_done`]). Source parsing is never performed on
// the UI thread.
thread_local! {
    static NEXT_PARSE_WORKER: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub fn dispatch_parse_to_worker(
    doc_id: lunco_doc::DocumentId,
    gen: u64,
    uri: String,
    source: String,
) {
    if let Some(error) = pipeline_failure() {
        queue_parse_failure(doc_id, gen, format!("Modelica worker unavailable: {error}"));
        return;
    }
    ensure_pool_spawned();
    if !is_worker_active() {
        queue_parse_failure(
            doc_id,
            gen,
            "Modelica Web Worker is unavailable; rebuild the browser worker bundle",
        );
        return;
    }
    // Parsing is independent of the MSL session, so it may be queued behind
    // worker startup or a compile. Posting to the worker preserves ordering
    // and keeps every parser invocation off the UI thread.
    let idx = {
        let p = pool().lock_or_recover();
        let n = p.worker_count();
        if n == 0 {
            queue_parse_failure(
                doc_id,
                gen,
                "Modelica Web Worker pool has no active workers",
            );
            return;
        }
        let start = NEXT_PARSE_WORKER.with(|c| {
            let s = c.get();
            c.set((s + 1) % n);
            s % n
        });
        (0..n)
            .map(|k| (start + k) % n)
            .find(|&i| p.msl[i] == MslState::Ready && p.running[i].is_none())
            .unwrap_or(0)
    };
    let message = WireMessage::ParseDocument {
        doc_id,
        gen,
        uri,
        source,
    };
    if let Err(error) = post_msg_to(idx, &message, "parse") {
        queue_parse_failure(
            doc_id,
            gen,
            format!("failed to dispatch Modelica parse: {error}"),
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
        doc_uri: "model.mo".to_string(),
        extra_sources: Vec::new(),
        parameter_overrides: Vec::new(),
        stream: None,
        // A transport smoke-test compile drives no predicted body.
        realtime_safe: false,
    };
    if let Err(e) = tx.send(cmd) {
        bevy::log::error!("[worker_transport] dispatch_compile: send failed: {e}");
    }
}

/// Ship the MSL bundle to the worker(s) as the raw **compressed**
/// `parsed-*.bin.zst` bytes. The worker decompresses + bincode-decodes off the
/// main thread and replies with [`WireResult::MslReady`], which opens the
/// compile gate (see the `onmessage` handler in [`make_worker`]).
///
/// This is the boot install path. It does not open the gate here — the worker
/// hasn't decoded yet, so compiles must keep
/// queuing until `MslReady` arrives. The retained `MSL_WIRE` bytes (used to
/// re-seed a respawned worker) are now the ~19 MB compressed envelope rather
/// than the ~165 MB decoded one, so the post-OOM re-seed is far lighter too.
///
/// Returns the number of workers the bundle was shipped to. A zero result is a
/// terminal transport failure; the caller must surface it instead of decoding
/// the artifact on the main thread.
pub fn install_msl_compressed_in_worker(compressed: &[u8]) -> usize {
    // The generated MSL artifact is installed only after a real worker exists.
    // Its decompression and deserialization are owned by that worker, never by
    // the UI thread.
    ensure_pool_spawned();
    // Serialize an envelope for one worker. Only the primary (worker 0) is asked
    // to ship the decoded bytes back to main (`provide_to_main`); the rest decode
    // for their own compiles and skip the transfer the main thread would dedupe.
    let encode = |provide_to_main: bool| -> Option<Vec<u8>> {
        match bincode::serde::encode_to_vec(
            &WireMessage::InstallParsedMslCompressed {
                bytes: compressed.to_vec(),
                provide_to_main,
            },
            bincode::config::standard(),
        ) {
            Ok(b) => Some(b),
            Err(e) => {
                bevy::log::error!("[worker_transport] encode compressed MSL install failed: {e}");
                None
            }
        }
    };

    // Retain ONE `provide_to_main = false` envelope (~16 MB), reused for both
    // secondary seeding (`seed_secondary_workers`) and crash-respawn re-seed
    // (`pump_worker_respawns`). `false` is correct for every (re)seed: main gets
    // the decoded bundle from worker 0's boot ship below, so no (re)seeded
    // worker needs to re-ship ~165 MB to main. Pre-serialized once here so the seed path doesn't
    // clone+serialize the bundle on the `MslReady` callback.
    let Some(reseed_wire) = encode(false) else {
        fail_worker_pipeline("failed to serialize the generated MSL artifact".to_string());
        return 0;
    };
    MSL_WIRE.with(|wire| *wire.borrow_mut() = Some(reseed_wire));

    // Ship to worker 0 ONLY. Broadcasting this multi-second decode to every
    // worker saturates the whole pool and starves the freshly-opened model's
    // parse — the reported "no diagram until MSL downloads" bug. Worker 0
    // decodes alone (a full
    // core, so faster) and ships the decoded bytes back to main; the
    // secondaries stay free to parse and are seeded later, once worker 0
    // reports `MslReady`. Zero-copy transfer (single recipient, so detaching
    // the buffer is safe).
    let Some(bytes) = encode(true) else { return 0 };
    let posted = {
        let mut p = pool().lock_or_recover();
        if !p.is_active() {
            return 0;
        }
        let array = Uint8Array::new_with_length(bytes.len() as u32);
        array.copy_from(&bytes);
        let transfer = js_sys::Array::new();
        transfer.push(&array.buffer());
        let res = p
            .inner
            .as_ref()
            .unwrap()
            .post_transfer(0, &array, &transfer);
        match res {
            Ok(()) => {
                p.msl[0] = MslState::Decoding;
                true
            }
            Err(e) => {
                bevy::log::error!(
                    "[worker_transport] compressed MSL install to worker 0 failed: {e:?}"
                );
                false
            }
        }
    };
    if !posted {
        return 0;
    }
    bevy::log::info!(
        "[worker_transport] shipped compressed MSL to worker 0 only (~{} bytes, zero-copy \
         transfer); secondaries seed after worker 0 is ready — awaiting off-thread decode \
         (gate opens on MslReady)",
        bytes.len()
    );
    1
}

/// Ask worker 0 to decode the generated editor index from the retained source
/// bundle. This is used by the pre-parsed fast path, where source bytes are
/// otherwise kept only for lazy drill-in.
pub fn load_msl_index_in_worker(compressed: &[u8]) -> usize {
    let bytes = match bincode::serde::encode_to_vec(
        &WireMessage::InstallMslIndexFromSource {
            bytes: compressed.to_vec(),
        },
        bincode::config::standard(),
    ) {
        Ok(b) => b,
        Err(e) => {
            bevy::log::error!("[worker_transport] encode MSL index request failed: {e}");
            return 0;
        }
    };
    let posted = {
        let p = pool().lock_or_recover();
        if !p.is_active() {
            return 0;
        }
        let array = Uint8Array::new_with_length(bytes.len() as u32);
        array.copy_from(&bytes);
        let transfer = js_sys::Array::new();
        transfer.push(&array.buffer());
        p.inner
            .as_ref()
            .unwrap()
            .post_transfer(0, &array, &transfer)
            .map(|()| 1usize)
            .unwrap_or_else(|e| {
                bevy::log::error!("[worker_transport] MSL index request to worker 0 failed: {e:?}");
                0
            })
    };
    if posted > 0 {
        bevy::log::info!(
            "[worker_transport] shipped compressed MSL editor index source to worker 0 (~{} bytes)",
            compressed.len()
        );
    }
    posted
}

/// Whether to eagerly preload (MSL-seed) the secondary Fast-Run workers at boot.
/// Off unless the page sets `window.__lc_worker_preload = true` — see the rationale
/// in [`seed_secondary_workers`]. Reads the flag live (cheap) so a deploy can flip
/// it without a rebuild.
fn worker_preload_enabled() -> bool {
    web_sys::window()
        .and_then(|w| {
            js_sys::Reflect::get(&w, &wasm_bindgen::JsValue::from_str("__lc_worker_preload")).ok()
        })
        .map(|v| v.is_truthy())
        .unwrap_or(false)
}

/// Seed the secondary workers (`1..n`) with the retained `MSL_WIRE` envelope
/// (`provide_to_main = false` — worker 0 already gave the decoded bytes to
/// main). Called from worker 0's `MslReady` so the pool ramps up to full
/// Fast-Run parallelism *after* boot, without having saturated the pool while
/// the freshly-opened model was parsing. Idempotent — only seeds workers still
/// in `Absent` state. Copies the pre-serialized envelope directly (no
/// clone+serialize on this callback).
fn seed_secondary_workers() {
    // OPT-IN preload. Seeding the full parsed MSL (~140 MB) into every secondary
    // worker at boot is what gives Fast-Run its parallelism — but cloning it N×
    // via `postMessage` costs hundreds of MB and OOMs a memory-constrained tab
    // (e.g. the web sandbox showing a heavy 3D twin: "secondary MSL seed to
    // worker 3 failed: out of memory"). So it is OFF by default: only worker 0 is
    // seeded (single compiles/runs work; `dispatch_run_fast` just serializes Fast
    // Runs on worker 0 instead of fanning out). Turn it back on for full parallel
    // Fast-Run by setting `window.__lc_worker_preload = true` before the app boots
    // (see index.html). Crash-respawn re-seed of an already-Ready worker is a
    // separate path (`pump_worker_respawns`) and is unaffected.
    if !worker_preload_enabled() {
        return;
    }
    let mut seeded = 0usize;
    MSL_WIRE.with(|wire| {
        let wire = wire.borrow();
        let Some(env) = wire.as_deref() else {
            return;
        };
        let mut p = pool().lock_or_recover();
        let n = p.worker_count();
        for i in 1..n {
            if p.msl[i] != MslState::Absent || is_reseed_pending(i) {
                continue;
            }
            let res = p.inner.as_ref().unwrap().post(i, env);
            match res {
                Ok(()) => {
                    p.msl[i] = MslState::Decoding;
                    seeded += 1;
                }
                Err(e) => bevy::log::error!(
                    "[worker_transport] secondary MSL seed to worker {i} failed: {e:?}"
                ),
            }
        }
    });
    if seeded > 0 {
        bevy::log::info!(
            "[worker_transport] seeding MSL into {seeded} secondary worker(s) for Fast-Run \
             parallelism"
        );
    }
}

/// Mark worker `idx` MSL-`Ready` (it can now compile / run a Fast Run).
fn mark_worker_msl_ready(idx: usize) {
    if let Ok(mut p) = pool().lock() {
        if let Some(s) = p.msl.get_mut(idx) {
            *s = MslState::Ready;
        }
    }
}
