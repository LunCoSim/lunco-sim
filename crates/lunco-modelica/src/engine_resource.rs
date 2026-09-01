//! Long-lived [`ModelicaEngine`] exposed as a Bevy resource and
//! kept in lockstep with [`crate::state::ModelicaDocumentRegistry`].
//!
//! ## Why a long-lived engine
//!
//! The engine wraps a `rumoca_compile::Session` whose phase caches
//! (parse, resolve, instantiate, typecheck, flatten, DAE) amortise
//! across every cross-file query the workbench makes — completion,
//! inheritance walks, icon merging, compile, future hover-info.
//! Building a fresh engine per call (the previous shape in
//! `api_queries.rs`) re-uploads every open document on each request;
//! with this handle that work runs once at edit-time and every reader
//! sees the same warm session.
//!
//! ## Concurrency contract
//!
//! - One `Mutex<ModelicaEngine>` per workbench (per-Twin scope today;
//!   becomes per-Twin entry of a map when multi-Twin lands).
//! - Lock calls must be **short**. Snapshot what you need into owned
//!   values and release. A panel that holds the lock across a render
//!   would block API observers, the sync system, and other panels.
//! - Async tasks that need to query the engine can clone the
//!   [`ModelicaEngineHandle`] (it's `Arc`-internal) into the task and
//!   lock there. Bundled and user documents share this source-aware session
//!   and lifecycle.
//!
//! ## Sync semantics
//!
//! [`drive_engine_sync`] runs every `Update` tick. For each document
//! in the registry it compares the document's generation against the
//! per-doc cursor in [`EngineSyncCursor`]; on a delta it re-upserts
//! the document's current source via
//! [`crate::engine::ModelicaEngine::upsert_document`] (which feeds rumoca's
//! content-hash artifact cache, so unchanged source between two
//! generations is a hashmap hit). Removed documents are flushed via
//! [`ModelicaEngine::close_document`].
//!
//! Lazy-on-edit means a render-frame after the user types lands in
//! the engine on the next system tick — same staleness contract as
//! the per-doc Index.

use bevy::prelude::*;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use crate::engine::ModelicaEngine;
use lunco_doc::{Document, DocumentId};

/// Process-wide accessor for the workbench's engine handle. Set
/// once during plugin init and read from non-Bevy contexts (static
/// helpers in `class_cache`, async tasks, etc.) that can't take a
/// `Res<ModelicaEngineHandle>` parameter.
///
/// Returns `None` before `ModelicaEnginePlugin::build` has run —
/// callers should treat that as "no engine yet" (same as MSL bundle
/// loading: a query before boot returns empty).
static GLOBAL_ENGINE: OnceLock<ModelicaEngineHandle> = OnceLock::new();

pub fn global_engine_handle() -> Option<&'static ModelicaEngineHandle> {
    GLOBAL_ENGINE.get()
}

/// Process-wide handle to the workbench's [`ModelicaEngine`].
///
/// `Clone` is cheap (Arc bump) so callers needing to hand a handle
/// to an async task can do so without holding a Bevy resource borrow.
#[derive(Resource, Clone)]
pub struct ModelicaEngineHandle(Arc<Mutex<ModelicaEngine>>);

impl Default for ModelicaEngineHandle {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(ModelicaEngine::new())))
    }
}

