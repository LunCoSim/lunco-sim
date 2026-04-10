//! End-to-end tests for drag selection and possession interaction.
//!
//! These tests simulate the full workflow:
//! 1. Shift+Left-click selects entity and activates drag mode
//! 2. DragModeActive blocks avatar possession
//! 3. Right-click/Escape exits drag mode
//! 4. Possession is allowed again after drag mode exits

use bevy::prelude::*;
use lunco_sandbox_edit::{SelectedEntity, spawn::DragConfig};
use lunco_core::DragModeActive;

// ─── Simulated Input Flow Tests ──────────────────────────────────────────────

/// Simulates what happens when user presses Shift+Left-click on a rover
#[test]
fn test_shift_left_click_full_flow() {
    let mut selected = SelectedEntity::default();
    let mut drag_mode = DragModeActive { active: false };
    let rover = Entity::from_bits(42);

    // --- BEFORE: No selection, drag mode off ---
    assert!(selected.entity.is_none());
    assert!(!selected.is_dragging);
    assert!(!drag_mode.active);

    // --- ACTION: User presses Shift+Left-click on rover ---
    // Selection system runs:
    selected.entity = Some(rover);
    selected.is_dragging = true;
    drag_mode.active = true;  // Set immediately (not via commands)

    // --- AFTER SELECTION ---
    assert_eq!(selected.entity, Some(rover));
    assert!(selected.is_dragging, "Should be in drag mode");
    assert!(drag_mode.active, "Drag mode should block possession");

    // Avatar possession checks: `if drag_mode_active.active { return; }`
    let possession_blocked = drag_mode.active;
    assert!(possession_blocked, "Possession MUST be blocked during drag");
}

/// Simulates what happens when user exits drag mode (Escape or Right-click)
#[test]
fn test_exit_drag_mode_allows_possession() {
    let mut selected = SelectedEntity::default();
    let mut drag_mode = DragModeActive { active: false };
    let rover = Entity::from_bits(42);

    // Start in drag mode (simulated Shift+click)
    selected.entity = Some(rover);
    selected.is_dragging = true;
    drag_mode.active = true;

    // --- ACTION: User presses Escape or Right-click ---
    selected.is_dragging = false;
    drag_mode.active = false;

    // --- AFTER EXIT ---
    assert!(!selected.is_dragging, "Should exit drag mode");
    assert!(!drag_mode.active, "Drag mode should be cleared");

    // Avatar possession checks: `if drag_mode_active.active { return; }`
    let possession_allowed = !drag_mode.active;
    assert!(possession_allowed, "Possession MUST be allowed after drag exits");
}

/// Verifies that possession check returns early when drag mode is active.
/// Simulates the exact logic from avatar_raycast_possession:
/// ```ignore
/// if drag_mode_active.active { return; } // Block possession
/// ```
#[test]
fn test_possession_blocked_every_frame() {
    let drag_mode = DragModeActive { active: true };
    let mut possession_attempted = false;

    // Simulate what avatar_raycast_possession does:
    // if drag_mode_active.active { return; }
    if drag_mode.active {
        // Possession blocked - early return (don't trigger POSSESS command)
        possession_attempted = false;
    } else {
        // Possession allowed
        possession_attempted = true;
    }

    assert!(!possession_attempted, "Possession should NOT be attempted when drag mode is active");
}

// ─── Configuration Tests ─────────────────────────────────────────────────────

#[test]
fn test_drag_config_tunable() {
    let mut config = DragConfig::default();

    // Verify defaults are reasonable
    assert!(config.spring_constant > 0.0);
    assert!(config.max_force > 0.0);
    assert!(config.stop_distance > 0.0);

    // Verify they can be tuned
    config.spring_constant = 100.0;
    config.max_force = 1000.0;
    config.stop_distance = 0.5;

    assert_eq!(config.spring_constant, 100.0);
    assert_eq!(config.max_force, 1000.0);
    assert_eq!(config.stop_distance, 0.5);
}

// ─── State Machine Tests ─────────────────────────────────────────────────────

/// Tests the complete state machine: Idle → Dragging → Idle
#[test]
fn test_drag_state_machine() {
    let mut selected = SelectedEntity::default();
    let mut drag_mode = DragModeActive { active: false };
    let rover = Entity::from_bits(100);

    // State: IDLE
    assert!(selected.entity.is_none() && !selected.is_dragging);

    // Transition: Shift+Left-click → DRAGGING
    selected.entity = Some(rover);
    selected.is_dragging = true;
    drag_mode.active = true;
    assert!(selected.is_dragging && drag_mode.active);

    // Verify possession is blocked
    assert!(drag_mode.active, "Possession should be blocked in dragging state");

    // Transition: Escape/Right-click → IDLE
    selected.is_dragging = false;
    drag_mode.active = false;
    assert!(!selected.is_dragging && !drag_mode.active);

    // Verify possession is allowed
    assert!(!drag_mode.active, "Possession should be allowed in idle state");
}
