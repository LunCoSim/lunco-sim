//! High-performance Modelica integration for Bevy.
//!
//! This crate provides a bridge between Bevy's ECS and Modelica simulation models.
//! It features:
//! - A background worker thread that owns non-Send `SimStepper` instances
//! - Command/response architecture with session ID fencing to prevent stale data
//! - Command squashing to handle rapid parameter changes without back-pressure
//! - DAE caching per entity for instant Reset and fast stepper rebuilds
//! - Real-time telemetry and plotting via egui
//!
//! ## Architecture
//!
//! The `ModelicaPlugin` spawns a background worker thread that owns all simulation
//! steppers and cached DAEs. The main Bevy thread sends `ModelicaCommand`s via a
//! crossbeam channel and receives `ModelicaResult`s back. Each entity with a
//! `ModelicaModel` component gets its own stepper instance, identified by a
//! `session_id` that increments on each recompile/reset to fence stale results.
//!
//! ## DAE Caching
//!
//! After a successful compilation, the `CompilationResult` (including the DAE) is
//! cached per entity. This enables:
//! - **Instant Reset**: Rebuilds the SimStepper from the cached DAE without recompilation
//! - **Fast Step auto-init**: If the stepper was lost, rebuilds from cached DAE instead of
//!   recompiling from the file on disk
//! - **Parameter updates**: After UpdateParameters, the modified source is written to the
//!   temp file and the new DAE replaces the old cache entry
//!
//! ## Worker Panic Recovery
//!
//! The worker wraps all simulation logic in `catch_unwind`. If a numerical instability
//! (e.g., mass=0.0 in SpringMass) causes a solver panic, the error is caught and
//! reported as "Solver Error" in the logs rather than crashing the application.

use bevy::prelude::*;
use rumoca_compile::{Session, SessionConfig};
use crossbeam_channel::unbounded;
use std::thread;
use lunco_assets::msl_dir;

/// Typed identity for a Modelica class across the workbench.
///
/// Replaces the legacy string ID schemes (`msl_path:`, `bundled://…#`,
/// raw file paths, `mem://`) with a single `ClassRef { library, path }`
/// value that flows through opening, drill-in, tab dedup, projection
/// target lookup, and documentation lookup. See module docs for the
/// migration map.
pub mod class_ref;

/// Unified read-side metadata for Modelica classes — folds the
/// pre-baked palette index ([`visual_diagram::MSLComponentDef`]) and
/// the live per-document [`index::ClassEntry`] into one
/// [`class_metadata::ClassMetadata`] shape so docs view, badges,
/// and inspector title all read through one path.
pub mod class_metadata;

/// AST-based extraction functions for Modelica source code.
///
/// Walks the full Modelica AST (via `rumoca_phase_parse`) to extract model names,
/// parameters, inputs, and other symbols.
pub mod ast_extract;

/// Structural AST mutation helpers. Each helper takes `&mut ClassDef` and
/// performs an in-place change matching one `ModelicaOp` variant. Source
/// regeneration via `to_modelica()` is a separate concern handled by
/// `op_to_patch`. See `docs/architecture/A1_RUMOCA_MUTATOR_AUDIT.md` for
/// the migration plan and `tests/ast_mut_set_parameter.rs` for the TDD
/// contract.
pub mod ast_mut;

/// `ModelicaDocument` — the Document System representation of a `.mo` file.
///
/// Introduced dormant (no panels use it yet). See the module-level docstring
/// for migration order.
pub mod document;

/// Shared parse + I/O cache for Modelica classes, built on
/// `lunco_cache`. Drill-in, AddComponent preload, and (later)
/// compile dep-walk all funnel through here so every class file is
/// read once, parsed once, and shared as `Arc<AstCache>` across tabs
/// and compile jobs.
pub mod class_cache;
pub mod library_fs;

/// Subset Modelica pretty-printer — emits source snippets for *new* AST
/// nodes (component declarations, connect equations, placement / line
/// annotations). Used by AST-level document ops that splice new text at
/// a span in the existing source. Not a full round-trip printer —
/// existing nodes keep their original source text.
pub mod pretty;

/// Modelica-to-diagram graph builder — converts AST into DiagramGraph.
pub mod diagram;

/// Typed extractors for graphical annotations (Placement, Icon, Diagram,
/// and the common `graphics={...}` primitives). Walks the raw
/// `Vec<Expression>` that rumoca preserves on each class/component and
/// produces structs ready for the canvas renderer.
pub mod annotations;

/// egui painter for the typed graphics produced by [`annotations`].
/// Renders `Rectangle`, `Line`, `Polygon`, and `Text` directly into a
/// destination screen rect, mapping Modelica diagram coordinates
/// (+Y up) to egui screen coordinates (+Y down).
// `icon_paint` lives under `ui/` — it's a UI/rendering concern, not
// a model-semantics one. Re-exported here so the previous flat path
// (`lunco_modelica::icon_paint::*`) keeps compiling for any external
// consumer that hardcoded it.
pub use ui::icon_paint;

/// Single 2×3 affine transform per node from Modelica icon-local
/// coords to canvas world coords. Replaces the scattered
/// position/extent_size/rotation/mirror fields with one matrix that
/// every consumer (port placement, edge stub direction, icon body
/// painting, AABB) shares.
pub mod icon_transform;

/// Visual diagram editor — drag-and-drop component composition.
pub mod visual_diagram;

/// Per-document UI projection — what panels read instead of the AST.
/// Skeleton; population happens in the upcoming AST-canonical refactor.
/// See `docs/architecture/REFACTOR_PLAN.md`.
pub mod index;

/// Per-Twin Modelica domain engine: long-lived `rumoca_compile::Session`
/// + per-doc URI mapping. Provides cross-file inheritance-merged queries.
pub mod engine;
pub mod engine_resource;

/// Modelica adapter to the canonical Twin journal in
/// `lunco-twin-journal`. Records each applied [`crate::document::ModelicaOp`] as a
/// summary entry alongside its inverse. See module docs for the
/// "summary, not full Serialize" rationale.
pub mod journal;

/// Minimal byte-range diff helper. Used by the code-editor commit path
/// to convert a debounced full-buffer snapshot into a single
/// `ModelicaOp::EditText` splice — finer undo granularity and
/// CRDT-friendly text edits.
pub mod text_diff;

/// Pre-warmer: walks each opened doc's AST collecting cross-package
/// type references, then primes the engine's icon cache via a single
/// off-thread task. Drill-in projection sees a populated cache.
pub mod icon_warmer;
pub mod source_roots;