impl ModelicaEngineHandle {
    /// Lock the engine for a query. Panics if the mutex is poisoned
    /// (would mean a previous panic happened while holding the lock —
    /// the engine state is then suspect anyway).
    pub fn lock(&self) -> MutexGuard<'_, ModelicaEngine> {
        self.0.lock().expect("modelica engine mutex poisoned")
    }

    /// Attempt a short engine read without waiting.
    ///
    /// Update/UI systems use this at contention boundaries so a background
    /// Modelica query can never turn into a window stall. A `None` result is a
    /// normal retry signal for the next update; it is not a fabricated model
    /// result.
    pub fn try_lock(&self) -> Option<MutexGuard<'_, ModelicaEngine>> {
        self.0.try_lock().ok()
    }

    /// Nonblocking UI read of a completed merged icon.
    ///
    /// A canvas paint must not wait for the shared engine mutex: a background
    /// Modelica inheritance query may legitimately hold it while resolving a
    /// standard-library class. Cache misses are therefore left to the
    /// background projection task, and the caller can use a local AST icon
    /// until that task requests a repaint.
    pub fn try_cached_icon_for(&self, qualified: &str) -> Option<crate::annotations::Icon> {
        let mut engine = self.0.try_lock().ok()?;
        engine.cached_icon_for(qualified).flatten()
    }

    /// Make a bundled source root available to the shared engine. This is a
    /// generic source-loading seam used by generated documents before the
    /// canvas asks the resolver for class icons, ports, or inherited members.
    pub fn ensure_library_root(&self, root: &str) -> bool {
        self.lock().ensure_library_root(root)
    }

    /// Schedule loading of a bundled source root without blocking the caller.
    ///
    /// The reservation is deliberately made inside the worker, rather than
    /// before spawning it. A projection, parse install, or MSL bootstrap may
    /// be using the engine when a generated document first requests its
    /// library; waiting for that mutex on the update thread was the second
    /// source of apparent "projection stalls" after the recursive projection
    /// lock was fixed. Duplicate requests are harmless: the first worker
    /// reserves the root and the others observe the pending/resident state.
    ///
    /// Returns `true` when a worker was scheduled. Completion is published by
    /// `ModelicaLibraryBecameReady`; the return value is not a readiness claim.
    pub fn ensure_library_root_async(&self, root: &str) -> bool {
        let root = root.to_string();
        let handle = self.clone();
        bevy::tasks::AsyncComputeTaskPool::get()
            .spawn(async move {
                {
                    let mut engine = handle.lock();
                    if !engine.begin_library_root_load(&root) {
                        return;
                    }
                }
                bevy::log::info!("[ModelicaLibrary] loading source root `{root}` asynchronously");
                let files = lunco_assets::models::package_files_live(&root);
                let mut parsed = Vec::with_capacity(files.len());
                let mut diagnostics = Vec::new();
                for (uri, source) in files {
                    match rumoca_phase_parse::parse_to_ast(&source, &uri) {
                        Ok(ast) => parsed.push((uri, ast)),
                        Err(error) => diagnostics.push(format!("{uri}: {error:?}")),
                    }
                }
                let mut engine = handle.lock();
                let count = engine.finish_library_root_load(&root, parsed, diagnostics);
                bevy::log::info!(
                    "[ModelicaLibrary] source root `{root}` ready: {count} parsed document(s)",
                );
            })
            .detach();
        true
    }

    /// Spawn an off-thread strict parse for `doc_id`'s `source` and
    /// install the resulting AST into the session when it completes.
    ///
    /// Returns immediately; the lock is held only briefly to mark
    /// `doc_id` as pending. The parse itself runs OUTSIDE the lock,
    /// then a brief lock at the end installs the AST and queues a
    /// completion via [`ModelicaEngine::finish_parse`].
    ///
    /// `gen` is the doc's generation at spawn time — readers that
    /// drain completions can compare it against the doc's current
    /// generation and discard stale results.
    ///
    /// `spawn_fn` is the platform task spawner: native callers pass
    /// `|task| AsyncComputeTaskPool::get().spawn(async move { task() }).detach()`.
    /// WASM can pass an equivalent. Decoupling the spawner keeps this
    /// crate Bevy-agnostic at the engine layer.
    ///
    /// No-op if a parse for `doc_id` is already in flight (dedupe).
    /// Mark a doc as pending-parse and return its URI without spawning
    /// any parser. Used by the wasm path that ships parsing to the
    /// Web Worker — see `worker_transport::dispatch_parse_to_worker`
    /// and `drain_worker_parse_results` (engine_resource).
    ///
    /// Returns `None` when another parse is already in flight for the
    /// same doc (dedup, same as `upsert_document_async`).
    pub fn mark_pending_for_worker(&self, doc_id: DocumentId) -> Option<String> {
        let mut engine = self.lock();
        if !engine.mark_pending(doc_id) {
            return None;
        }
        Some(engine.uri_for(doc_id))
    }

    /// Install a worker-parsed AST and clear the pending slot in one
    /// atomic step. Counterpart to `mark_pending_for_worker`.
    pub fn install_worker_parsed_ast(
        &self,
        doc_id: DocumentId,
        gen: u64,
        ast: rumoca_compile::parsing::ast::StoredDefinition,
    ) {
        let mut engine = self.lock();
        engine.install_parsed_ast(doc_id, ast);
        engine.finish_parse(doc_id, gen);
    }

    /// Drop the pending slot without installing anything — used when
    /// the worker reports a parse failure so the dedup gate clears
    /// and the next gen can retry.
    pub fn finish_pending_failed(&self, doc_id: DocumentId, gen: u64) {
        let mut engine = self.lock();
        engine.finish_parse(doc_id, gen);
    }

    /// Record a terminal worker parse failure and clear the in-flight slot.
    /// The next engine-sync pass installs the diagnostic into the document's
    /// syntax cache, so the same source generation is not retried forever.
    pub fn finish_worker_parse_failed(&self, doc_id: DocumentId, gen: u64, error: String) {
        let mut engine = self.lock();
        engine.set_parse_diags(doc_id, vec![lunco_doc::Diagnostic::message_only(error)]);
        engine.finish_parse(doc_id, gen);
    }

    /// Clear all pending parses. Used when a worker crashes to unwedge the
    /// parse queue.
    pub fn clear_all_pending(&self) {
        let mut engine = self.lock();
        engine.clear_all_pending();
    }

    pub fn upsert_document_async<F>(
        &self,
        doc_id: DocumentId,
        gen: u64,
        source: std::sync::Arc<str>,
        spawn_fn: F,
    ) where
        F: FnOnce(Box<dyn FnOnce() + Send + 'static>),
    {
        // Reserve the in-flight slot. Bail if another parse is running
        // for this doc — the next sync tick will pick up newer source
        // when the current parse finishes.
        let uri = {
            let mut engine = self.lock();
            if !engine.mark_pending(doc_id) {
                return;
            }
            engine.uri_for(doc_id)
        };
        let me = ModelicaEngineHandle(Arc::clone(&self.0));
        let bytes = source.len();
        spawn_fn(Box::new(move || {
            let t_total = web_time::Instant::now();
            // Lenient parser: always produces a usable tree.
            let t_parse = web_time::Instant::now();
            let recovery = rumoca_phase_parse::parse_to_syntax(&source, &uri);
            let parse_ms = t_parse.elapsed().as_secs_f64() * 1000.0;
            let has_errors = recovery.has_errors();
            // Resolve the lenient parser's structured errors into located
            // diagnostics now, while we still hold the source. Stashed on
            // the engine for the drain to fold into the doc's SyntaxCache
            // — without this the native live-edit path lost them entirely.
            let diags: Vec<lunco_doc::Diagnostic> = recovery
                .parse_errors()
                .iter()
                .map(|e| crate::document::parse_diag_from_error(e, &source))
                .collect();
            let ast = recovery.best_effort().clone();
            let t_install = web_time::Instant::now();
            let mut engine = me.lock();
            engine.install_parsed_ast(doc_id, ast);
            engine.set_parse_diags(doc_id, diags);
            engine.finish_parse(doc_id, gen);
            let install_ms = t_install.elapsed().as_secs_f64() * 1000.0;
            bevy::log::info!(
                "[engine] async parse doc={} gen={} bytes={} parse={:.1}ms install={:.1}ms total={:.1}ms has_errors={}",
                doc_id.raw(),
                gen,
                bytes,
                parse_ms,
                install_ms,
                t_total.elapsed().as_secs_f64() * 1000.0,
                has_errors,
            );
        }));
    }
}

