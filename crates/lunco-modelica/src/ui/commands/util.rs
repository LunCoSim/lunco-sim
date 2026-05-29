//! Miscellaneous utility commands: Ping and Exit.

use bevy::prelude::*;
use lunco_core::{Command, on_command};

// ─── Command Structs ─────────────────────────────────────────────────────────

/// API readiness probe.
#[Command(default)]
pub struct Ping {}

/// Gracefully shut down the application.
#[Command(default)]
pub struct Exit {}

// ─── Observers ───────────────────────────────────────────────────────────────

#[on_command(Ping)]
pub fn on_ping(_cmd: Ping) {
    // Intentional no-op.
}

#[on_command(Exit)]
pub fn on_exit(_trigger: On<Exit>, mut commands: Commands) {
    bevy::log::info!("[Exit] requested — routing through app-close flow");
    commands.queue(|world: &mut World| {
        // The API `Exit` command is a programmatic "really shut down" (unlike
        // the window-X path, which is interactive and should keep its
        // save-prompt). Arm the hard-exit watchdog up front so a dirty-doc
        // save-prompt with no GUI to answer it — or a runaway compute thread
        // that blocks the graceful join — can no longer wedge the process.
        // Idempotent (Once-guarded); the close flow still runs and exits
        // cleanly first in the common case, well within the grace period.
        crate::ui::commands::lifecycle::cancel_inflight_runs(world);
        crate::ui::commands::lifecycle::arm_shutdown_watchdog();
        crate::ui::commands::lifecycle::request_app_close(world);
    });
}