/// Simple wrapper around rumoca-compile for compiling Modelica models.
///
/// MSL is preloaded into the session at construction time via
/// [`rumoca_compile::compile::Session::load_source_root_tolerant`].
/// After preload, compiling any MSL-based user model is a plain
/// strict-reachable-DAE call against a session that already has
/// every MSL class visible to rumoca's §5 scope walker.
///
/// Why preload instead of demand-load? Demand-load requires
/// rumoca to emit fully-qualified references in its unresolved-ref
/// diagnostics so an external source provider can act on them.
/// Upstream rumoca currently emits raw short forms (`SI.Time`,
/// `Continuous.Filter`, `Rotational.Interfaces.PartialTwoFlanges`)
/// without scope qualification — which means an external resolver
/// has no way to disambiguate. Preload sidesteps the issue: once
/// every MSL class is in the session, the scope walker never has
/// to ask outside.
///
/// Cost: first session construction blocks while the parsed-artifact
/// cache (bincode under `RUMOCA_CACHE_DIR`) is hit. With a warm
/// cache, MSL loads in ~2–5 s. Cold cache (first run after a rumoca
/// version bump) is proportional to parser throughput; `msl_indexer`
/// can pre-warm offline.
pub struct ModelicaCompiler {
    session: Session,
}

impl ModelicaCompiler {
    /// Construct a compiler and preload MSL.
    ///
    /// MSL discovery order:
    ///
    /// 1. The process-wide source from [`lunco_assets::msl::global_msl_source`]
    ///    if it's been installed. This is how the wasm runtime feeds the
    ///    fetched-from-server MSL bundle in.
    /// 2. Fall back to [`lunco_assets::msl_source_root_path`] (filesystem).
    ///
    /// If both are absent — typical for the first wasm tick before the
    /// MSL bundle has finished downloading — the session is left empty
    /// and the next compile will surface `unresolved type reference`
    /// diagnostics until MSL lands. Callers that want to gate compiles
    /// on MSL readiness should consult `MslLoadState`.
    pub fn new() -> Self {
        let t_total = web_time::Instant::now();
        let mut session = Session::new(SessionConfig::default());
        if Self::preload_from_global(&mut session, t_total) {
            return Self { session };
        }
        // MSL preload is opt-in via `LUNCO_MODELICA_PRELOAD_MSL=1`.
        // Default-skip because the rumoca bincode cache invalidates on
        // every parse-schema bump, making cold preload cost minutes
        // for callers (sandbox balloon, standalone models) that import
        // nothing from MSL anyway. Models that *do* reference MSL
        // currently require the env until rumoca grows a lazy
        // resolve-on-first-symbol hook.
        let want_msl = std::env::var("LUNCO_MODELICA_PRELOAD_MSL")
            .map(|v| v != "0" && !v.is_empty())
            .unwrap_or(false);
        if want_msl {
            if let Some(msl_root) = lunco_assets::msl_source_root_path() {
                // Durable-external — MSL rarely changes and is
                // library-grade; rumoca uses this classification to
                // enable bincode persistence for parsed artifacts.
                let report = session.load_source_root_tolerant(
                    "msl",
                    rumoca_compile::compile::SourceRootKind::DurableExternal,
                    &msl_root,
                    None,
                );
                log::info!(
                    "[ModelicaCompiler] preloaded MSL from `{}` in {:.2}s: \
                     {} parsed / {} inserted (cache {:?}); diagnostics: {}",
                    msl_root.display(),
                    t_total.elapsed().as_secs_f64(),
                    report.parsed_file_count,
                    report.inserted_file_count,
                    report.cache_status,
                    if report.diagnostics.is_empty() {
                        "none".to_string()
                    } else {
                        format!("{} lines", report.diagnostics.len())
                    },
                );
            } else {
                log::info!(
                    "[ModelicaCompiler] no MSL source root available on this target; \
                     session starts empty",
                );
            }
        }
        Self { session }
    }

    /// If a process-wide MSL has been installed, preload it into the
    /// session. Returns `true` when handled (caller skips the disk
    /// fallback).
    ///
    /// Two web-side fast paths, in priority order:
    ///
    /// 1. **Pre-parsed bundle** ([`msl_remote::global_parsed_msl`]).
    ///    The chunked parse driver finished running, so we already have
    ///    `Vec<(uri, StoredDefinition)>` in hand — install via
    ///    `Session::replace_parsed_source_set` (no parsing). This is
    ///    the steady-state path on web.
    /// 2. **Source-only bundle** (`MslAssetSource::InMemory`). Bytes
    ///    are decompressed but the chunked parser hasn't finished yet.
    ///    Falling through to `load_source_root_in_memory` here would
    ///    block the main thread for ~60–120 s, so we skip preload and
    ///    let the caller see an empty session — the auto-retrigger on
    ///    parse-complete will rebuild the compiler properly.
    fn preload_from_global(session: &mut Session, t_total: web_time::Instant) -> bool {
        if let Some(parsed) = msl_remote::global_parsed_msl() {
            let docs = (**parsed).clone();
            let pair_count = docs.len();
            let inserted = session.replace_parsed_source_set(
                "msl",
                rumoca_compile::compile::SourceRootKind::DurableExternal,
                docs,
                None,
            );
            log::info!(
                "[ModelicaCompiler] installed pre-parsed MSL in {:.2}s: \
                 {} inserted (of {} docs)",
                t_total.elapsed().as_secs_f64(),
                inserted,
                pair_count,
            );
            return true;
        }
        if matches!(
            lunco_assets::msl::global_msl_source(),
            Some(lunco_assets::msl::MslAssetSource::InMemory(_))
        ) {
            log::info!(
                "[ModelicaCompiler] MSL bundle present but parse not yet complete — \
                 starting with empty session; compile will be retriggered when parse finishes"
            );
            return true;
        }
        if let Some(lunco_assets::msl::MslAssetSource::Filesystem(root)) =
            lunco_assets::msl::global_msl_source()
        {
            // Native fast-path: a single pre-parsed bundle on disk that
            // mirrors the wasm runtime's `parsed-*.bin.zst` strategy.
            // No per-file rumoca cache key — just bytes. Produced on
            // the first successful cold parse below; consumed by every
            // subsequent launch in ~1s.
            //
            // Bundle invalidation is implicit: if the rumoca version
            // changes its `StoredDefinition` layout, `bincode::deserialize`
            // fails and we fall through to the slow source-root parse,
            // which then rewrites the bundle.
            let bundle_path = lunco_assets::msl_dir().join("parsed-msl.bin");
            if let Ok(bytes) = std::fs::read(&bundle_path) {
                match bincode::deserialize::<
                    Vec<(String, rumoca_compile::parsing::StoredDefinition)>,
                >(&bytes)
                {
                    Ok(docs) => {
                        let pair_count = docs.len();
                        let inserted = session.replace_parsed_source_set(
                            "msl",
                            rumoca_compile::compile::SourceRootKind::DurableExternal,
                            docs,
                            None,
                        );
                        log::info!(
                            "[ModelicaCompiler] loaded pre-parsed MSL bundle from `{}` \
                             in {:.2}s: {} inserted (of {} docs)",
                            bundle_path.display(),
                            t_total.elapsed().as_secs_f64(),
                            inserted,
                            pair_count,
                        );
                        return true;
                    }
                    Err(e) => {
                        log::warn!(
                            "[ModelicaCompiler] parsed bundle decode failed ({e}) — \
                             falling back to source-root parse"
                        );
                    }
                }
            }

            // Slow path: parse the source root from disk. Goes through
            // rumoca's per-file artifact cache, but that cache is
            // fingerprint-keyed and easily invalidates — we treat this
            // as a one-time cost and write the bundle below so the
            // next launch hits the fast path above.
            let parsed = match rumoca_compile::source_roots::parse_source_root_with_cache_in(
                root,
                rumoca_compile::source_roots::resolve_source_root_cache_dir().as_deref(),
            ) {
                Ok(p) => p,
                Err(e) => {
                    log::warn!(
                        "[ModelicaCompiler] failed to parse MSL source root `{}`: {e}",
                        root.display()
                    );
                    return false;
                }
            };
            let parsed_count = parsed.file_count;
            // Serialise BEFORE moving documents into the session so we
            // don't pay a clone of ~30 MB of StoredDefinitions.
            match bincode::serialize(&parsed.documents) {
                Ok(bytes) => {
                    if let Some(parent) = bundle_path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    match std::fs::write(&bundle_path, &bytes) {
                        Ok(()) => log::info!(
                            "[ModelicaCompiler] wrote MSL bundle ({} MB) to `{}` — next launch will be ~1s",
                            bytes.len() / (1024 * 1024),
                            bundle_path.display()
                        ),
                        Err(e) => log::warn!(
                            "[ModelicaCompiler] failed to write MSL bundle to `{}`: {e}",
                            bundle_path.display()
                        ),
                    }
                }
                Err(e) => log::warn!(
                    "[ModelicaCompiler] failed to serialise MSL bundle: {e}"
                ),
            }
            let inserted = session.replace_parsed_source_set(
                "msl",
                rumoca_compile::compile::SourceRootKind::DurableExternal,
                parsed.documents,
                None,
            );
            log::info!(
                "[ModelicaCompiler] preloaded MSL from `{}` in {:.2}s: \
                 {} parsed / {} inserted (cache {:?})",
                root.display(),
                t_total.elapsed().as_secs_f64(),
                parsed_count,
                inserted,
                parsed.cache_status,
            );
            return true;
        }
        false
    }

