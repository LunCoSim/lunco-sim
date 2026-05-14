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
    use lunco_modelica::experiments_runner::{apply_inputs_to_source, apply_overrides_to_source};
    let started = web_time::Instant::now();
    post_log(scope, format!("run_fast: start run={run_id:?} model={model_name}"));

    let after_inputs = match apply_inputs_to_source(source, inputs) {
        Ok(s) => s,
        Err(e) => {
            post_run_update(
                scope,
                run_id,
                lunco_experiments::RunUpdate::Failed {
                    error: format!("input substitution failed: {e}"),
                    partial: None,
                },
            );
            return;
        }
    };
    let injected = match apply_overrides_to_source(&after_inputs, overrides) {
        Ok(s) => s,
        Err(e) => {
            post_run_update(
                scope,
                run_id,
                lunco_experiments::RunUpdate::Failed {
                    error: format!("override failed: {e}"),
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

    // Compile via the worker's persistent ModelicaCompiler. Reuses
    // the compile cache the worker already maintains for normal
    // Compile commands.
    let compile = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        STATE.with(|s| {
            let mut state = s.try_borrow_mut().expect("worker state borrow");
            state
                .compiler()
                .compile_str_multi(model_name, &injected, filename, extras)
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

    let opts = rumoca_sim::SimOptions {
        t_start: bounds.t_start,
        t_end: bounds.t_end,
        rtol: bounds.tolerance.unwrap_or(1e-6),
        atol: bounds.tolerance.unwrap_or(1e-6),
        dt: bounds.dt,
        scalarize: true,
        max_wall_seconds: None,
        solver_mode: rumoca_sim::SimSolverMode::Auto,
    };

    // Batch simulate. v1: no intra-step progress on wasm either —
    // diffsol's batch path doesn't expose a hook. TODO(progress):
    // switch to `build_simulation` + step loop with progress + cancel
    // poll between steps.
    let sim = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rumoca_sim::simulate_dae(&dae.dae, &opts)
    }));
    if is_cancelled(run_id) {
        clear_cancel();
        post_run_update(scope, run_id, lunco_experiments::RunUpdate::Cancelled);
        return;
    }
    match sim {
        Ok(Ok(r)) => {
            let result = sim_to_run_result(r, started.elapsed().as_millis() as u64);
            post_run_update(scope, run_id, lunco_experiments::RunUpdate::Completed(result));
            post_log(
                scope,
                format!("run_fast: done in {:.2}s", started.elapsed().as_secs_f64()),
            );
        }
        Ok(Err(e)) => {
            post_run_update(
                scope,
                run_id,
                lunco_experiments::RunUpdate::Failed {
                    error: format!("simulate: {e:?}"),
                    partial: None,
                },
            );
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
                    error: format!("simulate panic: {msg}"),
                    partial: None,
                },
            );
        }
    }
}

fn sim_to_run_result(r: rumoca_sim::SimResult, wall_time_ms: u64) -> lunco_experiments::RunResult {
    let mut series: std::collections::BTreeMap<String, Vec<f64>> = std::collections::BTreeMap::new();
    for (i, name) in r.names.iter().enumerate() {
        if let Some(col) = r.data.get(i) {
            series.insert(name.clone(), col.clone());
        }
    }
    let sample_count = r.times.len();
    lunco_experiments::RunResult {
        times: r.times,
        series,
        meta: lunco_experiments::RunMeta {
            wall_time_ms,
            sample_count,
            notes: None,
        },
    }
}

#[wasm_bindgen(start)]
pub fn run() -> Result<(), JsValue> {
    console_error_panic_hook::set_once();
    web_sys::console::log_1(&"[lunica_worker] starting".into());

    let scope = worker_global();
    let scope_for_cb = scope.clone();

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
                post_log(
                    &scope_for_cb,
                    format!(
                        "installed MSL: {count} docs in {:.2}s",
                        started.elapsed().as_secs_f64()
                    ),
                );
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
                    let errors: Vec<String> = recovery
                        .parse_errors()
                        .iter()
                        .map(|e| format!("{e:?}"))
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
                            rumoca_session::parsing::ast::StoredDefinition::default(),
                            vec![format!("worker panic: {msg}")],
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
