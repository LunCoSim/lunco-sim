//! B.6 — `ModelTabs` lifecycle invariants.
//!
//! Headless tests for the three entry points and the close paths.
//! Pins the contract documented in `model_view.rs::ModelTabs` ("Tab
//! lifecycle decision tree") so future refactors don't silently
//! drift away from VS Code semantics.
//!
//! Per AGENTS.md §1 these land before any further B.x cleanup; each
//! singleton retire (B.3) and per-tab buffer-state migration (B.2)
//! depends on `ModelTabs` keeping its current contract.

use lunco_doc::DocumentId;
use lunco_modelica::model_tabs::ModelTabs;
use lunco_modelica::model_tabs_types::ModelViewMode;

fn doc(n: u64) -> DocumentId {
    DocumentId::new(n)
}

// ─────────────────────────────────────────────────────────────────────
// ensure_for — deliberate open
// ─────────────────────────────────────────────────────────────────────

#[test]
fn ensure_for_dedups_same_doc_and_drilled_scope() {
    let mut tabs = ModelTabs::default();
    let a = tabs.ensure_for(doc(1), None);
    let b = tabs.ensure_for(doc(1), None);
    assert_eq!(a, b, "ensure_for must dedup same (doc, None)");
    assert_eq!(
        tabs.count_for_doc(doc(1)),
        1,
        "expected 1 tab, got duplicates"
    );
}

#[test]
fn ensure_for_distinguishes_drilled_scopes() {
    let mut tabs = ModelTabs::default();
    let root = tabs.ensure_for(doc(1), None);
    let drilled = tabs.ensure_for(doc(1), Some("Foo.Bar".into()));
    assert_ne!(
        root, drilled,
        "different drilled scope must yield distinct tab"
    );
    assert_eq!(tabs.count_for_doc(doc(1)), 2);
}

#[test]
fn ensure_for_pins_new_tabs_by_default() {
    let mut tabs = ModelTabs::default();
    let id = tabs.ensure_for(doc(1), None);
    let state = tabs.get(id).expect("tab present");
    assert!(state.pinned, "deliberate-open tabs must be pinned");
}

// ─────────────────────────────────────────────────────────────────────
// ensure_preview_for — browser single-click semantics
// ─────────────────────────────────────────────────────────────────────

#[test]
fn ensure_preview_for_new_doc_creates_unpinned() {
    let mut tabs = ModelTabs::default();
    let (id, evict) = tabs.ensure_preview_for(doc(1), None);
    assert!(evict.is_none(), "no prior preview to evict");
    let state = tabs.get(id).expect("tab present");
    assert!(
        !state.pinned,
        "preview tabs must NOT be pinned on first open"
    );
}

#[test]
fn ensure_preview_for_evicts_old_preview_and_allocates_new() {
    // Browser click on doc(1), then click on doc(2). Architectural
    // invariant: TabId → (doc, drilled) is immutable. So instead of
    // mutating the existing preview's binding, ensure_preview_for
    // returns the OLD preview's id for the caller to close, and
    // allocates a NEW TabId for the second click. Per-tab state
    // keyed on TabId never gets stranded.
    let mut tabs = ModelTabs::default();
    let (preview_a, evict_a) = tabs.ensure_preview_for(doc(1), None);
    assert!(evict_a.is_none(), "no prior preview");
    let (preview_b, evict_b) = tabs.ensure_preview_for(doc(2), None);
    assert_eq!(
        evict_b,
        Some(preview_a),
        "old preview returned for eviction"
    );
    assert_ne!(preview_a, preview_b, "new TabId allocated, not reused");
    // The caller is expected to close `evict_b` (preview_a). Simulate
    // that here so the post-state matches "after eviction".
    tabs.close_tab(preview_a);
    assert!(tabs.get(preview_a).is_none(), "old preview gone");
    let state_b = tabs.get(preview_b).expect("new preview present");
    assert_eq!(state_b.doc, doc(2));
    assert!(!state_b.pinned, "still unpinned");
}

#[test]
fn ensure_preview_for_focuses_existing_match() {
    // If the (doc, drilled) is already open, focus that tab — don't
    // touch the preview slot. Independent of pinned state.
    let mut tabs = ModelTabs::default();
    let pinned_id = tabs.ensure_for(doc(1), None);
    let (focused, evict) = tabs.ensure_preview_for(doc(1), None);
    assert_eq!(
        focused, pinned_id,
        "must focus existing pinned tab, not allocate"
    );
    assert!(evict.is_none(), "no eviction when matching existing tab");
    assert_eq!(tabs.count_for_doc(doc(1)), 1, "no duplicate created");
}

