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
use std::sync::OnceLock;

use bevy::prelude::*;
use crossbeam_channel::Sender;
use js_sys::Uint8Array;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{MessageEvent, Worker};

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
        errors: Vec<String>,
    },
    /// Lifecycle update for a Fast Run started via
    /// `WireMessage::RunFast`. The `run_id` lets the main thread
    /// demux to the right `RunHandle` receiver.
    RunUpdate {
        run_id: lunco_experiments::ExperimentId,
        update: lunco_experiments::RunUpdate,
    },
}

/// Process-wide handle to the JS `Worker` running the off-thread Modelica
/// pipeline. Set once at startup by [`install_worker`]; used by
/// [`pump_commands_to_worker`] to relay commands.
///
/// `Worker` is `!Send + !Sync` because it carries a `JsValue`, but
/// wasm32-unknown-unknown is single-threaded so this is vacuously safe — we
/// only ever touch it from the main thread. The `OnceLock` is simply a
/// late-initialised global; the `WorkerHandle` newtype wraps `Worker` so we
/// can `unsafe impl Send + Sync` for it.
struct WorkerHandle(Worker);
// SAFETY: wasm32-unknown-unknown has no threads. JsValue (and Worker) only
// live on the main thread; OnceLock requires Send+Sync but we never cross
// threads in practice.
unsafe impl Send for WorkerHandle {}
unsafe impl Sync for WorkerHandle {}

static WORKER: OnceLock<WorkerHandle> = OnceLock::new();

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
    pub errors: Vec<String>,
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

thread_local! {
    /// Holds the `onmessage` closure for the lifetime of the page so the
    /// callback isn't dropped as soon as `install_worker` returns. Storing
    /// in a `thread_local` keeps the borrow-checker happy without a
    /// process-wide static (which would need `Send`).
    static ONMESSAGE_CB: RefCell<Option<Closure<dyn FnMut(MessageEvent)>>> = RefCell::new(None);
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
    WORKER.get().is_some()
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
pub fn install_worker(worker_url: &str) -> Result<(), JsValue> {
    let mut opts = web_sys::WorkerOptions::new();
    opts.set_type(web_sys::WorkerType::Module);
    let worker = Worker::new_with_options(worker_url, &opts)?;

    // Wire the worker's `onmessage` into a closure that decodes a
    // ModelicaResult and pushes it into the Rust channel that
    // `handle_modelica_responses` already drains. Each message is a
    // single bincode-serialized result.
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
            Err(e) => {
                bevy::log::error!("[worker_transport] failed to decode result: {e}");
            }
        }
    }) as Box<dyn FnMut(MessageEvent)>);

    worker.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));

    // Stash the closure so it isn't dropped (it owns the JS-side function
    // pointer; dropping it would un-register the handler).
    ONMESSAGE_CB.with(|slot| {
        *slot.borrow_mut() = Some(onmessage);
    });

    let _ = WORKER.set(WorkerHandle(worker));
    bevy::log::info!("[worker_transport] worker installed: {worker_url}");
    Ok(())
}

/// Drain `ModelicaChannels.rx_cmd` and ship each command to the JS worker.
///
/// Bevy system. Runs every `Update`. Cheap when the queue is empty; when it
/// isn't, each command is bincode-encoded and posted as a `Uint8Array`. The
/// worker's `process_inline_command` runs in its own thread and posts results
/// back asynchronously via `onmessage` (see [`install_worker`]).
pub fn pump_commands_to_worker(channels: Res<ModelicaChannels>) {
    let Some(WorkerHandle(worker)) = WORKER.get() else {
        // install_worker hasn't run yet — main app is mid-bootstrap.
        // Commands stay in the channel; we'll catch them next tick.
        return;
    };

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
        let envelope = WireMessage::Command(cmd);
        let bytes = match bincode::serialize(&envelope) {
            Ok(b) => b,
            Err(e) => {
                bevy::log::error!("[worker_transport] failed to encode command: {e}");
                continue;
            }
        };
        let array = Uint8Array::new_with_length(bytes.len() as u32);
        array.copy_from(&bytes);
        if let Err(e) = worker.post_message(&array) {
            bevy::log::error!("[worker_transport] post_message failed: {e:?}");
        }
    }
}

