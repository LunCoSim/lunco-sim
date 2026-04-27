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
use bevy::tasks::AsyncComputeTaskPool;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::document::AstCache;
use crate::ui::state::ModelicaDocumentRegistry;

/// Shared slot the worker thread fills when its parse completes.
/// `Arc<Mutex<Option<_>>>` mirrors the pattern used by
/// `ui::image_loader` for the same reason: `mpsc::Receiver` is
/// `Send`-only, but Bevy `Resource` needs `Sync`. The mutex is
/// touched at most twice (worker writes once at parse completion,
/// main thread reads via `try_lock` once per Update tick), so
/// contention is negligible.
type ParseSlot = Arc<Mutex<Option<AstCache>>>;

/// Quiet window before a debounced reparse fires.
///
/// Was 250 ms (VS Code's default). Bumped to 2500 ms after profiling
/// showed the rumoca parse takes ~2.5 s in debug builds for a 20 KB
/// Modelica file, and even on a separate thread the parse causes
/// noticeable CPU contention with bevy_render's pipelined extract
/// (`Last` schedule blocks for ~parse-duration; verified in
/// telemetry: every multi-second slow frame correlates 1:1 with an
/// in-flight parse). With a long debounce, user gestures (drag,
/// rapid Add) don't keep re-arming the reparse — the parse only
/// fires after the user is idle for 2.5 s, by which point the
/// renderer has plenty of headroom.
///
/// Trade-off: panels that read the AST (inspector, lint) lag by up
/// to 2.5 s after the last edit. Acceptable because:
///   - Optimistic synth places nodes on canvas immediately
///   - Compile / Format / API path force-refresh via
///     `refresh_ast_now` regardless of debounce
///   - Inspector's "new component just added" view fills in once
///     the user pauses
pub const AST_DEBOUNCE_MS: u128 = 2500;

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
/// Per-doc receiver for the most recent in-flight parse on a
/// dedicated OS thread. Bevy's `AsyncComputeTaskPool` shares its
/// thread budget with bevy_render's pipelined extract on small-core
/// machines — a multi-second rumoca parse there starves the
/// renderer and the entire `Last` schedule blocks until the parse
/// completes (verified in telemetry: `[FrameTimeProbe] total=3599ms
/// last=3596`). Using `std::thread::spawn` guarantees an OS thread
/// the kernel scheduler can keep separate from bevy's render thread.
#[derive(Resource, Default)]
pub struct PendingAstParses {
    by_doc: HashMap<lunco_doc::DocumentId, ParseSlot>,
}

/// Per-Update driver. Drains completed parse tasks first (so a fresh
/// AST is visible this frame), then spawns parses for any docs whose
/// edit burst has cooled off and aren't already being parsed.
///
/// **Idle gate**: spawn is suppressed while
/// [`crate::ui::input_activity::InputActivity::is_active`] returns
/// true. The user's mouse moves, drags, clicks, and edits all reset
/// the activity timer. Parses only fire after 500 ms of true idle —
/// breaks the "every Add freezes UI for 1.5 s" feedback loop where
/// rumoca on a background thread still correlated 1:1 with bevy's
/// `Last`-schedule render stall.
pub fn refresh_stale_asts(
    mut registry: ResMut<ModelicaDocumentRegistry>,
    mut pending: ResMut<PendingAstParses>,
    activity: Res<crate::ui::input_activity::InputActivity>,
) {
    // ── Drain completed parses ───────────────────────────────────
    // Non-blocking `try_lock` per slot; the worker thread fills the
    // slot when its parse finishes. We never block the Update tick
    // on the parse — if a slot is contended (worker mid-write) we
    // skip and check again next tick.
    let ready: Vec<(lunco_doc::DocumentId, AstCache)> = pending
        .by_doc
        .iter()
        .filter_map(|(id, slot)| {
            let mut guard = slot.try_lock().ok()?;
            guard.take().map(|ast| (*id, ast))
        })
        .collect();
    for (id, ast) in ready {
        if let Some(host) = registry.host_mut(id) {
            host.document_mut().install_ast(ast);
        }
        pending.by_doc.remove(&id);
    }

    // ── Idle gate ────────────────────────────────────────────────
    // Defer all parse spawns until the user has been idle for at
    // least `IDLE_THRESHOLD_MS`. Drains above always run because a
    // task that's already complete is free to install — it's just
    // the *spawning* that we want to keep off the user's path.
    if activity.is_active() {
        return;
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
    // DIAGNOSTIC: temporarily skip ALL background parses to confirm
    // whether the parse itself is what blocks the `Last` schedule.
    // Set `LUNCO_DISABLE_BG_PARSE=1` to suppress.
    if std::env::var_os("LUNCO_DISABLE_BG_PARSE").is_some() {
        if !to_spawn.is_empty() {
            bevy::log::info!(
                "[ast_refresh] DIAGNOSTIC: would spawn {} parses, skipped",
                to_spawn.len()
            );
        }
        return;
    }
    if !to_spawn.is_empty() {
        let pool = AsyncComputeTaskPool::get();
        for (id, source, gen) in to_spawn {
            let bytes = source.len();
            let slot: ParseSlot = Arc::new(Mutex::new(None));
            let slot_for_worker = slot.clone();
            // `spawn().detach()` releases the `Task<T>` handle so we
            // never have to poll-or-drop it on the main thread; the
            // task runs to completion on the pool, fills the shared
            // slot, and the next Update tick observes it via
            // `try_lock`. Using Bevy's `AsyncComputeTaskPool` keeps
            // the parse on the platform's chosen thread budget
            // (cross-platform incl. wasm32 cooperative scheduling).
            pool.spawn(async move {
                let t = web_time::Instant::now();
                let ast = AstCache::from_source(&source, gen);
                let elapsed_ms = t.elapsed().as_secs_f64() * 1000.0;
                if elapsed_ms > 50.0 {
                    bevy::log::info!(
                        "[ast_refresh] off-thread parse: {bytes} bytes in {elapsed_ms:.1}ms (gen={gen})"
                    );
                }
                if let Ok(mut guard) = slot_for_worker.lock() {
                    *guard = Some(ast);
                }
            })
            .detach();
            pending.by_doc.insert(id, slot);
        }
    }
}
