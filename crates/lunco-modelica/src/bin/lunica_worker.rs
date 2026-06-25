//! Off-thread Modelica worker — wasm32-unknown-unknown only.
//!
//! Runs inside a Web Worker with its own wasm linear memory. Listens for
//! bincode-serialized `ModelicaCommand` messages from the main page, drives
//! them through the same `worker::process_inline_command` dispatch the inline
//! path uses, and `postMessage`s each `ModelicaResult` back.
//!
//! Why a separate bin
//! ------------------
//! `wasm32-unknown-unknown` has no OS threads, so any rumoca compile that
//! takes seconds blocks the UI. Putting the dispatch behind a Web Worker —
//! which is a separate JS thread with a separate wasm instance — moves the
//! blocking work off the page's main thread without needing nightly Rust
//! atomics or `SharedArrayBuffer`. The native build is unchanged: it still
//! uses `worker::modelica_worker` on a real `std::thread`.
//!
//! State
//! -----
//! One `InlineWorkerInner` per worker bundle; lives for the lifetime of the
//! page. State (steppers, DAE cache, lazy `ModelicaCompiler`) survives across
//! postMessage round-trips so back-to-back Step commands hit the warm
//! stepper without any re-compile cost.
//!
//! MSL
//! ---
//! TODO(arch-msl-handoff): the worker needs MSL to be present in its own
//! `GLOBAL_PARSED_MSL` slot before the first Compile resolves any
//! `Modelica.*` reference. The minimum-viable path is to have the main
//! app send an `InstallParsedMsl(Vec<(String, StoredDefinition)>)` envelope
//! to the worker once its own MSL fetch lands; the worker decodes and
//! installs. That requires a `WireMessage` envelope around `ModelicaCommand`
//! (variants: `Command(ModelicaCommand)` / `InstallMsl(...)`). Until that's
//! wired, the worker compiles will fail with "unresolved reference
//! Modelica.*" — the channel architecture is still verifiable by sending a
//! Compile of a self-contained model that doesn't reference MSL.

// Wasm32-only binary; the desktop stub below keeps `cargo build` for the
// host target passing without producing a meaningful executable.
fn main() {
    #[cfg(not(target_arch = "wasm32"))]
    panic!("lunica_worker is wasm32-only — built into a Web Worker bundle by scripts/build_web.sh.");
}

// NOTE: an experiment swapping the global allocator to `talc` (low-frag) made
// memory use STRICTLY WORSE here — runs that complete under the default wasm
// `dlmalloc` (e.g. Rover 1e-3 / t_end=1e5) OOM'd under talc, and even took the
// whole renderer down. So `dlmalloc` (the default) is kept. The 4 GiB OOM on
// the heavy stiff run is allocator-sensitive but talc is not the answer; see
// memory `project_wasm_4gb_worker_isolation`.