#[test]
fn ensure_preview_for_does_not_steal_pinned_tab() {
    // Browser-click on a different doc with no preview slot
    // available (only pinned tabs exist) → allocate a fresh
    // unpinned tab. Pinned tabs are never even considered — the
    // architectural invariant (TabId binding immutable) makes the
    // "steal" case structurally impossible regardless of pinned state.
    let mut tabs = ModelTabs::default();
    let pinned = tabs.ensure_for(doc(1), None);
    let (preview, evict) = tabs.ensure_preview_for(doc(2), None);
    assert_ne!(pinned, preview);
    assert!(evict.is_none(), "no prior preview slot occupant");
    assert!(tabs.get(pinned).unwrap().pinned, "pinned tab unchanged");
    assert!(!tabs.get(preview).unwrap().pinned, "new preview unpinned");
    assert_eq!(
        tabs.get(pinned).unwrap().doc,
        doc(1),
        "pinned still on doc(1)"
    );
}

#[test]
fn pin_promotes_preview_to_pinned_and_clears_slot() {
    let mut tabs = ModelTabs::default();
    let (id, _) = tabs.ensure_preview_for(doc(1), None);
    assert!(!tabs.get(id).unwrap().pinned);
    tabs.pin(id);
    assert!(tabs.get(id).unwrap().pinned, "pin must promote");
    // After pinning the preview slot must be cleared — the next
    // `ensure_preview_for` on a different doc should allocate fresh
    // without evicting the now-pinned tab.
    let (other, evict) = tabs.ensure_preview_for(doc(2), None);
    assert!(evict.is_none(), "pinned tab is no longer in preview slot");
    assert_ne!(other, id, "fresh tab for the new preview");
    assert!(
        tabs.get(id).unwrap().pinned,
        "previously-pinned tab still alive"
    );
}

#[test]
fn pin_all_for_doc_promotes_every_matching_tab() {
    let mut tabs = ModelTabs::default();
    let (a, _) = tabs.ensure_preview_for(doc(1), None);
    let b = tabs.open_new(doc(1), Some("Inner".into()));
    // After pin_all_for_doc(1), `a` is promoted (it views doc(1)) and
    // the preview slot is cleared. Asking for doc(2)'s preview now
    // allocates fresh (no eviction) — the architectural invariant
    // keeps `a` and `b` both alive on doc(1).
    tabs.pin_all_for_doc(doc(1));
    assert!(tabs.get(a).unwrap().pinned, "preview a promoted");
    assert!(tabs.get(b).unwrap().pinned, "split b still pinned");
    let (c, evict) = tabs.ensure_preview_for(doc(2), None);
    assert!(evict.is_none(), "preview slot cleared by pin_all_for_doc");
    assert_ne!(c, a, "fresh tab allocated");
    assert_eq!(tabs.get(a).unwrap().doc, doc(1), "a still on doc(1)");
}

// ─────────────────────────────────────────────────────────────────────
// open_new — split / "open in new view"
// ─────────────────────────────────────────────────────────────────────

#[test]
fn open_new_always_allocates_fresh_tab() {
    let mut tabs = ModelTabs::default();
    let a = tabs.ensure_for(doc(1), None);
    let b = tabs.open_new(doc(1), None);
    assert_ne!(
        a, b,
        "open_new must allocate even when (doc, drilled) matches"
    );
    assert_eq!(tabs.count_for_doc(doc(1)), 2);
}

#[test]
fn open_new_pins_by_default() {
    let mut tabs = ModelTabs::default();
    let id = tabs.open_new(doc(1), None);
    assert!(
        tabs.get(id).unwrap().pinned,
        "split tabs are deliberate; pinned"
    );
}

#[test]
fn open_new_distinct_view_modes_independent() {
    // Two splits of the same doc, each holds its own view_mode.
    let mut tabs = ModelTabs::default();
    let a = tabs.open_new(doc(1), None);
    let b = tabs.open_new(doc(1), None);
    tabs.get_mut(a).unwrap().view_mode = ModelViewMode::Text;
    tabs.get_mut(b).unwrap().view_mode = ModelViewMode::Canvas;
    assert!(matches!(
        tabs.get(a).unwrap().view_mode,
        ModelViewMode::Text
    ));
    assert!(matches!(
        tabs.get(b).unwrap().view_mode,
        ModelViewMode::Canvas
    ));
}