    /// Compile Modelica source string and return DAE result.
    ///
    /// The user source is fed as a workspace document on top of the
    /// already-preloaded MSL. Rumoca's strict-reachable DAE walker
    /// sees the user's model plus the entire MSL class tree, so
    /// short-form refs like `SI.Time`, `Continuous.Filter`, etc.
    /// resolve through normal MLS §5 scope lookup.
    ///
    /// `filename` is used as the document URI for error reporting.
    pub fn compile_str(
        &mut self,
        model_name: &str,
        source: &str,
        filename: &str,
    ) -> Result<Box<rumoca_compile::compile::DaeCompilationResult>, String> {
        self.session.update_document(filename, source);
        self.compile_loaded(model_name)
    }

    /// Like `compile_str`, but seats additional `(filename, source)`
    /// pairs into the rumoca session before compiling so the resolver
    /// can satisfy cross-doc class references (e.g. a fresh untitled
    /// `RocketStage` referencing `AnnotatedRocketStage.Tank` from a
    /// sibling untitled doc that holds the package). Each extra is
    /// loaded via the same `update_document` path; rumoca dedups by
    /// filename so re-loading the same file is harmless.
    pub fn compile_str_multi(
        &mut self,
        model_name: &str,
        source: &str,
        filename: &str,
        extras: &[(String, String)],
    ) -> Result<Box<rumoca_compile::compile::DaeCompilationResult>, String> {
        for (extra_filename, extra_source) in extras {
            if extra_filename == filename {
                continue;
            }
            self.session.update_document(extra_filename, extra_source);
        }
        self.session.update_document(filename, source);
        self.compile_loaded(model_name)
    }

    /// Compile an MSL class that is already loaded into the session
    /// (no `update_document` call). Used by the `msl_indexer --warm`
    /// pass to populate rumoca's semantic-summary cache for common
    /// examples — the workbench's first compile of those classes is
    /// then a cache hit instead of paying the full multi-minute walk.
    pub fn compile_msl_class(
        &mut self,
        qualified: &str,
    ) -> Result<Box<rumoca_compile::compile::DaeCompilationResult>, String> {
        self.compile_loaded(qualified)
    }

    /// Inner helper: heartbeat + session.compile + final timing log.
    /// Both [`Self::compile_str`] (user-edited source) and
    /// [`Self::compile_msl_class`] (already-loaded MSL class) flow
    /// through here so the heartbeat behaviour is identical.
    fn compile_loaded(
        &mut self,
        model_name: &str,
    ) -> Result<Box<rumoca_compile::compile::DaeCompilationResult>, String> {
        let t_total = web_time::Instant::now();

        // Heartbeat: rumoca's compile pipeline is opaque from outside
        // and can take minutes on cold caches with MSL-heavy models
        // (parol Debug::fmt overhead — see ../rumoca/docs/design-notes/
        // perf-parol-trace-overhead.md). Without a periodic log line,
        // the user sees nothing for the entire duration and reasonably
        // assumes the worker hung. Spawn a tiny thread that emits an
        // INFO log every 5s while the synchronous compile is in
        // flight; signal it to stop on return.
        //
        // Wasm note: `std::thread::spawn` panics on wasm32-unknown-unknown
        // (single-threaded target). The compile already runs on the main
        // task there via the inline worker, so the user sees the freeze
        // anyway — heartbeat would just be cosmetic. Skip it.
        use std::sync::atomic::{AtomicBool, Ordering};
        let still_compiling = std::sync::Arc::new(AtomicBool::new(true));
        #[cfg(not(target_arch = "wasm32"))]
        {
            let stopper = std::sync::Arc::clone(&still_compiling);
            let model_for_thread = model_name.to_string();
            // Spawn detached — we deliberately do NOT join after compile
            // returns. The heartbeat sleeps in 5-second chunks; joining
            // would block the worker for up to a full tick (5 s) on EVERY
            // compile, even fast cache-hit ones. The workbench's
            // is_compiling flag would then stay set for that whole window,
            // and the Step dispatcher would idle visibly. Letting the
            // JoinHandle drop detaches the thread; it self-exits within
            // 5 s of `stopper=false` with at most one stray "still
            // compiling +N s" log line if the timing aligns badly.
            let _ = std::thread::spawn(move || {
                let started = web_time::Instant::now();
                let tick = std::time::Duration::from_secs(5);
                loop {
                    std::thread::sleep(tick);
                    if !stopper.load(Ordering::Relaxed) {
                        return;
                    }
                    log::info!(
                        "[ModelicaCompiler] still compiling `{}` (+{:.0}s)",
                        model_for_thread,
                        started.elapsed().as_secs_f64()
                    );
                }
            });
        }

        let result = self
            .session
            .compile_model_dae_strict_reachable_uncached_with_recovery(model_name);

        still_compiling.store(false, Ordering::Relaxed);
        // No `join` — see spawn comment above. The thread is detached
        // and will exit on its own within one tick.

        log::info!(
            "[ModelicaCompiler] compile `{}` finished in {:.2}s ({})",
            model_name,
            t_total.elapsed().as_secs_f64(),
            if result.is_ok() { "OK" } else { "ERR" },
        );
        result
    }