/// Per-document generation cursor used by [`drive_engine_sync`] to
/// decide which documents need re-upsert this tick. Internal to the
/// sync mechanism.
#[derive(Resource, Default)]
pub struct EngineSyncCursor {
    /// Document → last-seen generation. Absent entry means
    /// "never synced".
    last_synced: HashMap<DocumentId, u64>,
    /// Last registry revision whose document set was scanned. Async parse
    /// completions advance the registry revision through `mark_changed`, so
    /// stale completions also reopen the scan.
    registry_revision: u64,
}

/// Pacing controls for the async parse scheduler in [`drive_engine_sync`].
///
/// The resource is shared by GUI and headless execution so scene loading and
/// editing have the same bounded scheduling contract. UI code only refreshes
/// the input/focus hints; the queue and completion budgets remain owned by the
/// core scheduler. A bounded native queue matters because Modelica parsing
/// shares Bevy's compute pool with scene projection and other loading work.
#[derive(Resource)]
pub struct ParsePacing {
    /// True while the user is actively typing — defers the edit-reparse
    /// debounce so a keystroke burst settles before paying for a parse.
    pub input_active: bool,
    /// The focused document, prioritised first in the async parse queue so
    /// the tab the user is staring at reparses ahead of background tabs.
    pub active_document: Option<DocumentId>,
    /// Maximum number of documents parsed concurrently. This keeps the
    /// shared compute pool available to scene loading and rendering.
    pub max_in_flight: usize,
    /// Maximum wall-clock time spent collecting parse completions in one
    /// update. At least one completion is collected when work is available;
    /// this value bounds the normal completion burst without dropping work.
    pub completion_budget_ms: u64,
}

impl Default for ParsePacing {
    fn default() -> Self {
        Self {
            input_active: false,
            active_document: None,
            max_in_flight: 4,
            completion_budget_ms: 2,
        }
    }
}

/// Sync open Modelica documents into the engine session. Generation-
/// gated: docs whose generation hasn't advanced since the previous
/// sync are no-ops. Docs that have been removed from the registry
/// since last tick are dropped from the engine session via
/// [`ModelicaEngine::close_document`].
///
/// Runs every `Update`. Reads `ModelicaDocumentRegistry`, mutates the
/// engine and the cursor.
/// Edit-debounce window before re-parsing a document that was
/// previously parsed. New docs (never parsed) spawn immediately —
/// only the edit path is debounced. Mirrors the prior `ast_refresh`
/// gate now that `drive_engine_sync` is the single parse driver.
///
/// The window is *size-adaptive*: small docs reparse near-instantly
/// (rename feedback in tabs / browser / experiments lags otherwise),
/// large docs keep the long coalesce window so a rapid typing burst
/// doesn't queue redundant multi-second parses.
pub const AST_DEBOUNCE_MS: u128 = 2500;
pub const AST_DEBOUNCE_MS_SMALL: u128 = 200;
/// Source-size cut-off (bytes) below which the short debounce
/// applies. ~4 KB ≈ a single hand-edited model with a few connectors;
/// anything larger is plausibly a package / library file where the
/// parse itself is non-trivial.
pub const AST_DEBOUNCE_SIZE_THRESHOLD: usize = 4 * 1024;

