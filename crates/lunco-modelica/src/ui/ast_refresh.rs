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

use crate::ui::state::ModelicaDocumentRegistry;

/// Quiet window before a debounced reparse fires. 250 ms matches
/// VS Code's default AST-refresh cadence and sits comfortably above
/// the "I'm still typing" threshold for competent typists (~6–8
/// keystrokes/s) while keeping the worst-case observed-AST lag short
/// enough that lint + diagram updates feel live.
pub const AST_DEBOUNCE_MS: u128 = 250;

/// Per-Update driver. Walks every doc in the registry and reparses
/// the stale ones whose edit burst has cooled off.
pub fn refresh_stale_asts(mut registry: ResMut<ModelicaDocumentRegistry>) {
    let now = web_time::Instant::now();
    // Collect candidates first (immutable pass) so we can fall back
    // to mutable access without borrowing the registry twice.
    let stale: Vec<lunco_doc::DocumentId> = registry
        .docs()
        .filter_map(|(id, host)| {
            let doc = host.document();
            if !doc.ast_is_stale() {
                return None;
            }
            let last = doc.last_source_edit_at()?;
            (now.duration_since(last).as_millis() >= AST_DEBOUNCE_MS).then_some(id)
        })
        .collect();
    for id in stale {
        if let Some(host) = registry.host_mut(id) {
            host.document_mut().refresh_ast_now();
        }
    }
}
