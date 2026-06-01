//! Miscellaneous utility commands: Ping and Exit.

use bevy::prelude::*;
use lunco_core::{Command, on_command};

// ─── Command Structs ─────────────────────────────────────────────────────────

/// API readiness probe.
#[Command(default)]
pub struct Ping {}

/// Shut down the application.
///
/// `force = false` (default): route through the interactive app-close flow,
/// which prompts to save dirty documents. **Over the API this can hang** — the
/// Save/Don't-save/Cancel modal needs a human at the window.
///
/// `force = true`: close in any way — fire `AppExit` immediately, bypassing the
/// save prompt. This is the reliable path for automation / headless control.
#[Command(default)]
pub struct Exit {
    pub force: bool,
}

// ─── Observers ───────────────────────────────────────────────────────────────

#[on_command(Ping)]
pub fn on_ping(_cmd: Ping) {
    // Intentional no-op.
}

#[on_command(Exit)]
pub fn on_exit(
    trigger: On<Exit>,
    mut commands: Commands,
    mut exit: MessageWriter<bevy::app::AppExit>,
) {
    if trigger.event().force {
        // Close in any way: fire AppExit immediately, bypassing the save prompt.
        // The sandbox auto-compiles Modelica models that are marked unsaved, so
        // the prompt flow (below) would otherwise hang over the API — the
        // Save/Don't-save/Cancel modal needs a human at the window.
        bevy::log::info!("[Exit] force — firing AppExit immediately (no save prompt)");
        exit.write(bevy::app::AppExit::Success);
    } else {
        // Interactive close: route through the dirty-document save-prompt flow,
        // same as the window-X button.
        bevy::log::info!("[Exit] requested — routing through app-close flow");
        commands.queue(|world: &mut World| {
            crate::ui::commands::lifecycle::request_app_close(world);
        });
    }
}
