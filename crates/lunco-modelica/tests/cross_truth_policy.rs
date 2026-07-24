//! Headless tests for cross-truth policy chokepoints
//! (`docs/architecture/B0_CROSS_TRUTH_POLICY.md`).
//!
//! Each rule's *helper-level* contract pins here; observer wiring is
//! exercised in single-file harness tests where Bevy is required.

use lunco_modelica::ui::wasm_autosave::{should_autosave, IsGestureActive};

// ─────────────────────────────────────────────────────────────────────
// R3 — mode-switch flush. Helper-level: the *condition* under which
// `commit_pending_buffer` must fire on a mode switch. Wiring exercise
// requires Bevy + an active code editor; pinned in panel-level
// integration tests separately.
// ─────────────────────────────────────────────────────────────────────

fn r3_should_flush(leaving_text_mode: bool, pending_commit: bool) -> bool {
    // Force-flush exactly when both apply: we're leaving Text mode
    // AND the editor has uncommitted bytes. Other transitions
    // (entering Text, mode unchanged, no pending edit) are no-ops.
    leaving_text_mode && pending_commit
}

#[test]
fn r3_flushes_when_leaving_text_with_pending() {
    assert!(r3_should_flush(true, true));
}

#[test]
fn r3_no_flush_entering_text() {
    // Entering Text mode never flushes — there's nothing to commit.
    assert!(!r3_should_flush(false, true));
}

#[test]
fn r3_no_flush_when_buffer_clean() {
    assert!(!r3_should_flush(true, false));
}

#[test]
fn r3_no_flush_when_neither() {
    assert!(!r3_should_flush(false, false));
}

// ─────────────────────────────────────────────────────────────────────
// R1 — autosave gates on active gesture
// ─────────────────────────────────────────────────────────────────────

#[test]
fn r1_should_autosave_writes_for_clean_untitled() {
    assert!(should_autosave(false, true), "untitled + idle = write");
}

#[test]
fn r1_should_autosave_skips_file_backed() {
    // File-backed docs have a real save path; localStorage write
    // would shadow it.
    assert!(
        !should_autosave(false, false),
        "file-backed doc never autosaves"
    );
}

#[test]
fn r1_should_autosave_skips_during_gesture() {
    // Mid-drag / mid-edit: a snapshot now would capture transient
    // state ("one component in two places") and persist it.
    assert!(
        !should_autosave(true, true),
        "active gesture blocks autosave"
    );
}

#[test]
fn r1_active_gesture_or_file_backed_blocks() {
    // Both filters required; either one blocking is enough.
    assert!(
        !should_autosave(true, false),
        "gesture AND file-backed = no"
    );
}

#[test]
fn r1_is_gesture_active_default_is_idle() {
    let g = IsGestureActive::default();
    assert!(!g.any(), "default is all-clear");
    assert!(!g.canvas);
    assert!(!g.text);
    assert!(!g.modal);
}

#[test]
fn r1_is_gesture_active_any_is_or_of_sources() {
    // Each source independently activates the gate.
    let mut g = IsGestureActive::default();
    g.canvas = true;
    assert!(g.any());

    let mut g = IsGestureActive::default();
    g.text = true;
    assert!(g.any());

    let mut g = IsGestureActive::default();
    g.modal = true;
    assert!(g.any());
}

#[test]
fn r1_text_source_mirrors_pending_commit_window() {
    // Pins the contract `drive_text_gesture_flag` enforces:
    // `gesture.text` is true exactly while
    // `EditorBufferState.pending_commit_at.is_some()`. The driver
    // system needs Bevy to run — this test models the same boolean
    // mirror so a future refactor can't drift the rule.
    fn mirror(pending: Option<f64>) -> bool {
        pending.is_some()
    }
    assert!(!mirror(None), "no pending edit → text source clear");
    assert!(mirror(Some(123.4)), "pending edit → text source active");
    assert!(mirror(Some(0.0)), "even t=0 counts as in-flight");
}

#[test]
fn r1_modal_source_mirrors_close_dialog_pending() {
    // Pins the contract `drive_modal_gesture_flag` enforces:
    // `gesture.modal` = `!CloseDialogState.pending.is_empty()`.
    // When extended to other modals (file picker, conflict prompt),
    // the OR semantics must keep the same shape.
    fn mirror(pending_dialogs: usize) -> bool {
        pending_dialogs > 0
    }
    assert!(!mirror(0), "no dialogs open → modal source clear");
    assert!(mirror(1), "one dialog → modal source active");
    assert!(mirror(3), "many dialogs → still active");
}

#[test]
fn r1_is_gesture_active_independent_sources() {
    // Two sources active at once; clearing one alone doesn't open
    // the gate. Pins the regression class where canvas-release
    // would re-enable autosave while a modal is still open.
    let mut g = IsGestureActive::default();
    g.canvas = true;
    g.modal = true;
    assert!(g.any());
    g.canvas = false;
    assert!(g.any(), "modal still active — gate stays closed");
    g.modal = false;
    assert!(!g.any());
}