// ─────────────────────────────────────────────────────────────────────
// close_tab vs close_all_for_doc
// ─────────────────────────────────────────────────────────────────────

#[test]
fn close_tab_drops_only_that_tab() {
    let mut tabs = ModelTabs::default();
    let a = tabs.ensure_for(doc(1), None);
    let b = tabs.open_new(doc(1), None);
    let dropped = tabs.close_tab(a);
    assert!(dropped.is_some(), "close_tab returns Some on hit");
    assert!(tabs.get(a).is_none());
    assert!(tabs.get(b).is_some(), "sibling tab survives");
    assert_eq!(tabs.count_for_doc(doc(1)), 1);
}

#[test]
fn close_tab_unknown_returns_none() {
    let mut tabs = ModelTabs::default();
    let dropped = tabs.close_tab(9999);
    assert!(dropped.is_none());
}

#[test]
fn close_all_for_doc_drops_every_view_of_doc() {
    let mut tabs = ModelTabs::default();
    let _a = tabs.ensure_for(doc(1), None);
    let _b = tabs.open_new(doc(1), Some("Inner".into()));
    let c = tabs.ensure_for(doc(2), None);
    let dropped_ids = tabs.close_all_for_doc(doc(1));
    assert_eq!(dropped_ids.len(), 2, "both doc(1) tabs dropped");
    assert_eq!(tabs.count_for_doc(doc(1)), 0);
    assert!(tabs.get(c).is_some(), "doc(2) tab survives");
}

// ─────────────────────────────────────────────────────────────────────
// Sibling-tab behaviour — the desync class A.5 / B.1 fixed
// ─────────────────────────────────────────────────────────────────────

#[test]
fn sibling_tabs_have_distinct_ids_but_same_doc() {
    let mut tabs = ModelTabs::default();
    let a = tabs.open_new(doc(1), None);
    let b = tabs.open_new(doc(1), None);
    assert_ne!(a, b);
    let s_a = tabs.get(a).unwrap();
    let s_b = tabs.get(b).unwrap();
    assert_eq!(s_a.doc, s_b.doc, "siblings share a doc");
    assert_eq!(s_a.drilled_class, s_b.drilled_class);
}

#[test]
fn iter_docs_dedups_siblings() {
    let mut tabs = ModelTabs::default();
    let _a = tabs.open_new(doc(1), None);
    let _b = tabs.open_new(doc(1), None);
    let _c = tabs.ensure_for(doc(2), None);
    let docs: std::collections::HashSet<_> = tabs.iter_docs().collect();
    assert_eq!(docs.len(), 2, "distinct docs only, sibling collapsed");
    assert!(docs.contains(&doc(1)));
    assert!(docs.contains(&doc(2)));
}

// ─────────────────────────────────────────────────────────────────────
// Allocation determinism — ids are monotonic
// ─────────────────────────────────────────────────────────────────────

#[test]
fn allocated_tab_ids_are_monotonic() {
    let mut tabs = ModelTabs::default();
    let a = tabs.ensure_for(doc(1), None);
    let b = tabs.ensure_for(doc(2), None);
    let c = tabs.open_new(doc(3), None);
    assert!(a < b && b < c, "ids must allocate strictly increasing");
}

// ─────────────────────────────────────────────────────────────────────
// drilled_class_for_doc — B.3 helper that replaces
// `DrilledInClassNames.get(doc)` reads. Pin the lookup semantics so
// the migration's reader rewrites stay safe.
// ─────────────────────────────────────────────────────────────────────

#[test]
fn drilled_class_for_doc_returns_none_when_no_tab() {
    let tabs = ModelTabs::default();
    assert_eq!(tabs.drilled_class_for_doc(doc(1)), None);
}

#[test]
fn drilled_class_for_doc_returns_no_drill_for_root_tab() {
    let mut tabs = ModelTabs::default();
    let _id = tabs.ensure_for(doc(1), None);
    assert_eq!(tabs.drilled_class_for_doc(doc(1)), None, "root tab → None");
}