#[inline]
pub fn ast_debounce_for_size(src_len: usize) -> u128 {
    if src_len <= AST_DEBOUNCE_SIZE_THRESHOLD {
        AST_DEBOUNCE_MS_SMALL
    } else {
        AST_DEBOUNCE_MS
    }
}

pub fn drive_engine_sync(
    mut commands: Commands,
    handle: Res<ModelicaEngineHandle>,
    mut registry: ResMut<crate::state::ModelicaDocumentRegistry>,
    mut cursor: ResMut<EngineSyncCursor>,
    pacing: Res<ParsePacing>,
) {
    let completed_roots = {
        let Some(mut engine) = handle.try_lock() else {
            return;
        };
        engine.drain_completed_library_roots()
    };
    for root in completed_roots {
        commands.trigger(ModelicaLibraryBecameReady { root });
    }
    // Active tab's doc id (if any). Used below to prioritise its
    // reparse over any background tabs queued behind it. `None` until
    // the UI feeds the hint (headless: always `None` → no reordering).
    let active_doc: Option<DocumentId> = pacing.active_document;
    // ── 1. Drain async-parse completions ──────────────────────────────
    // Pull a bounded completion batch from the workers. For each, fetch the
    // strict AST from the session and backfill the doc's local SyntaxCache +
    // AstCache so panels see the parsed state without needing a separate
    // `ast_refresh` pass. The wall-clock budget keeps a burst of background
    // parses from monopolising the frame that receives them.
    let completion_budget = web_time::Duration::from_millis(pacing.completion_budget_ms);
    let completion_started = web_time::Instant::now();
    let completed_results: Vec<_> = {
        let Some(mut engine) = handle.try_lock() else {
            return;
        };
        let mut results = Vec::new();
        loop {
            if !results.is_empty() && completion_started.elapsed() >= completion_budget {
                break;
            }
            let Some((doc_id, parse_gen)) = engine.drain_completed_up_to(1).into_iter().next()
            else {
                break;
            };
            results.push((
                doc_id,
                parse_gen,
                engine.parsed_for_doc(doc_id).cloned(),
                engine.take_parse_diags(doc_id),
            ));
        }
        results
    };
    for (doc_id, parse_gen, parsed_ast, parse_diags) in completed_results {
        // Snapshot current doc gen + URI under a brief engine lock —
        // we'll backfill if the doc still matches the gen this parse
        // ran against.
        let host_gen = registry
            .host(doc_id)
            .map(|h| h.document().generation())
            .unwrap_or(u64::MAX);
        if parse_gen != host_gen {
            // Doc moved on while parse was in flight; the next
            // sync tick will spawn a fresh parse for the new gen.
            bevy::log::info!(
                "[EngineSync] async parse stale (parse_gen={parse_gen} doc_gen={host_gen}) — discarded for doc={}",
                doc_id.raw(),
            );
            if registry.host(doc_id).is_some() {
                // The document still needs a fresh parse. Keep the normal
                // generation cursor unchanged and wake the registry-level
                // scan with the existing mutation signal.
                registry.mark_changed(doc_id);
            }
            continue;
        }
        match (parsed_ast, registry.host_mut(doc_id)) {
            (Some(ast), Some(host)) => {
                let arc_ast = std::sync::Arc::new(ast);
                let syntax = crate::document::SyntaxCache {
                    generation: parse_gen,
                    ast: arc_ast,
                    // Located parse diagnostics captured at spawn time.
                    errors: parse_diags,
                };
                host.document_mut().install_parse_results(syntax);
                bevy::log::info!(
                    "[EngineSync] async parse complete doc={} gen={} → backfilled doc.syntax",
                    doc_id.raw(),
                    parse_gen,
                );
            }
            (None, _) => {
                // Strict parse failed (recovered into session via
                // lenient parser recovery). Surface the failure into the
                // doc's parse cache so the diagnostics panel can
                // show a row.
                if let Some(host) = registry.host_mut(doc_id) {
                    // Prefer the located diagnostics; fall back to a
                    // generic note only when the parser gave us none.
                    let errors = if parse_diags.is_empty() {
                        vec![lunco_doc::Diagnostic::message_only(
                            "strict parse failed (lenient recovered)",
                        )]
                    } else {
                        parse_diags
                    };
                    let syntax = crate::document::SyntaxCache {
                        generation: parse_gen,
                        ast: std::sync::Arc::new(
                            rumoca_compile::parsing::ast::StoredDefinition::default(),
                        ),
                        errors,
                    };
                    host.document_mut().install_parse_results(syntax);
                }
                bevy::log::warn!(
                    "[EngineSync] async parse strict-failed doc={} gen={}",
                    doc_id.raw(),
                    parse_gen,
                );
            }
            (Some(_), None) => {
                // Doc was closed mid-parse; engine still got the AST.
            }
        }
        // install_parse_results rebuilds the index, which emits
        // structured `ClassAdded` / `ClassRemoved` / `ClassRenamed`
        // changes into the doc's change ring. Those need a
        // DocumentChanged trigger to wake the watermark observer —
        // otherwise the rename detection runs but never reaches the
        // tab / experiment / draft re-bind observers, and the
        // generation gets advanced past it on the next unrelated
        // edit. Mark the doc changed so the next drain fans the
        // notification out.
        registry.mark_changed(doc_id);
        let current = cursor.last_synced.get(&doc_id).copied().unwrap_or(0);
        if parse_gen > current {
            cursor.last_synced.insert(doc_id, parse_gen);
        }
    }

    // Completed work above can mutate the registry and is therefore visible
    // in this same tick. If neither the registry nor an async completion moved,
    // the per-document cursor is authoritative and a full host walk is pure
    // overhead on the render path.
    let registry_revision = registry.revision();
    if registry_revision == cursor.registry_revision {
        return;
    }

    // ── 2. Collect docs needing sync ──────────────────────────────────
    // For each doc whose generation has advanced past the cursor,
    // decide between sync fast-path (fresh strict AST already on doc)
    // and async path (no AST or stale).
    enum SyncPlan {
        Sync(std::sync::Arc<rumoca_compile::parsing::ast::StoredDefinition>),
        Async(std::sync::Arc<str>),
    }
    let mut to_upsert: Vec<(DocumentId, u64, SyncPlan)> = Vec::new();
    let mut alive: HashSet<DocumentId> = HashSet::new();
    for (doc_id, host) in registry.iter() {
        alive.insert(doc_id);
        let doc = host.document();
        let gen = doc.generation();
        let needs = match cursor.last_synced.get(&doc_id) {
            Some(prev) => *prev < gen,
            None => true,
        };
        if !needs {
            continue;
        }
        // Fresh strict AST = doc.syntax.generation matches doc.generation
        // and AstCache reports Ok. Otherwise the cached tree is stale
        // (post-edit pre-reparse) and re-using it would push stale
        // bytes into the engine session. Async path handles staleness
        // by re-parsing.
        let fresh_ast = if !doc.syntax_is_stale() && !doc.ast_is_stale() {
            doc.strict_ast()
        } else {
            None
        };
        let plan = match fresh_ast {
            Some(ast) => SyncPlan::Sync(ast),
            None => SyncPlan::Async(doc.source_arc()),
        };
        to_upsert.push((doc_id, gen, plan));
    }
    let removed: Vec<DocumentId> = cursor
        .last_synced
        .keys()
        .copied()
        .filter(|id| !alive.contains(id))
        .collect();

    if to_upsert.is_empty() && removed.is_empty() {
        cursor.registry_revision = registry_revision;
        return;
    }

    // ── 3. Apply sync upserts + spawn async parses ────────────────────
    let mut sync_only: Vec<(
        DocumentId,
        u64,
        std::sync::Arc<rumoca_compile::parsing::ast::StoredDefinition>,
    )> = Vec::new();
    let mut async_only: Vec<(DocumentId, u64, std::sync::Arc<str>)> = Vec::new();
    for (doc_id, gen, plan) in to_upsert {
        match plan {
            SyncPlan::Sync(ast) => sync_only.push((doc_id, gen, ast)),
            SyncPlan::Async(src) => async_only.push((doc_id, gen, src)),
        }
    }
    {
        let Some(mut engine) = handle.try_lock() else {
            return;
        };
        for (doc_id, gen, ast) in sync_only {
            engine.upsert_document_with_ast(doc_id, (*ast).clone());
            bevy::log::info!(
                "[EngineSync] upsert(parsed) doc={} gen={}",
                doc_id.raw(),
                gen,
            );
            cursor.last_synced.insert(doc_id, gen);
        }
        for doc_id in &removed {
            engine.close_document(*doc_id);
        }
    }
    for doc_id in removed {
        cursor.last_synced.remove(&doc_id);
    }
    // Dispatch parses outside the engine lock. Native parses use the
    // task pool; wasm parses are posted to the Web Worker and return
    // through the parse-done channel.
    //
    // Debounce gate (replaces the prior `ast_refresh` system):
    //   - First parse for a doc (syntax.generation == 0) fires
    //     immediately — open-flow, user is waiting.
    //   - Edit reparse (syntax.generation > 0 but stale) waits for
    //     `AST_DEBOUNCE_MS` of post-edit silence + no input activity.
    //     Lets a typing burst settle before paying for a parse.
    #[cfg(not(target_arch = "wasm32"))]
    let pool = bevy::tasks::AsyncComputeTaskPool::get();
    let now = web_time::Instant::now();
    // Active-doc-first ordering. The active tab's reparse takes
    // priority over background tabs because the user is staring at
    // its canvas; any other tab can wait.
    if let Some(active) = active_doc {
        async_only.sort_by_key(|(doc_id, _, _)| if *doc_id == active { 0 } else { 1 });
    }
    let max_in_flight = pacing.max_in_flight.max(1);

    for (doc_id, gen, source) in async_only {
        let pending_count = {
            let Some(eng) = handle.try_lock() else {
                return;
            };
            if eng.has_parse_work(doc_id) {
                continue;
            }
            eng.pending_count()
        };
        if pending_count >= max_in_flight {
            // Bail out of the *whole loop* — subsequent iterations
            // would only enqueue more work onto an already-saturated
            // wasm pool. The next tick will retry the docs we
            // skipped in the same priority order (active first).
            bevy::log::debug!(
                "[EngineSync] parse queue full ({pending_count} in flight) — \
                 deferring doc={} gen={} until next tick",
                doc_id.raw(),
                gen,
            );
            break;
        }
        // Look up the doc to decide first-parse-vs-edit-reparse.
        // Note: AST-canonical structured ops never reach this path
        // because they install a fresh AST inline (see
        // document::apply_patch), so `ast_is_stale` is false and the
        // earlier `fresh_ast` branch took the sync `upsert` route. Only
        // free-form text edits land here, and the debounce + activity
        // gates exist to coalesce keystroke bursts on those.
        let (was_parsed, last_edit) = match registry.host(doc_id) {
            Some(host) => {
                let doc = host.document();
                (doc.syntax_arc().generation > 0, doc.last_source_edit_at())
            }
            None => (false, None),
        };
        if was_parsed {
            let debounce_ms = ast_debounce_for_size(source.len());
            let elapsed_ok = match last_edit {
                Some(t) => now.duration_since(t).as_millis() >= debounce_ms,
                None => true,
            };
            if !elapsed_ok || pacing.input_active {
                continue;
            }
        }
        let src_len = source.len();
        // Wasm path: ship parsing to the off-thread Web Worker. The worker
        // bundle has its own wasm instance; parsing there leaves the UI
        // thread free to render. The result lands via the parse-done channel
        // that `drain_worker_parse_results` polls each tick.
        //
        // If the worker is still starting or cannot accept the request,
        // leave the document eligible for a retry on the next tick. There
        // is no wasm parser on the page's main thread.
        #[cfg(target_arch = "wasm32")]
        let dispatched_to_worker = match handle.mark_pending_for_worker(doc_id) {
            Some(uri) => {
                // MSL-bundle short-circuit: every file under
                // `Modelica/` (and the extra-library trees) was
                // pre-parsed at build time and bincode-shipped in
                // `parsed-<sha>.bin.zst`; on wasm those ASTs live in
                // `crate::msl_remote::global_parsed_msl()` keyed by
                // their original MSL-relative path
                // (`Modelica/Blocks/Sources.mo`, etc.). If the doc
                // we're about to parse came from one of those files,
                // grab the cached `StoredDefinition` and skip the
                // worker round-trip — re-parsing a 150 KB MSL file
                // takes minutes on wasm and produces a byte-identical
                // result.
                //
                // Identification: the doc's `DocumentOrigin::File`
                // path is the same key the MSL bundle uses.
                let cached_ast: Option<rumoca_compile::parsing::ast::StoredDefinition> = {
                    let host = registry.host(doc_id);
                    let origin_path =
                        host.map(|h| h.document().origin().clone())
                            .and_then(|o| match o {
                                lunco_doc::DocumentOrigin::File { path, .. } => Some(path),
                                _ => None,
                            });
                    origin_path.and_then(|path| {
                        let key = path.to_string_lossy().to_string();
                        crate::msl_remote::global_parsed_msl().and_then(|bundle| {
                            bundle
                                .iter()
                                .find(|(k, _)| k == &key)
                                .map(|(_, ast)| ast.clone())
                        })
                    })
                };
                if let Some(ast) = cached_ast {
                    let t0 = web_time::Instant::now();
                    handle.install_worker_parsed_ast(doc_id, gen, ast.clone());
                    let t_engine = t0.elapsed().as_secs_f64() * 1000.0;
                    let t1 = web_time::Instant::now();
                    if let Some(host) = registry.host_mut(doc_id) {
                        let syntax = crate::document::SyntaxCache {
                            generation: gen,
                            ast: std::sync::Arc::new(ast),
                            errors: Vec::new(),
                        };
                        host.document_mut().install_parse_results(syntax);
                    }
                    // See sibling site above: rebuild_index emits
                    // class-diff changes; wake the watermark observer.
                    registry.mark_changed(doc_id);
                    let t_doc = t1.elapsed().as_secs_f64() * 1000.0;
                    bevy::log::info!(
                        "[EngineSync] reuse pre-parsed MSL AST doc={} gen={} \
                         engine={:.0}ms doc={:.0}ms",
                        doc_id.raw(),
                        gen,
                        t_engine,
                        t_doc,
                    );
                    continue;
                }
                crate::worker_transport::dispatch_parse_to_worker(
                    doc_id,
                    gen,
                    uri,
                    source.to_string(),
                );
                true
            }
            None => {
                // Already in flight — let the next tick retry once
                // the current parse finishes.
                continue;
            }
        };
        #[cfg(not(target_arch = "wasm32"))]
        let dispatched_to_worker = false;

        #[cfg(target_arch = "wasm32")]
        if !dispatched_to_worker {
            bevy::log::debug!(
                "[EngineSync] Modelica Web Worker not ready; retrying parse doc={} gen={}",
                doc_id.raw(),
                gen,
            );
            continue;
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            handle.upsert_document_async(doc_id, gen, source, |task| {
                pool.spawn(async move { task() }).detach();
            });
        }
        bevy::log::info!(
            "[EngineSync] async parse spawned doc={} gen={} src={}B (first_parse={}, target={}{})",
            doc_id.raw(),
            gen,
            src_len,
            !was_parsed,
            if dispatched_to_worker {
                "worker"
            } else {
                "native-task"
            },
            if Some(doc_id) == active_doc {
                ", priority=active"
            } else {
                ""
            },
        );
    }
    cursor.registry_revision = registry.revision();
}

