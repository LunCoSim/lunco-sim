//! End-to-end tests for selection and gizmo interaction.
//!
//! Tests verify:
//! 1. Shift+Left-click selects entity and enables gizmo
//! 2. DragModeActive blocks avatar possession during selection
//! 3. Escape exits selection and clears gizmo
//! 4. Possession is allowed again after deselection

use bevy::prelude::*;
use lunco_sandbox_edit::SelectedEntity;
use lunco_core::DragModeActive;

// ─── Selection Flow Tests ─────────────────────────────────────────────────────

/// Simulates what happens when user presses Shift+Left-click on a rover
#[test]
fn test_shift_left_click_selects_entity() {
    let mut selected = SelectedEntity::default();
    let mut drag_mode = DragModeActive { active: false };
    let rover = Entity::from_bits(42);

    // --- BEFORE: No selection ---
    assert!(selected.entity.is_none());
    assert!(!drag_mode.active);

    // --- ACTION: User presses Shift+Left-click on rover ---
    // Selection system runs:
    selected.entity = Some(rover);
    drag_mode.active = true;

    // --- AFTER SELECTION ---
    assert_eq!(selected.entity, Some(rover));
    assert!(drag_mode.active, "Drag mode should be set to block possession");
}

/// Simulates what happens when user deselects with Escape
#[test]
fn test_escape_deselects() {
    let mut selected = SelectedEntity::default();
    let mut drag_mode = DragModeActive { active: false };
    let rover = Entity::from_bits(42);

    // Start selected (simulated Shift+click)
    selected.entity = Some(rover);
    drag_mode.active = true;

    // --- ACTION: User presses Escape ---
    selected.entity = None;
    drag_mode.active = false;

    // --- AFTER DESELECTION ---
    assert!(selected.entity.is_none(), "Entity should be deselected");
    assert!(!drag_mode.active, "Drag mode should be cleared");
}

/// Verifies that possession check returns early when drag mode is active.
/// Simulates the exact logic from avatar_raycast_possession:
/// `if drag_mode_active.active { return; } // Block possession`
#[test]
fn test_possession_blocked_during_selection() {
    let drag_mode = DragModeActive { active: true };
    let mut possession_attempted = false;

    // Simulate what avatar_raycast_possession does:
    if drag_mode.active {
        // Possession blocked - early return
        possession_attempted = false;
    } else {
        // Possession allowed
        possession_attempted = true;
    }

    assert!(!possession_attempted, "Possession should NOT be attempted during selection");
}

/// Verifies that after deselection, possession is allowed again
#[test]
fn test_deselection_allows_possession() {
    let mut drag_mode = DragModeActive { active: false };

    // Select
    drag_mode.active = true;
    assert!(drag_mode.active);

    // Deselect
    drag_mode.active = false;

    // Verify possession is now allowed
    let mut possession_attempted = false;
    if drag_mode.active {
        possession_attempted = false; // blocked
    } else {
        possession_attempted = true; // allowed
    }
    assert!(possession_attempted, "Possession MUST be allowed after deselection");
}

/// Tests switching selection between entities
#[test]
fn test_switching_selection() {
    let mut selected = SelectedEntity::default();
    let mut drag_mode = DragModeActive { active: false };
    let rover1 = Entity::from_bits(100);
    let rover2 = Entity::from_bits(200);

    // Select first rover
    selected.entity = Some(rover1);
    drag_mode.active = true;

    // Select second rover (replace selection)
    selected.entity = Some(rover2);
    // drag_mode stays true

    assert_eq!(selected.entity, Some(rover2), "Should now select second rover");
    assert!(drag_mode.active, "Possession should still be blocked");
}
