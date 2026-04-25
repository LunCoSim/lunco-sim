//! Debounced AST reparse driver.
//!
//! Every text-editor keystroke hits
//! [`ModelicaDocument::apply_patch`](crate::document::ModelicaDocument),
//! which used to call `rumoca_phase_parse::parse_to_ast` synchronously
//! on the main thread. A single parse is cheap (a few ms on small
//! files) but under a fast typist, while the simulation worker is
//! concurrently pushing sample batches through the main thread for
//! plot rendering, those milliseconds accumulate into visibly laggy
//! frames. Editing a comment during a run could drop frame rate
//! below 30 Hz on the reference machine.
//!
//! Rather than reparse per-keystroke, `apply_patch` now only:
//!   - advances the source (`source.replace_range`)
//!   - bumps `generation`
//!   - stamps `last_source_edit_at`
//!
//! The previous `AstCache` stays live but *stale* (its `generation`
//! lags `document.generation`). This system ticks every
//! [`bevy::prelude::Update`] and, for any doc whose AST is stale
//! **and** hasn't been edited in the last [`AST_DEBOUNCE_MS`]
//! milliseconds, runs the catch-up reparse.
//!
//! Rapid typing therefore keeps the AST frozen at the pre-typing
//! state for the duration of the burst. The moment the user pauses
//! ≥250 ms the system reparses once and everything downstream
//! (diagram projection, lint, diagnostics) sees the new AST.
//!
//! For correctness-critical consumers that must observe the exact
//! current source (Compile, Format Document), call
//! [`ModelicaDocument::refresh_ast_now`](crate::document::ModelicaDocument::refresh_ast_now)
//! explicitly to force the reparse on the spot.

use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task};
use futures_lite::future;
use std::collections::HashMap;

use crate::document::AstCache;
use crate::ui::state::ModelicaDocumentRegistry;

/// Quiet window before a debounced reparse fires. 250 ms matches
/// VS Code's default AST-refresh cadence and sits comfortably above
/// the "I'm still typing" threshold for competent typists (~6–8
/// keystrokes/s) while keeping the worst-case observed-AST lag short
/// enough that lint + diagram updates feel live.
pub const AST_DEBOUNCE_MS: u128 = 250;

/// Tracks in-flight off-thread AST parses, keyed by document id.
///
/// rumoca's `parse_to_ast` is **very** slow in debug builds (~2 s on
/// a 20 KB Modelica file with deep imports — verified empirically).
/// Synchronous reparse on the main thread froze the UI for the
/// duration of every structural edit. Parsing on
/// `AsyncComputeTaskPool` and polling here keeps the main thread
/// responsive — every consumer (canvas projection, telemetry,
/// diagnostics) reads the *previous* AST until the new one lands,
/// which is fine because they already tolerate stale-by-one-edit
/// reads.
///
/// One entry per doc; a new edit while a parse is in flight just
/// lets the existing parse finish — `install_ast` discards the
/// result if the doc's generation has moved on, and the next
/// debounce tick will spawn a fresh parse against the latest source.
#[derive(Resource, Default)]
pub struct PendingAstParses {
    by_doc: HashMap<lunco_doc::DocumentId, Task<AstCache>>,
}

/// Per-Update driver. Drains completed parse tasks first (so a fresh
/// AST is visible this frame), then spawns parses for any docs whose
/// edit burst has cooled off and aren't already being parsed.
pub fn refresh_stale_asts(
    mut registry: ResMut<ModelicaDocumentRegistry>,
    mut pending: ResMut<PendingAstParses>,
) {
    // ── Drain completed parses ───────────────────────────────────
    let ready: Vec<(lunco_doc::DocumentId, AstCache)> = pending
        .by_doc
        .iter_mut()
        .filter_map(|(id, task)| {
            future::block_on(future::poll_once(task)).map(|ast| (*id, ast))
        })
        .collect();
    for (id, ast) in ready {
        if let Some(host) = registry.host_mut(id) {
            host.document_mut().install_ast(ast);
        }
        pending.by_doc.remove(&id);
    }

    // ── Spawn parses for newly-stale docs ────────────────────────
    let now = web_time::Instant::now();
    let to_spawn: Vec<(lunco_doc::DocumentId, String, u64)> = registry
        .docs()
        .filter_map(|(id, host)| {
            if pending.by_doc.contains_key(&id) {
                return None; // already parsing
            }
            let doc = host.document();
            if !doc.ast_is_stale() {
                return None;
            }
            let last = doc.last_source_edit_at()?;
            (now.duration_since(last).as_millis() >= AST_DEBOUNCE_MS).then(|| {
                (
                    id,
                    doc.source_snapshot(),
                    <crate::document::ModelicaDocument as lunco_doc::Document>::generation(doc),
                )
            })
        })
        .collect();
    if !to_spawn.is_empty() {
        let pool = AsyncComputeTaskPool::get();
        for (id, source, gen) in to_spawn {
            let bytes = source.len();
            let task = pool.spawn(async move {
                let t = web_time::Instant::now();
                let ast = AstCache::from_source(&source, gen);
                let elapsed_ms = t.elapsed().as_secs_f64() * 1000.0;
                if elapsed_ms > 50.0 {
                    bevy::log::info!(
                        "[ast_refresh] off-thread parse: {bytes} bytes in {elapsed_ms:.1}ms (gen={gen})"
                    );
                }
                ast
            });
            pending.by_doc.insert(id, task);
        }
    }
}