#[cfg(target_arch = "wasm32")]
mod wasm {
use std::cell::RefCell;

use js_sys::Uint8Array;
use lunco_modelica::worker::{ModelicaCommand, ModelicaResult};
use lunco_modelica::worker_transport::{WireMessage, WireResult};

fn command_label(cmd: &ModelicaCommand) -> String {
    match cmd {
        ModelicaCommand::Step { model_name, entity, .. } => format!("Step {model_name} entity={entity:?}"),
        ModelicaCommand::Compile { model_name, entity, .. } => format!("Compile {model_name} entity={entity:?}"),
        ModelicaCommand::UpdateParameters { model_name, entity, .. } => format!("UpdateParameters {model_name} entity={entity:?}"),
        ModelicaCommand::Reset { entity, .. } => format!("Reset entity={entity:?}"),
        ModelicaCommand::Despawn { entity } => format!("Despawn entity={entity:?}"),
        ModelicaCommand::LoadSourceRoot { id, .. } => format!("LoadSourceRoot id={id}"),
    }
}

/// `(entity, session_id)` for the in-flight command, so a panic-recovery
/// path can synthesize a `ModelicaResult` that resolves the UI's session.
/// Without this the UI keeps a "Compiling…" spinner running forever
/// after a rumoca panic.
fn command_session(cmd: &ModelicaCommand) -> (bevy::prelude::Entity, u64) {
    match cmd {
        ModelicaCommand::Step { entity, session_id, .. }
        | ModelicaCommand::Compile { entity, session_id, .. }
        | ModelicaCommand::UpdateParameters { entity, session_id, .. }
        | ModelicaCommand::Reset { entity, session_id, .. } => (*entity, *session_id),
        ModelicaCommand::Despawn { entity } => (*entity, 0),
        ModelicaCommand::LoadSourceRoot { .. } => (bevy::prelude::Entity::PLACEHOLDER, 0),
    }
}

fn synth_panic_result(entity: bevy::prelude::Entity, session_id: u64, msg: &str) -> ModelicaResult {
    ModelicaResult {
        entity,
        session_id,
        new_time: 0.0,
        outputs: Vec::new(),
        detected_symbols: Vec::new(),
        error: Some(format!("Worker panic: {msg}")),
        log_message: Some(format!("Worker panicked while processing command — recovered: {msg}")),
        is_new_model: false,
        is_parameter_update: false,
        is_reset: false,
        detected_input_names: Vec::new(),
        ..Default::default()
    }
}
use wasm_bindgen::closure::Closure;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{DedicatedWorkerGlobalScope, MessageEvent};

use lunco_modelica::worker::{process_inline_command, InlineWorkerInner};

thread_local! {
    /// Per-worker dispatch state. Outlives any single message because rumoca
    /// session caches and the lazy `ModelicaCompiler` are expensive to
    /// rebuild.
    static STATE: RefCell<InlineWorkerInner> = RefCell::new(InlineWorkerInner::default());

    /// Holds the `onmessage` closure for the lifetime of the worker; dropping
    /// it would un-register the JS-side handler.
    static ONMESSAGE_CB: RefCell<Option<Closure<dyn FnMut(MessageEvent)>>> = RefCell::new(None);
}

fn worker_global() -> DedicatedWorkerGlobalScope {
    js_sys::global()
        .dyn_into::<DedicatedWorkerGlobalScope>()
        .expect("running outside a DedicatedWorker context")
}

fn post_wire(scope: &DedicatedWorkerGlobalScope, msg: &WireResult) {
    let bytes = match bincode::serialize(msg) {
        Ok(b) => b,
        Err(e) => {
            web_sys::console::error_1(
                &format!("[lunica_worker] serialize wire failed: {e}").into(),
            );
            return;
        }
    };
    let array = Uint8Array::new_with_length(bytes.len() as u32);
    array.copy_from(&bytes);
    if let Err(e) = scope.post_message(&array) {
        web_sys::console::error_1(
            &format!("[lunica_worker] post_message failed: {e:?}").into(),
        );
    }
}

/// Ship the decompressed MSL bincode bytes to the main thread as a *transferred*
/// `ArrayBuffer` (zero-copy move, not a structured-clone copy). Posted as a bare
/// `ArrayBuffer` — the only non-`Uint8Array` message in the protocol — which the
/// main `onmessage` handler routes to `msl_remote::ingest_worker_decoded_msl`.
/// Sending the raw bytes (rather than a bincode `WireResult`) avoids re-encoding
/// ~165 MB and lets the transfer be zero-copy.
fn post_decoded_msl_transfer(scope: &DedicatedWorkerGlobalScope, bytes: Vec<u8>) {
    let array = Uint8Array::new_with_length(bytes.len() as u32);
    array.copy_from(&bytes);
    let buffer = array.buffer();
    let transfer = js_sys::Array::of1(&buffer);
    if let Err(e) = scope.post_message_with_transfer(&buffer, &transfer) {
        web_sys::console::error_1(
            &format!("[lunica_worker] post decoded MSL transfer failed: {e:?}").into(),
        );
    }
}

fn post_result(scope: &DedicatedWorkerGlobalScope, result: ModelicaResult) {
    post_wire(scope, &WireResult::Result(result));
}

fn post_log(scope: &DedicatedWorkerGlobalScope, line: impl Into<String>) {
    post_wire(scope, &WireResult::Log(line.into()));
}

fn post_run_update(
    scope: &DedicatedWorkerGlobalScope,
    run_id: lunco_experiments::ExperimentId,
    update: lunco_experiments::RunUpdate,
) {
    post_wire(scope, &WireResult::RunUpdate { run_id, update });
}

// Cancellation flag set per in-flight run. Worker is single-threaded
// (separate wasm instance, but no preemption inside it) so a plain
// thread_local is enough — the message loop checks the flag between
// solver phases.
thread_local! {
    static CANCEL_RUN_ID: RefCell<Option<lunco_experiments::ExperimentId>> = RefCell::new(None);
}

fn cancel_run_in_worker(run_id: lunco_experiments::ExperimentId) {
    CANCEL_RUN_ID.with(|c| *c.borrow_mut() = Some(run_id));
}

fn is_cancelled(run_id: lunco_experiments::ExperimentId) -> bool {
    CANCEL_RUN_ID.with(|c| c.borrow().map(|x| x == run_id).unwrap_or(false))
}

fn clear_cancel() {
    CANCEL_RUN_ID.with(|c| *c.borrow_mut() = None);
}

#[allow(clippy::too_many_arguments)]
fn run_fast_in_worker(
    scope: &DedicatedWorkerGlobalScope,
    run_id: lunco_experiments::ExperimentId,
    model_name: &str,
    source: &str,
    filename: &str,
    extras: &[(String, String)],
    overrides: &std::collections::BTreeMap<lunco_experiments::ParamPath, lunco_experiments::ParamValue>,
    inputs: &std::collections::BTreeMap<lunco_experiments::ParamPath, lunco_experiments::ParamValue>,
    bounds: &lunco_experiments::RunBounds,
) {
    use lunco_modelica::experiments_runner::apply_value_bindings_to_dae;
    let started = web_time::Instant::now();
    post_log(scope, format!("run_fast: start run={run_id:?} model={model_name}"));

    if is_cancelled(run_id) {
        clear_cancel();
        post_run_update(scope, run_id, lunco_experiments::RunUpdate::Cancelled);
        return;
    }

    // Compile the CLEAN model source (no inputs/overrides baked in) via the
    // worker's persistent ModelicaCompiler — identical to the interactive
    // Compile path and to native `run_inner`, so the source seated in the
    // shared session is always the same. Reuses the worker's compile cache.
    let compile = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        STATE.with(|s| {
            let mut state = s.try_borrow_mut().expect("worker state borrow");
            state
                .compiler()
                .compile_str_multi(model_name, source, filename, extras)
        })
    }));
    let dae = match compile {
        Ok(Ok(d)) => d,
        Ok(Err(e)) => {
            post_run_update(
                scope,
                run_id,
                lunco_experiments::RunUpdate::Failed {
                    error: format!("compile: {e}"),
                    partial: None,
                },
            );
            return;
        }
        Err(payload) => {
            let msg = payload
                .downcast_ref::<&'static str>()
                .copied()
                .or_else(|| payload.downcast_ref::<String>().map(|s| s.as_str()))
                .unwrap_or("(unknown panic)");
            post_run_update(
                scope,
                run_id,
                lunco_experiments::RunUpdate::Failed {
                    error: format!("compile panic: {msg}"),
                    partial: None,
                },
            );
            return;
        }
    };

    if is_cancelled(run_id) {
        clear_cancel();
        post_run_update(scope, run_id, lunco_experiments::RunUpdate::Cancelled);
        return;
    }

    // Inject run values (experiment inputs + parameter overrides) at the DAE
    // level — the SINGLE injection path, identical to native. No source
    // rewriting, so the run's DAE derives from exactly the compiled source.
    // `dae.dae` is an `Arc<Dae>` (shared compile cache); clone the inner DAE
    // before mutating, exactly as native `run_inner` does.
    let mut bindings = inputs.clone();
    bindings.extend(overrides.clone());
    let run_dae = if bindings.is_empty() {
        dae.dae.clone()
    } else {
        let mut d = (*dae.dae).clone();
        if let Err(reason) = apply_value_bindings_to_dae(&mut d, &bindings) {
            post_run_update(
                scope,
                run_id,
                lunco_experiments::RunUpdate::Failed {
                    error: format!("parameter/input binding failed: {reason}"),
                    partial: None,
                },
            );
            return;
        }
        std::sync::Arc::new(d)
    };

    // Drive the run through the SHARED `drive_run` — the EXACT same entry
    // point native (`experiments_runner::run_inner`) uses. It honours
    // `bounds.runtime`: Batch → the dense-output `simulate_with_diagnostics`
    // solve (robust on stiff models), Interactive → the streamable
    // `run_stepping_loop`. `WorkerSink` is the only worker-specific part
    // (postMessage + the cancel registry). Previously the worker open-coded a
    // stepper-only path, so stiff models (orbital-datacenter eclipse switch)
    // ran natively via Batch but failed in the browser with
    // `BDF step: step size too small at t=0`; routing through `drive_run`
    // closes that divergence and also brings the worker the batch
    // output-decimation.
    let mut sink = WorkerSink { scope, run_id };
    lunco_modelica::experiments_runner::drive_run(&run_dae, bounds, started, &mut sink);
    post_log(
        scope,
        format!("run_fast: done in {:.2}s", started.elapsed().as_secs_f64()),
    );

    // Report this worker's wasm linear-memory size so the main thread can
    // recycle it if it's bloated. wasm linear memory is GROW-ONLY — a heavy
    // run's footprint never shrinks back and accumulates across runs until the
    // next one OOM-traps (observed: runs 1–3 complete, 4–5 OOM). Discarding the
    // whole worker instance is the only way to reclaim it, so when we're over
    // the watermark we ask the main thread to respawn us (it does so now that
    // we're idle — this message is sent after the run's terminal update).
    let mem_mb = (core::arch::wasm32::memory_size(0) * 64 / 1024) as u32;
    const RECYCLE_WATERMARK_MB: u32 = 1024;
    if mem_mb > RECYCLE_WATERMARK_MB {
        post_wire(scope, &WireResult::RecycleRequest { mem_mb });
    }
}