    /// Access the underlying `rumoca_compile::Session` — used by a
    /// test helper that needs to inspect loaded source roots.
    #[cfg(test)]
    pub fn session(&self) -> &Session {
        &self.session
    }

    /// Merge a Modelica source root into the live session so
    /// subsequent compiles can resolve its types. Used by the
    /// `LoadSourceRoot` worker command (`source_roots` lazy-load
    /// pipeline) — main thread sends this command before a Compile
    /// that depends on the library. Idempotent: rumoca dedups by
    /// `id`, so re-issuing for an already-loaded root is cheap.
    ///
    /// Blocks the worker thread for the duration of the parse:
    /// MSL warm-bundle ~1–3 s; cold parse 10–60 s. Other queued
    /// commands wait behind it.
    pub fn load_source_root(
        &mut self,
        id: &str,
        root_dir: &std::path::Path,
    ) -> rumoca_compile::compile::SourceRootLoadReport {
        // MSL fast path: a pre-parsed bundle (`parsed-msl.bin`,
        // ~316 MB) sits next to the MSL source tree, produced by
        // `msl_indexer`. Installing it via
        // `Session::replace_parsed_source_set` takes ~1–3 s vs the
        // 30+ s cold-parse path of `load_source_root_tolerant` over
        // 2847 .mo files. The same fast path already runs inside
        // [`ModelicaCompiler::new`]'s `preload_from_global`; we
        // duplicate it here so the lazy `LoadSourceRoot` worker
        // command also benefits when MSL is loaded on-demand
        // (i.e. after the compiler was created empty for non-MSL
        // models like Balloon).
        if id == "Modelica" {
            let bundle_path = lunco_assets::msl_dir().join("parsed-msl.bin");
            if let Ok(bytes) = std::fs::read(&bundle_path) {
                if let Ok(docs) = bincode::deserialize::<
                    Vec<(String, rumoca_compile::parsing::StoredDefinition)>,
                >(&bytes)
                {
                    let pair_count = docs.len();
                    let inserted = self.session.replace_parsed_source_set(
                        id,
                        rumoca_compile::compile::SourceRootKind::DurableExternal,
                        docs,
                        None,
                    );
                    log::info!(
                        "[ModelicaCompiler] installed pre-parsed MSL bundle \
                         ({} of {} docs)",
                        inserted, pair_count,
                    );
                    return rumoca_compile::compile::SourceRootLoadReport {
                        source_set_id: id.to_string(),
                        source_root_path: bundle_path.display().to_string(),
                        parsed_file_count: pair_count,
                        inserted_file_count: inserted,
                        cache_status: None,
                        cache_key: None,
                        cache_file: None,
                        diagnostics: Vec::new(),
                    };
                }
            }
        }
        self.session.load_source_root_tolerant(
            id,
            rumoca_compile::compile::SourceRootKind::DurableExternal,
            root_dir,
            None,
        )
    }

    /// Merge an in-memory source root (e.g. a bundled `.mo` file or
    /// a single workspace file) into the live session. Same
    /// idempotency + blocking semantics as
    /// [`Self::load_source_root`], but bytes are passed inline so
    /// callers without a real on-disk path can still install
    /// sources.
    ///
    /// `label` shows up in diagnostics as the "source root path"
    /// (rumoca convention: `"in-memory:<id>"`). `files` is a list
    /// of `(uri, source)` pairs; each `uri` is the filename rumoca
    /// will report errors against.
    pub fn load_source_root_in_memory(
        &mut self,
        id: &str,
        label: &str,
        files: Vec<(String, String)>,
    ) -> rumoca_compile::compile::SourceRootLoadReport {
        let file_count = files.len();
        let mut inserted = 0;
        for (uri, text) in &files {
            if self.session.add_document(uri, text).is_ok() {
                inserted += 1;
            }
        }
        rumoca_compile::compile::SourceRootLoadReport {
            source_set_id: id.to_string(),
            source_root_path: label.to_string(),
            parsed_file_count: file_count,
            inserted_file_count: inserted,
            cache_status: None,
            cache_key: None,
            cache_file: None,
            diagnostics: Vec::new(),
        }
    }
}


pub mod ui;

/// Bundled Modelica models for web deployment.
/// Available on all targets, but primarily used for wasm builds.
pub mod models;
pub mod msl_remote;
pub mod msl_settings;
pub mod indexer;
pub mod sim_stream;
pub mod worker;
pub mod experiments_runner;

/// Bevy resource wrapping the singleton [`experiments_runner::ModelicaRunner`].
/// Stored as `Arc` so UI panels can clone the handle and call
/// `run_fast` from event handlers without holding a `ResMut` borrow.
#[derive(Resource, Clone)]
pub struct ModelicaRunnerResource(pub std::sync::Arc<experiments_runner::ModelicaRunner>);
/// Wasm-only Web Worker transport — relays `ModelicaCommand` /
/// `ModelicaResult` between the main wasm instance and the off-thread
/// worker bundle so the UI never blocks on rumoca compile / step.
#[cfg(target_arch = "wasm32")]
pub mod worker_transport;
pub use worker::{
    ModelicaChannels, ModelicaCommand, ModelicaModel, ModelicaResult, handle_modelica_responses,
    spawn_modelica_requests,
};

#[cfg(feature = "lunco-api")]
pub mod api_queries;

// Always built — the UI (palette, inspector, canvas) dispatches these
// `ApplyModelicaOps` Reflect events directly. The module is named `api_*`
// because external HTTP callers also use it when `lunco-api` is enabled,
// but the events themselves carry no `lunco-api` dependency.
/// External JSON-RPC API handlers.
pub mod api;
pub use sim_stream::{new_sim_stream, SimSnapshot, SimStream, VarHistory, SimSample};

/// UI-thread registry of per-entity lock-free sim streams (Phase A
/// of the multi-sim architecture). On Compile the command observer
/// calls [`SimStreamRegistry::get_or_insert`] and ships a clone of
/// the returned `SimStream` to the worker thread; plots and
/// telemetry query the registry to get the same handle and render
/// without locking.
///
/// TODO(arch-phase-b): promote this into the full `SimRegistry`
///   keyed by `SimId` (not `Entity`) so non-Modelica backends can
///   publish snapshots through the same channel.
#[derive(Resource, Default)]
pub struct SimStreamRegistry {
    streams: std::collections::HashMap<Entity, SimStream>,
}

impl SimStreamRegistry {
    /// Existing stream for `entity`, or a freshly-created one. The
    /// returned handle is cheap to clone (Arc bump) and safe to
    /// share across threads.
    pub fn get_or_insert(&mut self, entity: Entity) -> SimStream {
        self.streams
            .entry(entity)
            .or_insert_with(sim_stream::new_sim_stream)
            .clone()
    }

