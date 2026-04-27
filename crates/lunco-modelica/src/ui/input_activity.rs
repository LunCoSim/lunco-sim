//! Idle / activity tracker.
//!
//! The workbench's CPU-expensive background work (rumoca AST parses,
//! MSL extends-chain prewarm) is **gated on user activity**: while the
//! user is actively interacting (mouse moves, clicks, keystrokes,
//! editor ops, canvas drags), no rumoca parse is allowed to spawn.
//! After 500 ms of true idle, queued parses run. This is the load-
//! bearing piece of the "lazy-parse + instant feedback" architecture
//! — it broke the 1.4-2 s `Last`-schedule freeze that otherwise
//! happened on every Add / Move (rumoca parses were correlating 1:1
//! with multi-second Bevy render stalls, even off-thread, even on
//! capped rayon pools).
//!
//! See `crate::ui::ast_refresh` for the gated parse spawn site.

use bevy::prelude::*;
use bevy_egui::EguiContexts;

/// Idle-window threshold. Below this many milliseconds since the
/// last user event, no background rumoca parse is allowed to spawn.
/// 500 ms is short enough to feel "responsive" (parses fire mid-
/// pause when the user is reading the result of an edit) and long
/// enough to absorb the user's natural finger pauses between
/// successive Adds / drags.
pub const IDLE_THRESHOLD_MS: u128 = 500;

/// Wall-clock timestamp of the user's most recent interaction.
/// Stamped by [`stamp_user_input`] (egui input observer) and by
/// every Modelica edit site (`apply_ops`, document mutations).
#[derive(Resource, Default)]
pub struct InputActivity {
    last_event_at: Option<web_time::Instant>,
}

impl InputActivity {
    /// Stamp `now` as the latest user activity.
    pub fn stamp(&mut self) {
        self.last_event_at = Some(web_time::Instant::now());
    }

    /// True when the most recent stamp is within
    /// [`IDLE_THRESHOLD_MS`]. Background parse spawn sites should
    /// short-circuit when this returns true — they'll get another
    /// chance next Update tick.
    pub fn is_active(&self) -> bool {
        self.last_event_at
            .map(|t| t.elapsed().as_millis() < IDLE_THRESHOLD_MS)
            .unwrap_or(false)
    }

    /// Convenience: how long since the last event, for telemetry.
    pub fn idle_for(&self) -> Option<std::time::Duration> {
        self.last_event_at.map(|t| t.elapsed())
    }
}

/// PreUpdate system: peek at every egui context's input this frame.
/// If any pointer movement, click, scroll, or keypress happened,
/// stamp the activity tracker. Cheap — egui keeps the input state
/// in `Memory`; we just read flags, no buffering.
pub fn stamp_user_input(
    mut activity: ResMut<InputActivity>,
    mut contexts: EguiContexts,
) {
    // `EguiContexts::ctx_mut()` returns the primary context; we only
    // need one for input detection (workbench has a single window).
    let Ok(ctx) = contexts.ctx_mut() else { return };
    let active = ctx.input(|i| {
        // Pointer motion is the dominant signal during drag.
        // `pointer.is_moving()` returns true any frame the cursor
        // changed position. `pointer.any_pressed()` covers clicks.
        // `events` is a thin Vec; checking non-empty is constant
        // time. Keys covers typing in the code editor.
        i.pointer.is_moving()
            || i.pointer.any_pressed()
            || i.pointer.any_released()
            || i.pointer.any_down()
            || !i.events.is_empty()
            || !i.keys_down.is_empty()
    });
    if active {
        activity.stamp();
    }
}