/// Worker-side [`RunSink`](lunco_modelica::experiments_runner::RunSink):
/// streams run updates over `postMessage` and reads the worker's cancel
/// registry. The ONLY platform-specific half of the run loop — the loop
/// itself is shared with the native runner.
struct WorkerSink<'a> {
    scope: &'a DedicatedWorkerGlobalScope,
    run_id: lunco_experiments::ExperimentId,
}

impl lunco_modelica::experiments_runner::RunSink for WorkerSink<'_> {
    fn is_cancelled(&mut self) -> bool {
        if is_cancelled(self.run_id) {
            clear_cancel();
            true
        } else {
            false
        }
    }
    fn emit(&mut self, update: lunco_experiments::RunUpdate) {
        // DIAGNOSTIC: on each streamed Progress, log the worker's wasm linear
        // memory size. Native runs this exact solve flat at ~905 MB; this
        // surfaces whether the in-browser worker's memory climbs unbounded
        // (allocator fragmentation) toward the 4 GiB trap.
        if let lunco_experiments::RunUpdate::Progress { t_current, .. } = &update {
            let pages = core::arch::wasm32::memory_size(0);
            post_log(
                self.scope,
                format!(
                    "[mem] sim_t={:.0} wasm_linear={}MB",
                    t_current,
                    pages * 64 / 1024
                ),
            );
        }
        post_run_update(self.scope, self.run_id, update);
    }
    fn wall_budget(&self) -> Option<core::time::Duration> {
        // Generous backstop: a Fast Run that hasn't finished in this long is
        // almost certainly pathological (e.g. a stiff model over a huge
        // horizon at a too-tight tolerance). Fail it gracefully so the worker
        // is freed for the next run instead of grinding indefinitely. A
        // single runaway solver `step()` that blows the heap before this
        // fires is caught instead by worker-crash recovery on the main thread.
        Some(core::time::Duration::from_secs(120))
    }
}