    /// Returns the stream for `entity` if one has been registered.
    /// Readers never need mutable access — they just `load()` the
    /// `ArcSwap`.
    pub fn get(&self, entity: Entity) -> Option<&SimStream> {
        self.streams.get(&entity)
    }

    /// Drop the stream for `entity`. Called on despawn so stale
    /// snapshots don't pin memory.
    pub fn remove(&mut self, entity: Entity) {
        self.streams.remove(&entity);
    }
}

/// System sets for Modelica stepping in [`FixedUpdate`].
///
/// These sets let downstream code (e.g., balloon_setup) order its sync systems
/// relative to the Modelica worker communication.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelicaSet {
    /// Receive async results from the worker thread.
    HandleResponses,
    /// Send the next step command to the worker thread.
    SpawnRequests,
}

/// Bevy plugin for Modelica integration.
///
/// Sets up the background worker thread, channel resources, and response systems.
/// Modelica stepping runs in [`FixedUpdate`] so all co-simulation engines share
/// the same fixed timestep.
pub struct ModelicaPlugin;

/// Headless variant of [`ModelicaPlugin`] without UI panels.
///
/// Use in tests and non-windowed binaries. Starts the worker, inserts channels,
/// schedules stepping systems, but skips `ModelicaUiPlugin`.
pub struct ModelicaCorePlugin;

impl Plugin for ModelicaCorePlugin {
    fn build(&self, app: &mut App) {
        build_modelica_core(app);
    }
}

impl Plugin for ModelicaPlugin {
    fn build(&self, app: &mut App) {
        // The Modelica UI experience requires both core logic and plotting.
        // Enforce the dependency chain by adding them here.
        if !app.is_plugin_added::<ModelicaCorePlugin>() {
            app.add_plugins(ModelicaCorePlugin);
        }
        if !app.is_plugin_added::<lunco_viz::LuncoVizPlugin>() {
            app.add_plugins(lunco_viz::LuncoVizPlugin);
        }

        // PR-A: inventory every source root the workbench can load
        // into a rumoca compile session. No loads yet — the registry
        // just enumerates so PR-B's gate has something to look up.
        app.insert_resource(source_roots::SourceRootRegistry::build());
        app.add_plugins(ui::ModelicaUiPlugin);
        app.add_plugins(lunco_doc_bevy::ViewSyncPlugin);
        // Self-register with the workbench's plugin-driven document-
        // kind registry so File→New, the file picker, the library
        // browser, and `twin.toml` parsers all see Modelica without
        // any central edit. Init the resource defensively in case
        // the workbench plugin hasn't been added yet.
        app.init_resource::<lunco_twin::DocumentKindRegistry>();
        let mut registry = app
            .world_mut()
            .resource_mut::<lunco_twin::DocumentKindRegistry>();
        registry.register(
            lunco_twin::DocumentKindId::new("modelica"),
            lunco_twin::DocumentKindMeta {
                display_name: "Modelica Model".into(),
                extensions: vec!["mo"],
                can_create_new: true,
                default_filename: Some("NewModel.mo"),
                uri_scheme: Some("modelica"),
                manifest_section: Some("modelica"),
            },
        );
        // Install the user-facing indentation default for the
        // pretty-printer. The library-level default is two-space so
        // pure-Rust tests have predictable output; the workbench UI
        // wants tabs (matches Dymola / MSL hand-authored style).
        // Users can override at runtime via a settings panel or
        // script by calling `pretty::set_options` again.
        pretty::set_options(pretty::PrettyOptions::tabs());
    }
}

