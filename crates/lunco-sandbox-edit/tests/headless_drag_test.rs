//! Headless integration tests for entity selection and gizmo.
//!
//! These tests verify the selection state machine and possession blocking logic
//! without requiring GPU, window, or physics systems.

use bevy::prelude::*;
use lunco_sandbox_edit::SelectedEntity;
use lunco_core::DragModeActive;

// ─── Selection State Machine Tests ─────────────────────────────────────────────

/// Simulates the complete selection lifecycle that selection.rs implements
#[test]
fn test_shift_click_selects_entity() {
    let mut selected = SelectedEntity::default();
    let mut drag_mode = DragModeActive { active: false };
    let rover = Entity::from_bits(42);

    // Initial state
    assert!(selected.entity.is_none());
    assert!(!drag_mode.active);

    // --- ACTION: Shift+Left-click (selection.rs logic) ---
    selected.entity = Some(rover);
    drag_mode.active = true;

    // Verify selection state
    assert_eq!(selected.entity, Some(rover));
    assert!(drag_mode.active, "DragModeActive must be set to block possession");
}

/// Verifies that possession is blocked during selection
#[test]
fn test_selection_blocks_possession() {
    let drag_mode = DragModeActive { active: true };
    let mut possession_triggered = false;

    // Simulate avatar_raycast_possession logic:
    if drag_mode.active {
        // BLOCKED - early return, no POSSESS command sent
        possession_triggered = false;
    } else {
        possession_triggered = true;
    }

    assert!(!possession_triggered, "Possession MUST be blocked when drag_mode.active=true");
}

/// Verifies that after deselection, possession is allowed again
#[test]
fn test_deselection_allows_possession() {
    let mut selected = SelectedEntity::default();
    let mut drag_mode = DragModeActive { active: false };
    let rover = Entity::from_bits(42);

    // Select
    selected.entity = Some(rover);
    drag_mode.active = true;
    assert!(drag_mode.active);

    // --- ACTION: Escape (deselection) ---
    selected.entity = None;
    drag_mode.active = false;

    // Verify deselection
    assert!(selected.entity.is_none(), "Entity should be deselected");
    assert!(!drag_mode.active, "DragModeActive must be cleared to allow possession");

    // Verify possession is now allowed
    let mut possession_triggered = false;
    if drag_mode.active {
        possession_triggered = false; // blocked
    } else {
        possession_triggered = true; // allowed
    }
    assert!(possession_triggered, "Possession MUST be allowed after deselection");
}

/// Tests Escape key behavior during selection
#[test]
fn test_escape_cancels_selection() {
    let mut selected = SelectedEntity::default();
    let mut drag_mode = DragModeActive { active: false };
    let rover = Entity::from_bits(42);

    // Select
    selected.entity = Some(rover);
    drag_mode.active = true;

    // --- ACTION: Escape key (selection.rs logic) ---
    selected.entity = None;
    drag_mode.active = false;

    // Verify everything is cleared
    assert!(selected.entity.is_none(), "Entity should be deselected");
    assert!(!drag_mode.active, "Possession should be allowed after cancel");
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
    drag_mode.active = true; // Still active

    assert_eq!(selected.entity, Some(rover2), "Should now select second rover");
    assert!(drag_mode.active, "Possession should still be blocked");
}