/// Drain parse-done envelopes from the off-thread Web Worker and
/// install each AST into the engine session.
///
/// Wasm-only system. The worker emits one
/// [`crate::worker_transport`] per
/// finished parse; the transport layer pushes each into a crossbeam
/// channel; this system pulls them off the channel and routes each
/// through [`ModelicaEngineHandle::install_worker_parsed_ast`] (success)
/// or [`ModelicaEngineHandle::finish_pending_failed`] (parse error).
///
/// Native: still registered but always sees an empty queue (worker
/// never runs there), so the system is a per-tick HashMap miss —
/// negligible.
// `registry` is mutated only in the wasm worker-parse path below; on native
// the queue is always empty and the cfg block is excluded, so the `mut` reads as
// unused there. Allow it on native only — wasm genuinely needs it.
#[cfg_attr(not(target_arch = "wasm32"), allow(unused_mut))]
pub fn drain_worker_parse_results(
    handle: Res<ModelicaEngineHandle>,
    mut registry: ResMut<crate::state::ModelicaDocumentRegistry>,
) {
    #[cfg(target_arch = "wasm32")]
    {
        use crate::document::SyntaxCache;
        use std::sync::Arc;
        while let Some(env) = crate::worker_transport::try_recv_parse_failed() {
            handle.finish_worker_parse_failed(env.doc_id, env.gen, env.error.clone());
            bevy::log::error!(
                "[EngineSync] worker parse failed doc={} gen={}: {}",
                env.doc_id.raw(),
                env.gen,
                env.error,
            );
        }
        while let Some(env) = crate::worker_transport::try_recv_parse_done() {
            // Lenient parser always returns an AST. `errors` carries
            // any recovery diagnostics; `is_empty()` ⇒ source was
            // well-formed. Both fields land in the doc's single
            // `SyntaxCache` and the engine session adopts the AST as
            // its canonical view.
            let ast_arc = Arc::new(env.ast);
            handle.install_worker_parsed_ast(env.doc_id, env.gen, (*ast_arc).clone());
            if let Some(host) = registry.host_mut(env.doc_id) {
                let syntax = SyntaxCache {
                    generation: env.gen,
                    ast: ast_arc,
                    errors: env.errors.clone(),
                };
                host.document_mut().install_parse_results(syntax);
            }
            // Wake the watermark observer for the class-diff
            // changes the rebuild may have just pushed.
            registry.mark_changed(env.doc_id);
            if env.errors.is_empty() {
                bevy::log::info!(
                    "[EngineSync] worker-parsed install doc={} gen={}",
                    env.doc_id.raw(),
                    env.gen,
                );
            } else {
                bevy::log::warn!(
                    "[EngineSync] worker-parsed install doc={} gen={} with {} parse error(s)",
                    env.doc_id.raw(),
                    env.gen,
                    env.errors.len(),
                );
            }
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (handle, registry);
    }
}

/// Plugin registering the engine handle, sync cursor, and sync
/// system. Add once at app build; safe to add multiple times because
/// every component is `init_resource` / unique-system.
pub struct ModelicaEnginePlugin;

impl Plugin for ModelicaEnginePlugin {
    fn build(&self, app: &mut App) {
        // Install the resource and mirror it into the process-wide
        // `GLOBAL_ENGINE` slot so static helpers (`class_cache`,
        // off-thread projection tasks) read the same handle the
        // resource exposes. The clone is `Arc`-cheap.
        let handle = ModelicaEngineHandle::default();
        let _ = GLOBAL_ENGINE.set(handle.clone());
        app.insert_resource(handle)
            .init_resource::<EngineSyncCursor>()
            .init_resource::<ParsePacing>()
            .init_resource::<MslBootstrapState>()
            .init_resource::<MslBootstrapTask>()
            .add_systems(
                Update,
                (
                    drive_engine_sync,
                    drive_msl_bootstrap,
                    drain_worker_parse_results,
                ),
            );
    }
}

/// Tracks whether the MSL bundle has been bootstrapped into the
/// workspace engine. Once `Done`, `drive_msl_bootstrap` becomes a
/// no-op for the rest of the session.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum MslBootstrapState {
    #[default]
    Pending,
    Loading,
    Done,
}