/// Post a Fast Run request to the worker. Gated behind MSL install
/// just like compiles — without MSL the worker's compile would emit
/// silent "unresolved Modelica.*" failures.
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
    if WORKER.get().is_none() {
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
        // Queue: convert the WireMessage back to bytes and stash. We
        // can't reuse PENDING_COMMANDS (different envelope shape), so
        // route through a dedicated queue.
        PENDING_RUN_FAST.with(|q| q.borrow_mut().push(msg));
        return true;
    }
    post_to_worker(&msg, "run_fast");
    true
}

/// Cancel an in-flight Fast Run. Best-effort; latency depends on the
/// worker's poll cadence.
pub fn dispatch_cancel_run(run_id: lunco_experiments::ExperimentId) {
    if WORKER.get().is_none() {
        return;
    }
    post_to_worker(&WireMessage::CancelRun { run_id }, "cancel_run");
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
        post_to_worker(&msg, "run_fast(flushed)");
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

fn post_to_worker_bytes(bytes: &[u8], label: &str) {
    let Some(WorkerHandle(worker)) = WORKER.get() else {
        bevy::log::warn!("[worker_transport] {label}: worker not installed");
        return;
    };
    let array = Uint8Array::new_with_length(bytes.len() as u32);
    array.copy_from(bytes);
    if let Err(e) = worker.post_message(&array) {
        bevy::log::error!("[worker_transport] {label}: post_message failed: {e:?}");
    }
}

fn post_to_worker(msg: &WireMessage, label: &str) {
    let Some(WorkerHandle(worker)) = WORKER.get() else {
        bevy::log::warn!("[worker_transport] {label}: worker not installed");
        return;
    };
    let bytes = match bincode::serialize(msg) {
        Ok(b) => b,
        Err(e) => {
            bevy::log::error!("[worker_transport] {label}: serialize failed: {e}");
            return;
        }
    };
    let array = Uint8Array::new_with_length(bytes.len() as u32);
    array.copy_from(&bytes);
    if let Err(e) = worker.post_message(&array) {
        bevy::log::error!("[worker_transport] {label}: post_message failed: {e:?}");
    }
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
    if WORKER.get().is_none() {
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
/// Uses `postMessage(message, [transfer])` so the `ArrayBuffer` ownership
/// is *moved* into the worker instead of structured-cloned. Without this
/// the main thread spends ~1–2 s memcpying the 165 MB encoded bundle
/// when MSL install fires — visible as a UI stutter on first page load.
/// The transfer call detaches the source `ArrayBuffer` immediately;
/// the worker receives it with no extra allocation.
pub fn install_msl_in_worker(
    parsed: &[(String, rumoca_compile::parsing::StoredDefinition)],
) {
    let Some(WorkerHandle(worker)) = WORKER.get() else { return };
    let envelope = WireMessage::InstallParsedMsl(parsed.to_vec());
    let bytes = match bincode::serialize(&envelope) {
        Ok(b) => b,
        Err(e) => {
            bevy::log::error!("[worker_transport] encode MSL install failed: {e}");
            return;
        }
    };
    let len = bytes.len();
    let array = Uint8Array::new_with_length(len as u32);
    array.copy_from(&bytes);
    let transfer = js_sys::Array::new();
    transfer.push(&array.buffer());
    if let Err(e) = worker.post_message_with_transfer(&array, &transfer) {
        bevy::log::error!("[worker_transport] post_message_with_transfer MSL install failed: {e:?}");
    } else {
        bevy::log::info!(
            "[worker_transport] installed MSL into worker: {} docs ({} bytes wire, transferred)",
            parsed.len(),
            len
        );
        // Open the parse-request gate now that the worker has its
        // index. Any parse request that came in earlier this session
        // was buffered in `PENDING_PARSES`; drain it.
        #[cfg(target_arch = "wasm32")]
        {
            MSL_INSTALLED.with(|c| c.set(true));
            flush_pending_parses();
            flush_pending_commands();
            flush_pending_run_fast();
        }
    }
}
