//! Icon pre-warmer.
//!
//! On [`DocumentOpened`](lunco_doc_bevy::DocumentOpened) we walk the
//! doc's AST collecting every cross-package type referenced (component
//! types, extends bases, connector port types). A single
//! [`bevy::tasks::AsyncComputeTaskPool`] task fans out
//! [`crate::class_cache::peek_or_load_class_blocking`] for each unique
//! qualified name, then primes the engine's icon resolution by calling
//! [`crate::engine::ModelicaEngine::icon_for`] for each one.
//!
//! Effect: by the time the user drills into a class whose icon merge
//! requires walking a source-library extends chain, rumoca's
//! `class_interface_index_query` cache is already populated for every
//! class on the chain — drill-in projection finishes in milliseconds
//! instead of the cold-walk seconds.
//!
//! Idempotent: re-firing for the same doc is fine (rumoca's content-hash
//! short-circuits repeated work). A warm miss remains an explicit unresolved
//! class state; the source-root completion event schedules the normal
//! re-projection that can resolve the authored icon.
//!
//! AST-as-source-of-truth: the warmer reads the doc's AST directly
//! via [`crate::engine::ModelicaEngine::parsed_for_doc`]. No re-parse,
//! no peeking at the source bytes.

use bevy::prelude::*;
#[cfg(not(target_arch = "wasm32"))]
use bevy::tasks::AsyncComputeTaskPool;
#[cfg(not(target_arch = "wasm32"))]
use lunco_doc::DocumentId;
use lunco_doc_bevy::DocumentOpened;
#[cfg(not(target_arch = "wasm32"))]
use rumoca_compile::parsing::ast::{ClassDef, StoredDefinition};
#[cfg(not(target_arch = "wasm32"))]
use std::collections::HashSet;

/// Bevy plugin: registers the `DocumentOpened` observer that fans out
/// pre-warm tasks. Add once per app.
pub struct IconWarmerPlugin;

impl Plugin for IconWarmerPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_document_opened_warm);
    }
}

/// Wasm has no background task that can safely warm the engine.
#[cfg(target_arch = "wasm32")]
fn on_document_opened_warm(
    _trigger: On<DocumentOpened>,
    _registry: Res<lunco_doc_bevy::DocumentRegistry<lunco_modelica_document::ModelicaDocument>>,
) {
}

/// Observer body — extracted so unit tests can drive it without Bevy.
#[cfg(not(target_arch = "wasm32"))]
fn on_document_opened_warm(
    trigger: On<DocumentOpened>,
    registry: Res<lunco_doc_bevy::DocumentRegistry<lunco_modelica_document::ModelicaDocument>>,
) {
    let doc_id = trigger.event().doc;
    let Some(host) = registry.host(doc_id) else {
        return;
    };
    // Read from the doc's lenient cache — it's a reactive mirror of
    // the engine's strict parse, populated by either
    // `drive_engine_sync`'s drain step (workspace docs) or
    // the library document loader's strict-adopt path (drill-in / library docs).
    //
    // We must NOT call `engine.upsert_document(...)` here: for a
    // drill-in into Modelica.Blocks.* the source is the whole
    // 152 kB Blocks/package.mo, and synchronously parsing that on
    // the main thread freezes the workbench for 100+ seconds in
    // dev. The engine catches up async via `drive_engine_sync` anyway; an
    // icon paint that misses the warm cache remains unresolved until the
    // normal background projection resolves it.
    // **Wasm: warmer disabled.** `AsyncComputeTaskPool` is the main
    // thread on wasm32-unknown-unknown, so the warm task's
    // `engine.icon_for(ty)` calls — each up to ~1.3 s on a cold source library
    // qualified-name lookup — block the UI exactly as if they ran
    // synchronously. Native still benefits because AsyncCompute there has
    // its own threads.
    let types = collect_referenced_types(&host.document().syntax_arc().ast);
    if types.is_empty() {
        return;
    }
    spawn_warm_task(doc_id, types);
}

/// Walk `ast` collecting every unique fully-qualified or partially-
/// qualified type reference that's worth warming. Skips Modelica
/// built-in scalars and bare local names (already in the doc).
#[cfg(not(target_arch = "wasm32"))]
fn collect_referenced_types(ast: &StoredDefinition) -> Vec<String> {
    let mut out: HashSet<String> = HashSet::new();
    for class in ast.classes.values() {
        walk_class(class, &mut out);
    }
    out.into_iter().collect()
}

#[cfg(not(target_arch = "wasm32"))]
fn walk_class(class: &ClassDef, out: &mut HashSet<String>) {
    lunco_modelica_ast::ast_extract::walk_class_type_names(class, &mut |name| {
        if interesting_type(name) {
            out.insert(name.to_string());
        }
    });
}

/// True for type names worth warming. Filters out Modelica built-ins
/// and bare names (which resolve locally — no warm needed).
#[cfg(not(target_arch = "wasm32"))]
fn interesting_type(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    if !name.contains('.') {
        // Bare name → resolves locally via rumoca's suffix-match;
        // already covered by the synchronous engine upsert.
        return false;
    }
    !lunco_modelica_ast::ast_extract::is_builtin_type_name(name)
}

#[cfg(not(target_arch = "wasm32"))]
fn spawn_warm_task(doc_id: DocumentId, types: Vec<String>) {
    let n = types.len();
    let started = web_time::Instant::now();
    bevy::log::debug!(
        "[IconWarmer] doc={} fanning out {} type warm tasks",
        doc_id.raw(),
        n
    );
    AsyncComputeTaskPool::get()
        .spawn(async move {
            // **Cache-only warm.** We deliberately do NOT call
            // `peek_or_load_class_blocking` here — it would parse large
            // source library files (200KB+) under the engine mutex, blocking
            // both the projection task and any main-thread query
            // for tens of seconds in dev builds. That regresses the
            // `feels instant` first paint we just won.
            //
            // Instead: hit `engine.icon_for` for every type. Classes
            // already in the session warm their inheritance walk
            // (cheap). source library classes not yet loaded silently return
            // None and stay cold; their first projection-time miss
            // pays the load cost, but only when a user actually
            // drills into them. Future: pre-load source library files in tiny
            // batches between frames to trickle them in without
            // mutex contention.
            // Per-type lock + yield. On wasm `AsyncComputeTaskPool`
            // **is** the main thread, so a single 1.3 s loop holding
            // the engine mutex stalls every frame inside it. Drop
            // the lock between types and yield back to the runtime
            // so input + render systems get to run; resume on the
            // next tick. Native gets a tiny per-iteration overhead
            // but the warmer is best-effort anyway.
            let mut warmed = 0usize;
            for ty in &types {
                if let Some(handle) = crate::engine_resource::global_engine_handle() {
                    let mut engine = handle.lock();
                    if engine.icon_for(ty).is_some() {
                        warmed += 1;
                    }
                }
                bevy::tasks::futures_lite::future::yield_now().await;
            }
            bevy::log::info!(
                "[IconWarmer] doc={} warmed {}/{} types in {:.0}ms (cache-only)",
                doc_id.raw(),
                warmed,
                n,
                started.elapsed().as_secs_f64() * 1000.0
            );
        })
        .detach();
}