#[wasm_bindgen(start)]
pub fn run() -> Result<(), JsValue> {
    console_error_panic_hook::set_once();
    web_sys::console::log_1(&"[lunica_worker] starting".into());

    let scope = worker_global();
    let scope_for_cb = scope.clone();

    // Announce the wire-protocol fingerprint BEFORE any bincode traffic. The
    // main thread compares it against its own `WIRE_BUILD_ID`; a mismatch means
    // this worker wasm is stale relative to the main bundle and is reported
    // loudly there. Plain string (not bincode) so the framing can't itself be a
    // victim of the layout drift it detects.
    let _ = scope.post_message(&JsValue::from_str(&format!(
        "{}{}",
        lunco_modelica::worker_transport::WIRE_HANDSHAKE_PREFIX,
        lunco_modelica::worker_transport::WIRE_BUILD_ID,
    )));

    let onmessage = Closure::wrap(Box::new(move |event: MessageEvent| {
        let bytes: Vec<u8> = match Uint8Array::new(&event.data()).to_vec() {
            v if !v.is_empty() => v,
            _ => return,
        };
        let envelope: WireMessage = match bincode::deserialize(&bytes) {
            Ok(c) => c,
            Err(e) => {
                web_sys::console::error_1(
                    &format!("[lunica_worker] decode message failed: {e}").into(),
                );
                return;
            }
        };

        match envelope {
            WireMessage::Command(cmd) => {
                let scope = scope_for_cb.clone();
                let label = command_label(&cmd);
                // Capture session BEFORE moving `cmd` into the
                // dispatch closure — needed for the panic-recovery
                // synthetic result so the UI's spinner clears.
                let (entity, session_id) = command_session(&cmd);
                let started = web_time::Instant::now();
                // `Step` fires at ~60 Hz once a model is running and
                // floods the console with `[worker] recv: Step …` /
                // `done: Step …` pairs that drown out everything
                // useful. Suppress recv/done log for Step but keep
                // panic logging on the error path so a step that
                // crashes still shows up.
                let is_hot_path = matches!(cmd, ModelicaCommand::Step { .. });
                if !is_hot_path {
                    post_log(&scope, format!("recv: {label}"));
                }
                // STATE is held across the whole dispatch. If a
                // panic unwinds *while* the RefCell mutable borrow is
                // active, the next message would hit `BorrowMutError`
                // and panic the worker. Drop the borrow before
                // `catch_unwind` returns by scoping it tightly.
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    STATE.with(|s| {
                        // `try_borrow_mut` so a poisoned borrow from
                        // a previous panic doesn't crash this one too.
                        match s.try_borrow_mut() {
                            Ok(mut state) => {
                                process_inline_command(&mut state, cmd, |result| {
                                    post_result(&scope, result);
                                });
                            }
                            Err(e) => {
                                post_log(
                                    &scope,
                                    format!("STATE borrow refused: {e} — resetting"),
                                );
                                // Replace the cell wholesale so the
                                // next command starts fresh. Loses
                                // cached compilers but avoids a
                                // wedge.
                                s.replace(InlineWorkerInner::default());
                            }
                        }
                    });
                }));
                match outcome {
                    Ok(()) => {
                        if !is_hot_path {
                            post_log(
                                &scope,
                                format!(
                                    "done: {label} in {:.2}s",
                                    started.elapsed().as_secs_f64()
                                ),
                            );
                        }
                    }
                    Err(e) => {
                        let msg = e
                            .downcast_ref::<&'static str>()
                            .copied()
                            .or_else(|| e.downcast_ref::<String>().map(|s| s.as_str()))
                            .unwrap_or("(unknown panic payload)");
                        post_log(
                            &scope,
                            format!(
                                "PANIC during {label} after {:.2}s: {msg}",
                                started.elapsed().as_secs_f64()
                            ),
                        );
                        // Synthesize an error result so the UI's
                        // session resolves. Without this the spinner
                        // stays in "Compiling…" forever after a
                        // rumoca panic (the Balloon example
                        // reproduces this).
                        post_result(&scope, synth_panic_result(entity, session_id, msg));
                        // Reset state — a panic mid-dispatch likely
                        // left the per-entity steppers / compiler
                        // in an inconsistent state. Better to lose
                        // caches than wedge every subsequent compile.
                        STATE.with(|s| {
                            s.replace(InlineWorkerInner::default());
                        });
                        post_log(&scope, "STATE reset after panic — caches cleared");
                    }
                }
            }
            WireMessage::InstallParsedMsl(parsed) => {
                let count = parsed.len();
                let started = web_time::Instant::now();
                lunco_modelica::msl_remote::install_global_parsed_msl_pub(parsed);
                post_wire(&scope_for_cb, &WireResult::MslReady { docs: count });
                post_log(
                    &scope_for_cb,
                    format!(
                        "installed MSL: {count} docs in {:.2}s",
                        started.elapsed().as_secs_f64()
                    ),
                );
            }
            WireMessage::InstallParsedMslCompressed { bytes, provide_to_main } => {
                // Decompress the bundle here in the worker — off the main thread.
                // We decompress ONCE to the raw bincode bytes, then deserialize
                // our own ASTs (for compiles). If we're the designated provider
                // (`provide_to_main`, set only for the primary worker), we also
                // transfer the same decoded bytes back to the main thread so it
                // skips the ruzstd decompress and only deserializes into its own
                // heap. Non-primary pool workers skip that transfer — the main
                // thread needs exactly one copy and would dedupe the rest.
                let started = web_time::Instant::now();
                match lunco_modelica::msl_remote::decompress_parsed_bundle(&bytes) {
                    Ok(decoded) => {
                        match lunco_modelica::msl_remote::deserialize_parsed_bundle(&decoded) {
                            Ok(parsed) => {
                                let count = parsed.len();
                                lunco_modelica::msl_remote::install_global_parsed_msl_pub(parsed);
                                // Ship the decoded bytes to main (transferred
                                // ArrayBuffer, zero-copy) BEFORE MslReady so the
                                // resolution/autocomplete heap fills as early as
                                // possible. `decoded` is moved out here.
                                if provide_to_main {
                                    post_decoded_msl_transfer(&scope_for_cb, decoded);
                                }
                                post_wire(&scope_for_cb, &WireResult::MslReady { docs: count });
                                post_log(
                                    &scope_for_cb,
                                    format!(
                                        "decoded compressed MSL: {count} docs in {:.2}s{}",
                                        started.elapsed().as_secs_f64(),
                                        if provide_to_main { " (provided to main)" } else { "" }
                                    ),
                                );
                            }
                            Err(e) => {
                                post_log(&scope_for_cb, format!("MSL deserialize failed: {e}"));
                            }
                        }
                    }
                    Err(e) => {
                        post_log(&scope_for_cb, format!("MSL decompress failed: {e}"));
                    }
                }
            }
            WireMessage::Ping(tag) => {
                post_log(
                    &scope_for_cb,
                    format!(
                        "pong: {tag} (msl={})",
                        lunco_modelica::msl_remote::global_parsed_msl()
                            .map(|m| m.len())
                            .unwrap_or(0)
                    ),
                );
            }
            WireMessage::RunFast {
                run_id,
                model_name,
                source,
                filename,
                extras,
                overrides,
                inputs,
                bounds,
            } => {
                let scope = scope_for_cb.clone();
                run_fast_in_worker(
                    &scope,
                    run_id,
                    &model_name,
                    &source,
                    &filename,
                    &extras,
                    &overrides,
                    &inputs,
                    &bounds,
                );
            }
            WireMessage::CancelRun { run_id } => {
                cancel_run_in_worker(run_id);
                post_log(&scope_for_cb, format!("cancel requested for run {run_id:?}"));
            }
            WireMessage::ParseDocument { doc_id, gen, uri, source } => {
                let started = web_time::Instant::now();
                // Lenient parser: always returns a usable
                // `StoredDefinition` plus a list of recovery errors.
                // Replaces the previous `parse_source_to_ast` (strict)
                // call so the receiver gets both the AST and the
                // diagnostics in one round-trip — matching the single
                // `SyntaxCache` shape the doc now uses.
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let recovery = rumoca_phase_parse::parse_to_syntax(&source, &uri);
                    // Resolve byte spans → located diagnostics here, where
                    // the source is in hand, so the main thread receives
                    // clickable parse errors (not just debug strings).
                    let errors: Vec<lunco_modelica::document::ParseDiag> = recovery
                        .parse_errors()
                        .iter()
                        .map(|e| lunco_modelica::document::parse_diag_from_error(e, &source))
                        .collect();
                    let ast = recovery.best_effort().clone();
                    (ast, errors)
                }));
                let (ast, errors) = match outcome {
                    Ok(pair) => pair,
                    Err(e) => {
                        let msg = e
                            .downcast_ref::<&'static str>()
                            .copied()
                            .or_else(|| e.downcast_ref::<String>().map(|s| s.as_str()))
                            .unwrap_or("(unknown panic payload)");
                        post_log(
                            &scope_for_cb,
                            format!("PANIC during ParseDocument doc={doc_id:?}: {msg}"),
                        );
                        (
                            rumoca_compile::parsing::ast::StoredDefinition::default(),
                            vec![lunco_modelica::document::ParseDiag::message_only(format!(
                                "worker panic: {msg}"
                            ))],
                        )
                    }
                };
                let ms = started.elapsed().as_secs_f64() * 1000.0;
                post_log(
                    &scope_for_cb,
                    format!(
                        "parsed doc={doc_id:?} gen={gen} src={}B in {ms:.0}ms (errors={})",
                        source.len(),
                        errors.len(),
                    ),
                );
                post_wire(
                    &scope_for_cb,
                    &WireResult::ParseDocumentDone { doc_id, gen, ast, errors },
                );
            }
        }
    }) as Box<dyn FnMut(MessageEvent)>);

    scope.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
    ONMESSAGE_CB.with(|slot| {
        *slot.borrow_mut() = Some(onmessage);
    });

    // Echo a hello back to main so the page knows the worker
    // wasm finished init and onmessage is wired. Without this the only
    // way to know the worker came up was to send a ping; if anything
    // panicked during init the page just silently never got results.
    post_log(&scope, "ready (worker wasm init complete)");
    web_sys::console::log_1(&"[lunica_worker] ready".into());
    Ok(())
}
} // mod wasm