/// Background source-index install for a resident MSL bundle.
///
/// `Session::replace_parsed_source_set` builds the cross-file scope index and
/// can take hundreds of milliseconds even when parsing has already happened.
/// Keep that work off the Bevy update thread; the shared engine mutex is the
/// only rendezvous and UI readers use `try_lock`/cache-only APIs while it is
/// held.
#[derive(Resource, Default)]
struct MslBootstrapTask(Option<bevy::tasks::Task<usize>>);

/// Notification event emitted exactly once per session by
/// [`drive_msl_bootstrap`] the frame MSL is installed into the
/// workspace engine session.
///
/// This is a *notification* (system tells the world "MSL is now
/// resolvable"), **not** a user-facing command — observe it with
/// `app.add_observer(fn)` rather than dispatching it from UI.
///
/// Typical observer: re-trigger canvas diagram projection so
/// standard-library component icons resolve (they show as blank
/// boxes when projected before MSL was available).
#[derive(Event, Clone, Debug)]
pub struct MslBecameReady;

/// Generic source-root completion. The UI uses the same re-projection seam
/// for MSL, LunCo, and future bundled libraries.
#[derive(Event, Clone, Debug)]
pub struct ModelicaLibraryBecameReady {
    pub root: String,
}

/// Bevy system: once the pre-parsed MSL bundle is **resident in memory**,
/// bulk-install it into the workspace engine as a `DurableExternal` source root
/// so main-thread hover/diagnostics/resolution see all of MSL at once. Runs at
/// most once per session — flips [`MslBootstrapState`] to `Done` and idles.
///
/// **The parsed source slot is the readiness predicate.** Install fires iff
/// `msl_remote::global_parsed_msl()` is populated (the in-process slot holds the
/// parsed `Vec<(uri, StoredDefinition)>`); install is a clone plus the engine's
/// source-set boundary, with no re-parsing. `MslLoadState::Ready` only means
/// that the source tree/index is available; native fills the parsed slot lazily
/// after a class is first opened, so using that UI state as the bootstrap gate
/// would strand the shared engine before the actual AST source set exists. The
/// system itself is target-agnostic — the native/web difference is **upstream**,
/// in *who fills that slot*:
///
/// - **Web:** the worker-decoded bundle is installed into the slot during boot,
///   so this runs the bulk install → instant full-MSL resolution.
/// - **Native:** the slot is filled **lazily** (first `parsed_msl_bundle()` disk
///   read), so at boot it is empty and this system waits without reading the
///   bundle. The first class lookup seats the complete bundle through the same
///   engine source-set boundary; this system then publishes the common ready
///   notification for documents that were projected before that lookup.
fn drive_msl_bootstrap(
    handle: Res<ModelicaEngineHandle>,
    mut bootstrap: ResMut<MslBootstrapState>,
    mut task: ResMut<MslBootstrapTask>,
    mut commands: Commands,
) {
    if matches!(*bootstrap, MslBootstrapState::Done) {
        return;
    }

    // A first class lookup can install the complete source set before this
    // system observes the resident bundle. That lookup must publish the same
    // lifecycle event so diagrams projected before it are reprojected.
    // Checking this boundary before polling or spawning also prevents a race
    // from installing the immutable source set twice.
    if handle
        .try_lock()
        .is_some_and(|engine| engine.source_set_installed("msl"))
    {
        task.0 = None;
        *bootstrap = MslBootstrapState::Done;
        bevy::log::info!("[EngineBootstrap] MSL source set already installed by class lookup");
        commands.trigger(MslBecameReady);
        #[cfg(target_arch = "wasm32")]
        crate::worker_transport::prewarm_pool_on_msl_ready();
        return;
    }

    // The source-set index is a CPU-heavy session mutation. Poll only the
    // completion here; the actual clone and index build run in the task below.
    // This keeps the Bevy update thread available for input, rendering, and
    // API wakeups while the shared engine is being prepared.
    if let Some(background) = task.0.as_mut() {
        if let Some(count) =
            futures_lite::future::block_on(futures_lite::future::poll_once(background))
        {
            task.0 = None;
            *bootstrap = MslBootstrapState::Done;
            bevy::log::info!(
                "[EngineBootstrap] installed MSL into workspace engine: {count} pre-parsed docs"
            );
            commands.trigger(MslBecameReady);
            #[cfg(target_arch = "wasm32")]
            crate::worker_transport::prewarm_pool_on_msl_ready();
        }
        return;
    }

    // A resident bundle is an immutable input snapshot. Clone only its Arc on
    // the update thread; copying the definitions and building the source index
    // happen on the worker. The install still uses the one shared Modelica
    // session, so all readers observe the same standard Modelica namespace.
    let Some(docs) = crate::msl_remote::global_parsed_msl().cloned() else {
        return;
    };
    let handle = handle.clone();
    task.0 = Some(bevy::tasks::AsyncComputeTaskPool::get().spawn(async move {
        let defs: Vec<(String, rumoca_compile::parsing::ast::StoredDefinition)> = docs
            .iter()
            .map(|(uri, definition)| (uri.clone(), definition.clone()))
            .collect();
        let count = defs.len();
        let mut engine = handle.lock();
        engine.replace_parsed_source_set(
            "msl",
            rumoca_compile::compile::SourceRootKind::DurableExternal,
            defs,
        );
        count
    }));
    *bootstrap = MslBootstrapState::Loading;
    bevy::log::info!("[EngineBootstrap] installing resident MSL source index in background");
}