#[test]
fn drilled_class_for_doc_returns_scope_for_drilled_tab() {
    let mut tabs = ModelTabs::default();
    let _id = tabs.ensure_for(doc(1), Some("Foo.Bar".into()));
    assert_eq!(tabs.drilled_class_for_doc(doc(1)), Some("Foo.Bar".into()),);
}

#[test]
fn drilled_class_for_doc_unknown_doc_is_none() {
    let mut tabs = ModelTabs::default();
    let _id = tabs.ensure_for(doc(1), Some("Foo".into()));
    assert_eq!(tabs.drilled_class_for_doc(doc(9)), None);
}

// ─────────────────────────────────────────────────────────────────────
// close_drilled_into — cross-truth rule R4 (RemoveClass closes
// drilled tabs). Helper-level pin; observer wiring is a separate
// chokepoint.
// ─────────────────────────────────────────────────────────────────────

#[test]
fn close_drilled_into_drops_exact_match() {
    let mut tabs = ModelTabs::default();
    let drilled = tabs.ensure_for(doc(1), Some("Foo.Bar".into()));
    let root = tabs.ensure_for(doc(1), None);
    let closed = tabs.close_drilled_into(doc(1), "Foo.Bar");
    assert_eq!(closed, vec![drilled]);
    assert!(tabs.get(drilled).is_none(), "drilled tab dropped");
    assert!(
        tabs.get(root).is_some(),
        "root tab survives — different scope"
    );
}

#[test]
fn close_drilled_into_drops_descendants() {
    // Removing `Foo.Bar` must also close tabs drilled into
    // `Foo.Bar.Baz`, `Foo.Bar.Other.Inner`, etc.
    let mut tabs = ModelTabs::default();
    let _t1 = tabs.ensure_for(doc(1), Some("Foo.Bar".into()));
    let _t2 = tabs.ensure_for(doc(1), Some("Foo.Bar.Baz".into()));
    let _t3 = tabs.ensure_for(doc(1), Some("Foo.Bar.Other.Inner".into()));
    let sibling = tabs.ensure_for(doc(1), Some("Foo.BarSibling".into()));
    let closed = tabs.close_drilled_into(doc(1), "Foo.Bar");
    assert_eq!(closed.len(), 3, "3 tabs match (exact + 2 descendants)");
    assert!(
        tabs.get(sibling).is_some(),
        "Foo.BarSibling NOT a descendant"
    );
}

#[test]
fn close_drilled_into_scoped_to_doc() {
    // Same drilled path in different docs — only the matching doc's
    // tab closes.
    let mut tabs = ModelTabs::default();
    let in_a = tabs.ensure_for(doc(1), Some("Foo.Bar".into()));
    let in_b = tabs.ensure_for(doc(2), Some("Foo.Bar".into()));
    let closed = tabs.close_drilled_into(doc(1), "Foo.Bar");
    assert_eq!(closed, vec![in_a]);
    assert!(tabs.get(in_b).is_some(), "doc(2)'s drilled tab survives");
}

#[test]
fn close_drilled_into_ignores_no_drill() {
    let mut tabs = ModelTabs::default();
    let root = tabs.ensure_for(doc(1), None);
    let closed = tabs.close_drilled_into(doc(1), "Foo.Bar");
    assert!(closed.is_empty());
    assert!(tabs.get(root).is_some(), "no-drill tab unaffected");
}

#[test]
fn close_drilled_into_empty_qualified_is_noop() {
    // Defensive: an empty qualified path would otherwise match
    // every drilled tab (every string starts with empty). Reject.
    let mut tabs = ModelTabs::default();
    let _t = tabs.ensure_for(doc(1), Some("Foo".into()));
    let closed = tabs.close_drilled_into(doc(1), "");
    assert!(closed.is_empty(), "empty path must be a no-op");
}

#[test]
fn closing_then_reallocating_does_not_reuse_id() {
    // Even after a tab is closed, its id is never reused — closed tab
    // ids go to the void. Prevents a stale reference (e.g. cached
    // canvas state keyed by closed-tab-id) from accidentally aliasing
    // a later tab.
    let mut tabs = ModelTabs::default();
    let a = tabs.ensure_for(doc(1), None);
    tabs.close_tab(a);
    let b = tabs.ensure_for(doc(2), None);
    assert!(
        b > a,
        "fresh allocation is strictly newer than the closed id"
    );
}