fn build_modelica_core(app: &mut App) {
    let (tx_cmd, rx_cmd) = unbounded();
    let (tx_res, rx_res) = unbounded();

    // Ensure MSL remote management is present (fetching, settings, status).
    // The domain is incomplete without MSL access.
    if !app.is_plugin_added::<msl_remote::MslRemotePlugin>() {
        app.add_plugins(msl_remote::MslRemotePlugin);
    }

    let msl = msl_dir();
    if msl.exists() {
        if let Ok(abs_path) = std::fs::canonicalize(&msl) {
            std::env::set_var("MODELICAPATH", abs_path.to_string_lossy().to_string());
        }
    }

    // Point rumoca at the workspace's shared `.cache/rumoca/`, the
    // same one `modelica_run` and `msl_indexer` use. Without this
    // alignment, the workbench reads XDG default (`~/.cache/rumoca`)
    // while the CLI tools warm `<workspace>/.cache/rumoca` —
    // `msl_indexer --warm` then does NOTHING for first workbench
    // compile, which stretches from ~12 s (warm) to 13+ minutes (cold,
    // observed). Honor an externally-set `RUMOCA_CACHE_DIR` if the
    // caller wants a sandboxed location (CI, tests).
    //
    // Historical note: earlier versions deliberately left this
    // unset to share with `modelica_tester` (which used XDG too).
    // The new `modelica_run` / `msl_indexer` CLI tools standardised
    // on workspace, and the workbench needs to follow suit so a
    // single `--warm` pass benefits every tool.
    #[cfg(not(target_arch = "wasm32"))]
    if std::env::var_os("RUMOCA_CACHE_DIR").is_none() {
        let target = lunco_assets::cache_dir().join("rumoca");
        std::env::set_var("RUMOCA_CACHE_DIR", &target);
        log::info!(
            "[ModelicaPlugin] using rumoca cache at {} (set RUMOCA_CACHE_DIR to override)",
            target.display(),
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        thread::spawn(move || {
            worker::modelica_worker(rx_cmd, tx_res);
        });
    }

    // On wasm we still hold the inline worker resource as a fallback for
    // pages that haven't loaded the off-thread worker bundle (e.g. local
    // dev where the worker JS file is missing). The Web Worker transport,
    // when wired up by the binary's startup code via
    // `worker_transport::install_worker`, takes precedence — it intercepts
    // commands via its own pump system and ships them to the worker
    // bundle, bypassing the inline path entirely.
    #[cfg(target_arch = "wasm32")]
    {
        app.insert_resource(worker::InlineWorker::default());
    }

    #[cfg(not(target_arch = "wasm32"))]
    app.insert_resource(ModelicaChannels { tx: tx_cmd, rx: rx_res });
    #[cfg(target_arch = "wasm32")]
    {
        // Hand the result-side sender to the worker_transport so the JS
        // `onmessage` callback can deliver decoded results into the same
        // channel the existing `handle_modelica_responses` system drains.
        // Cheap to clone; the original still goes into ModelicaChannels.
        let _ = worker_transport::register_result_sender(tx_res.clone());
        // Same trick on the command side so the JS test bridge can post
        // commands without going through the UI.
        let _ = worker_transport::register_command_sender(tx_cmd.clone());
        app.insert_resource(ModelicaChannels { tx: tx_cmd, rx: rx_res, rx_cmd, tx_res });
    }

    app.init_resource::<ui::WorkbenchState>();
    app.init_resource::<SimStreamRegistry>();

    // Experiments / Fast Run: backend-agnostic registry + this crate's
    // ModelicaRunner binding. UI for the Run buttons and Experiments
    // panel is layered in `ui::experiments_panel` (Step 5+).
    app.add_plugins(lunco_experiments::ExperimentsPlugin);
    app.insert_resource(ModelicaRunnerResource(
        std::sync::Arc::new(experiments_runner::ModelicaRunner::new()),
    ));
    app.init_resource::<experiments_runner::PendingHandles>();
    app.init_resource::<experiments_runner::ExperimentDrafts>();
    app.init_resource::<experiments_runner::ExperimentSources>();
    app.init_resource::<experiments_runner::PlaybackEntities>();
    app.add_systems(Update, experiments_runner::drain_pending_handles);

    app.configure_sets(
        FixedUpdate,
        (ModelicaSet::HandleResponses, ModelicaSet::SpawnRequests).chain(),
    );

    app.register_type::<ModelicaModel>()
        .add_systems(FixedUpdate, (
            handle_modelica_responses.in_set(ModelicaSet::HandleResponses),
            spawn_modelica_requests.in_set(ModelicaSet::SpawnRequests),
        ));

    // Global frame-time tracker. Logs every Update tick that exceeds
    // a threshold AND every tick within a 5-second window after the
    // last Modelica edit. The canvas-render-internal SLOW frame
    // instrumentation only caught time spent inside one panel; this
    // catches main-thread blocking in ANY system on the Bevy schedule
    // (other panels, observers, drive_*_cache, etc.).
    app.init_resource::<FrameTimeProbe>();
    app.add_systems(bevy::prelude::First, frame_time_probe_start);
    app.add_systems(bevy::prelude::PreUpdate, frame_time_probe_pre_update_end);
    app.add_systems(bevy::prelude::Update, frame_time_probe_update_end);
    app.add_systems(bevy::prelude::PostUpdate, frame_time_probe_post_update_end);
    app.add_systems(bevy::prelude::Last, frame_time_probe_end);

    #[cfg(target_arch = "wasm32")]
    {
        // Both systems run every Update; only one actually does work per
        // frame. `pump_commands_to_worker` early-returns if the JS worker
        // hasn't been installed yet — which keeps `inline_worker_process`
        // as the fallback dispatch until then. Once `install_worker` is
        // called by the binary, pump_commands wins because it drains
        // `rx_cmd` first; inline_worker_process then sees an empty queue
        // and no-ops.
        app.add_systems(Update, worker_transport::pump_commands_to_worker);
        app.add_systems(Update, worker::inline_worker_process);
        app.add_systems(Update, ui::update_file_load_result);
        // Drain Web-Worker RunUpdate streams into the runner's
        // RunHandle receivers and clear the runner's busy flag on
        // terminal updates. Cheap when no run is in flight.
        app.add_systems(Update, |_world: &mut World| {
            experiments_runner::pump_wasm_forwarders();
        });
    }
}

/// Global frame-time probe — start of frame.
fn frame_time_probe_start(mut probe: ResMut<FrameTimeProbe>) {
    let now = web_time::Instant::now();
    probe.frame_start = Some(now);
    probe.pre_update_start = Some(now);
}

fn frame_time_probe_pre_update_end(mut probe: ResMut<FrameTimeProbe>) {
    let now = web_time::Instant::now();
    probe.pre_update_ms = probe
        .pre_update_start
        .map(|t| now.duration_since(t).as_secs_f64() * 1000.0)
        .unwrap_or(0.0);
    probe.update_start = Some(now);
}

fn frame_time_probe_update_end(mut probe: ResMut<FrameTimeProbe>) {
    let now = web_time::Instant::now();
    probe.update_ms = probe
        .update_start
        .map(|t| now.duration_since(t).as_secs_f64() * 1000.0)
        .unwrap_or(0.0);
    probe.post_update_start = Some(now);
}

fn frame_time_probe_post_update_end(mut probe: ResMut<FrameTimeProbe>) {
    let now = web_time::Instant::now();
    probe.post_update_ms = probe
        .post_update_start
        .map(|t| now.duration_since(t).as_secs_f64() * 1000.0)
        .unwrap_or(0.0);
    probe.last_start = Some(now);
}

/// Global frame-time probe — end of frame. Logs the total Bevy
/// schedule time and (if applicable) flags whether we're inside the
/// post-edit window so per-edit hitches anywhere in the app surface.
fn frame_time_probe_end(mut probe: ResMut<FrameTimeProbe>) {
    let now = web_time::Instant::now();
    let last_ms = probe
        .last_start
        .map(|t| now.duration_since(t).as_secs_f64() * 1000.0)
        .unwrap_or(0.0);
    let Some(start) = probe.frame_start else { return };
    let dt_ms = now.duration_since(start).as_secs_f64() * 1000.0;
    let in_window = probe
        .last_edit
        .map(|t| t.elapsed().as_secs_f64() < 5.0)
        .unwrap_or(false);
    // Default fully off. egui idles at ~30 fps and even a long
    // compile / parse can run for seconds, so any unconditional
    // "hard hitch" threshold floods the console during normal use.
    // Enable explicitly with `LUNCO_FRAME_PROBE=1` (any non-empty
    // value) when investigating a specific freeze.
    let probe_enabled = std::env::var_os("LUNCO_FRAME_PROBE").is_some();
    if probe_enabled && (dt_ms > 30.0 || in_window) {
        bevy::log::info!(
            "[FrameTimeProbe] total={dt_ms:.0}ms pre={:.0} update={:.0} post={:.0} last={last_ms:.0}{}",
            probe.pre_update_ms,
            probe.update_ms,
            probe.post_update_ms,
            if in_window { " (post-edit window)" } else { "" }
        );
    }
    probe.frame_start = None;
}

/// Stamp the `last_edit` timestamp from anywhere that mutates a
/// document. The `apply_ops` site in `canvas_diagram` calls this so
/// the post-edit window covers any frame after a user edit.
pub fn frame_time_probe_stamp_edit(world: &mut World) {
    if let Some(mut probe) = world.get_resource_mut::<FrameTimeProbe>() {
        probe.last_edit = Some(web_time::Instant::now());
    }
}

#[derive(Resource, Default)]
pub struct FrameTimeProbe {
    frame_start: Option<web_time::Instant>,
    pre_update_start: Option<web_time::Instant>,
    update_start: Option<web_time::Instant>,
    post_update_start: Option<web_time::Instant>,
    last_start: Option<web_time::Instant>,
    pre_update_ms: f64,
    update_ms: f64,
    post_update_ms: f64,
    last_edit: Option<web_time::Instant>,
}


// ---------------------------------------------------------------------------
// Re-export AST extraction for public API compatibility
// ---------------------------------------------------------------------------
// These functions live in `ast_extract` but are re-exported here so external
// callers (workbench binaries, UI panels) can import from the crate root.
pub use ast_extract::{
    extract_model_name,
    extract_model_name_from_ast,
    extract_parameters,
    extract_parameters_from_ast,
    extract_inputs_with_defaults,
    extract_inputs_with_defaults_from_ast,
    hash_content,
};
// `strip_input_defaults` is already imported via `use self::ast_extract::strip_input_defaults`
// above and is available publicly through the `pub mod ast_extract` declaration.

// ---------------------------------------------------------------------------
// Re-export diagram types for public API
// ---------------------------------------------------------------------------
pub use diagram::{
    DiagramType,
    ModelicaComponentBuilder,
    list_class_names,
};

#[derive(Component, Reflect, Default)]
pub struct ModelicaInput { pub variable_name: String, pub value: f64 }

#[derive(Component, Reflect, Default)]
pub struct ModelicaOutput { pub variable_name: String, pub value: f64 }

#[cfg(test)]
mod observables_smoke {
    use super::*;
    use rumoca_sim::{SimStepper, StepperOptions};

    /// End-to-end smoke test for the observables pipeline: compile the
    /// bundled RocketEngine, run one step at full throttle, and assert
    /// every algebraic observable shows up with a physically-sensible
    /// value in [`collect_stepper_observables`]. Protects against
    /// (a) bumping rumoca to a version that drops `EliminationResult`
    ///     from the stepper again, and
    /// (b) reintroducing a Boolean intermediate in the bundled model
    ///     that rumoca's elimination pass can't reconstruct.
    // FIXME: `collect_stepper_observables` was removed during the
    // SnapshotStream refactor; this test still references it. Disable
    // until the stepper-observable wiring lands in its new home.
    #[cfg(any())]
    #[test]
    fn rocket_engine_observables_round_trip() {
        let raw = include_str!("../../../assets/models/RocketEngine.mo");
        let (src, _) = ast_extract::strip_input_defaults(raw);
        let mut c = ModelicaCompiler::new();
        let r = c.compile_str("RocketEngine", &src, "RocketEngine.mo")
            .expect("compile ok");
        let mut stepper = SimStepper::new(&r.dae, StepperOptions::default())
            .expect("stepper ok");
        stepper.set_input("throttle", 1.0).expect("throttle is an input");
        stepper.step(0.01).expect("step ok");

        let obs = collect_stepper_observables(&stepper);
        let by_name: std::collections::HashMap<_, _> =
            obs.into_iter().collect();

        for name in ["m_prop", "impulse", "m_dot", "thrust", "p_chamber", "isp"] {
            assert!(by_name.contains_key(name), "missing observable: {name}");
        }
        assert!(by_name["m_dot"] > 0.0,
            "m_dot should be nonzero at throttle=1, got {}", by_name["m_dot"]);
        assert!(by_name["thrust"] > 0.0,
            "thrust should be nonzero, got {}", by_name["thrust"]);
        assert!(by_name["p_chamber"] > 0.0,
            "p_chamber should be nonzero, got {}", by_name["p_chamber"]);
        assert!((by_name["isp"] - 2900.0 / 9.80665).abs() < 1e-3,
            "isp should equal v_e / g, got {}", by_name["isp"]);
    }

    /// Verifies that `"..."` description strings (MLS §A.2.5) survive
    /// into the per-doc [`crate::index::ModelicaIndex`] — that's what
    /// panels read for hover tooltips. If this regresses, Telemetry
    /// tooltips go dark.
    #[test]
    fn rocket_engine_descriptions_populate() {
        let raw = include_str!("../../../assets/models/RocketEngine.mo");
        let ast = rumoca_phase_parse::parse_to_ast(raw, "RocketEngine.mo")
            .expect("parses");
        let mut index = crate::index::ModelicaIndex::new();
        index.rebuild_from_ast(&ast, raw);
        for (var, needle) in [
            ("m_dot_max", "mass flow"),
            ("throttle",  "Throttle"),
            ("m_prop",    "Propellant"),
            ("thrust",    "Thrust"),
        ] {
            let entry = index
                .find_component_by_leaf(var)
                .unwrap_or_else(|| panic!("no component '{var}' in index"));
            assert!(
                entry.description.contains(needle),
                "'{var}' description should contain '{needle}', got: {:?}",
                entry.description
            );
        }
    }

    // ─────────────────────────────────────────────────────────
    // MSL demand-driven compile tests
    // ─────────────────────────────────────────────────────────
    //
    // Run with: `cargo test -p lunco-modelica msl --nocapture`
    //
    // `msl_` tests require the MSL tree at `<cache>/msl/Modelica/`
    // (populated by our indexer). They skip with a stderr notice if
    // absent — CI can run the non-MSL subset unconditionally.
    //
    // The headline test `msl_compile_with_limpid_is_fast_and_succeeds`
    // exercises the full iterative demand-load pipeline: alias
    // resolution, rumoca error → missing-class regex, fs::read,
    // update_document, retry loop. A known-good MSL example that
    // used to hang for minutes is the sanity check; we assert the
    // happy path + print elapsed so regression to "minutes" is
    // obvious in the log even if the timing isn't asserted strictly
    // (test runner load is variable).

    fn msl_available() -> bool {
        lunco_assets::msl_source_root_path().is_some()
    }


    /// Trivial smoke test — compile a self-contained model with no
    /// MSL references. Shouldn't touch the iterative loop at all,
    /// verifies the plain-compile path works post-refactor.
    #[test]
    fn bare_model_compiles_without_msl() {
        let src = r#"
            model Bare
              Real x(start=1);
            equation
              der(x) = -x;
            end Bare;
        "#;
        let mut c = ModelicaCompiler::new();
        let r = c.compile_str("Bare", src, "Bare.mo")
            .expect("bare model must compile without MSL");
        // Just assert we got a DAE at all — shape details vary
        // by rumoca version.
        let _ = r.dae;
    }

    /// Headline: end-to-end demand-driven compile that pulls MSL
    /// classes via the iterative loop. A minimal LimPID-using model
    /// forces the compiler to iteratively resolve Continuous.LimPID
    /// → Interfaces.SISO → SI types → Icons → etc.
    ///
    /// **Asserts**:
    /// - compile succeeds
    /// - logs total elapsed time (paste-able into regression tracking)
    /// - iteration count reasonable (< 20 for a small closure)
    ///
    /// Skips with a print if MSL isn't installed locally.
    /// Known-failing — not a resolver issue. Compiles through the
    /// resolve phase cleanly (all `SI.*`, `Logical.*` refs are
    /// resolved via the lazy hook). Fails at DAE (ToDae phase)
    /// with `unresolved reference: ModelicaServices.Machine.eps`.
    /// Rumoca hardcodes `ModelicaServices.Machine` + `Modelica.Constants`
    /// as CONSTANT_PACKAGES
    /// (rumoca-phase-flatten/src/lib.rs:687-689); its lookup in
    /// the resolved tree doesn't find `ModelicaServices.Machine`
    /// even after we `update_document(ModelicaServices/package.mo)`.
    /// Fetch trace confirms the file lands in the session but
    /// rumoca's constant-package resolver still errors.
    ///
    /// Note: `msl_compile_pid_controller_example_succeeds` passes
    /// and *also* instantiates LimPID transitively — so the gap
    /// is specific to this minimal direct-instantiation shape, not
    /// to LimPID itself. Filed as a rumoca-internal issue.
    #[test]
    #[ignore = "rumoca CONSTANT_PACKAGES lookup can't find ModelicaServices.Machine even after the file is loaded"]
    fn msl_compile_tiny_limpid_model_is_fast() {
        if !msl_available() {
            eprintln!("skipping msl_compile_tiny_limpid_model_is_fast: \
                       MSL not at {:?}",
                lunco_assets::msl_source_root_path());
            return;
        }
        // Tiny model that references one MSL block — drags in the
        // transitive closure via the iterative loader. Kept inline
        // so the test doesn't depend on a user file.
        let src = r#"
            model TestLimPID
              import Modelica.Units.SI;
              parameter SI.Time Ti = 0.1;
              Modelica.Blocks.Continuous.LimPID ctrl(
                k = 1.0,
                Ti = Ti,
                yMax = 10.0
              );
            equation
              ctrl.u_s = 1.0;
              ctrl.u_m = 0.0;
            end TestLimPID;
        "#;
        let mut c = ModelicaCompiler::new();
        let t0 = web_time::Instant::now();
        let result = c.compile_str("TestLimPID", src, "TestLimPID.mo");
        let elapsed = t0.elapsed();
        eprintln!(
            "msl_compile_tiny_limpid_model_is_fast: elapsed {:.2}s, \
             result = {}",
            elapsed.as_secs_f64(),
            if result.is_ok() { "OK".to_string() } else {
                format!("ERR: {}", result.as_ref().err().unwrap())
            }
        );
        result.expect("compile must succeed after iterative MSL load");
    }

    /// Same shape, against the actual PID_Controller example
    /// extracted from `Blocks/package.mo`. Bigger closure —
    /// Mechanics.Rotational + Blocks.Continuous + KinematicPTP +
    /// sensors + Icons.
    ///
    /// With the lazy ExternalResolver hook in place this should
    /// work without any alias-table workaround; rumoca's own §5
    /// resolver walks the `within Modelica;` + enclosing package
    /// imports and calls us for the bytes. Kept as a *diagnostic*
    /// test: if it fails, the failure is either (a) a genuine
    /// rumoca MLS gap (PID is NOT in rumoca's 180-supported MSL
    /// targets list — it may be one of the 15 known-failing), or
    /// (b) a resolver miss our hook should have handled.
    #[test]
    fn msl_compile_pid_controller_example_succeeds() {
        if !msl_available() {
            eprintln!("skipping: MSL not available");
            return;
        }
        // Reference by fully-qualified name so rumoca's scope-walker
        // sees the enclosing `package Blocks` (which carries
        // `import Modelica.Units.SI;`). The earlier version of this
        // test sliced PID_Controller out of `Blocks/package.mo` and
        // fed it as a standalone class, which dropped the enclosing
        // package's imports — failure was a test-construction flaw
        // on our side, not a resolver or rumoca gap.
        let src = r#"
            model TestPID
              extends Modelica.Blocks.Examples.PID_Controller;
            end TestPID;
        "#;
        let mut c = ModelicaCompiler::new();
        let t0 = web_time::Instant::now();
        let result = c.compile_str("TestPID", src, "TestPID.mo");
        let elapsed = t0.elapsed();
        eprintln!(
            "msl_compile_pid_controller_example_succeeds: elapsed {:.2}s, \
             result = {}",
            elapsed.as_secs_f64(),
            if result.is_ok() { "OK".to_string() } else {
                format!("ERR (first 500 chars): {}",
                    result.as_ref().err().unwrap().chars().take(500).collect::<String>())
            }
        );
        result.expect("PID_Controller must compile after iterative MSL load");
    }

    /// End-to-end test against an MSL target rumoca *officially*
    /// claims to support (from `msl_simulation_targets_180.json` in
    /// rumoca-test-msl). This is the real acceptance test for the
    /// lazy-resolver architecture: rumoca's §5 resolver walks the
    /// scope, our `MslLazyResolver` supplies bytes on demand, a
    /// tiny wrapper model instantiates a known-good MSL example by
    /// fully-qualified name. If this fails, the loader architecture
    /// is broken. If it passes but PID_Controller fails, the delta
    /// is a rumoca MLS gap — not our problem.
    #[test]
    fn msl_compile_known_good_rotational_example() {
        if !msl_available() {
            eprintln!("skipping: MSL not available");
            return;
        }
        // Minimal wrapper — forces the resolver to pull in
        // Rotational.Examples.First and its entire transitive
        // closure (Rotational.Components, Interfaces, SI types,
        // Icons, …). References by fully-qualified name, which is
        // the scope-friendly form rumoca resolves cleanly.
        let src = r#"
            model TestRotFirst
              extends Modelica.Mechanics.Rotational.Examples.First;
            end TestRotFirst;
        "#;
        let mut c = ModelicaCompiler::new();
        let t0 = web_time::Instant::now();
        let result = c.compile_str("TestRotFirst", src, "TestRotFirst.mo");
        let elapsed = t0.elapsed();
        eprintln!(
            "msl_compile_known_good_rotational_example: elapsed {:.2}s, \
             result = {}",
            elapsed.as_secs_f64(),
            if result.is_ok() { "OK".to_string() } else {
                format!("ERR (first 800 chars): {}",
                    result.as_ref().err().unwrap().chars().take(800).collect::<String>())
            }
        );
        result.expect("Rotational.Examples.First (known-good MSL target) must compile");
    }

    /// Purely-qualified-name test. If this passes but
    /// `msl_compile_known_good_rotational_example` fails, the gap
    /// is unambiguously in rumoca's short-form scope walking
    /// (enclosing-package imports aren't reaching nested classes),
    /// not in our resolver.
    #[test]
    fn msl_fully_qualified_time_resolves() {
        if !msl_available() {
            eprintln!("skipping: MSL not available");
            return;
        }
        let src = r#"
            model TestFullyQualifiedSI
              parameter Modelica.Units.SI.Time Ti = 0.5;
              Real x(start=1);
            equation
              der(x) = -x / Ti;
            end TestFullyQualifiedSI;
        "#;
        let mut c = ModelicaCompiler::new();
        let t0 = web_time::Instant::now();
        let result = c.compile_str("TestFullyQualifiedSI", src, "Q.mo");
        let elapsed = t0.elapsed();
        eprintln!(
            "msl_fully_qualified_time_resolves: elapsed {:.2}s, result = {}",
            elapsed.as_secs_f64(),
            if result.is_ok() { "OK".into() } else {
                format!("ERR (first 800 chars): {}",
                    result.as_ref().err().unwrap().chars().take(800).collect::<String>())
            }
        );
        result.expect("fully-qualified SI.Time must compile");
    }
}




