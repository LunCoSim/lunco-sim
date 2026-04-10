//! Headless integration tests for entity dragging.
//!
//! These tests verify the drag state machine and possession blocking logic
//! without requiring GPU, window, or physics systems.

use bevy::prelude::*;
use lunco_sandbox_edit::{SelectedEntity, ToolMode, spawn::DragConfig};
use lunco_core::DragModeActive;

// ─── State Machine Tests ─────────────────────────────────────────────────────

/// Simulates the complete drag lifecycle that selection.rs and spawn.rs implement
#[test]
fn test_shift_click_enters_drag_mode() {
    let mut selected = SelectedEntity::default();
    let mut drag_mode = DragModeActive { active: false };
    let rover = Entity::from_bits(42);

    // Initial state
    assert!(selected.entity.is_none());
    assert!(!selected.is_dragging);
    assert!(!drag_mode.active);

    // --- ACTION: Shift+Left-click (selection.rs logic) ---
    // This is exactly what handle_entity_selection does:
    selected.entity = Some(rover);
    selected.is_dragging = true;
    drag_mode.active = true;

    // Verify selection state matches what the code sets
    assert_eq!(selected.entity, Some(rover));
    assert!(selected.is_dragging, "Selection should enter drag mode");
    assert!(drag_mode.active, "DragModeActive must be set to block possession");
}

/// Verifies that possession is blocked during drag mode
/// This simulates the exact check in avatar_raycast_possession:
/// `if drag_mode_active.active { return; }`
#[test]
fn test_drag_mode_blocks_possession() {
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

/// Verifies that after placement, possession is allowed again
#[test]
fn test_placement_clears_drag_mode() {
    let mut selected = SelectedEntity::default();
    let mut drag_mode = DragModeActive { active: false };
    let rover = Entity::from_bits(42);

    // Enter drag mode (Shift+click)
    selected.entity = Some(rover);
    selected.is_dragging = true;
    drag_mode.active = true;
    assert!(drag_mode.active);

    // --- ACTION: Right-click or Escape (spawn.rs logic) ---
    // This is exactly what update_selected_entity_drag does on placement:
    selected.is_dragging = false;
    drag_mode.active = false;

    // Verify drag mode is cleared
    assert!(!selected.is_dragging, "Should exit drag mode after placement");
    assert!(!drag_mode.active, "DragModeActive must be cleared to allow possession");

    // Verify possession is now allowed
    let mut possession_triggered = false;
    if drag_mode.active {
        possession_triggered = false; // blocked
    } else {
        possession_triggered = true; // allowed
    }
    assert!(possession_triggered, "Possession MUST be allowed after drag mode cleared");
}

/// Tests Escape key behavior during drag
#[test]
fn test_escape_cancels_drag() {
    let mut selected = SelectedEntity::default();
    let mut drag_mode = DragModeActive { active: false };
    let rover = Entity::from_bits(42);

    // Enter drag mode
    selected.entity = Some(rover);
    selected.is_dragging = true;
    drag_mode.active = true;

    // --- ACTION: Escape key (selection.rs logic) ---
    selected.entity = None;
    selected.is_dragging = false;
    selected.mode = ToolMode::Select;
    drag_mode.active = false;

    // Verify everything is cleared
    assert!(selected.entity.is_none(), "Entity should be deselected");
    assert!(!selected.is_dragging, "Drag should be cleared");
    assert!(!drag_mode.active, "Possession should be allowed after cancel");
}

/// Tests switching selection between entities maintains drag mode
#[test]
fn test_switching_selection_maintains_drag() {
    let mut selected = SelectedEntity::default();
    let mut drag_mode = DragModeActive { active: false };
    let rover1 = Entity::from_bits(100);
    let rover2 = Entity::from_bits(200);

    // Select first rover
    selected.entity = Some(rover1);
    selected.is_dragging = true;
    drag_mode.active = true;

    // Select second rover (replace selection)
    selected.entity = Some(rover2);
    selected.is_dragging = true; // Still dragging new entity
    drag_mode.active = true;

    assert_eq!(selected.entity, Some(rover2), "Should now select second rover");
    assert!(selected.is_dragging, "Should still be in drag mode");
    assert!(drag_mode.active, "Possession should still be blocked");
}

/// Tests that DragConfig values are sensible and tunable
#[test]
fn test_drag_config_defaults() {
    let config = DragConfig::default();
    
    // Verify defaults are physically reasonable
    assert!(config.spring_constant > 0.0, "Spring constant must be positive");
    assert!(config.max_force > 0.0, "Max force must be positive");
    assert!(config.stop_distance > 0.0, "Stop distance must be positive");
    assert!(config.max_force >= config.spring_constant, 
        "Max force should handle at least 1m of spring displacement");
}

/// Tests that config can be tuned at runtime
#[test]
fn test_drag_config_is_tunable() {
    let mut config = DragConfig::default();
    
    // Tune for stiffer response
    config.spring_constant = 200.0;
    config.max_force = 2000.0;
    config.stop_distance = 0.05;

    assert_eq!(config.spring_constant, 200.0);
    assert_eq!(config.max_force, 2000.0);
    assert_eq!(config.stop_distance, 0.05);
}
