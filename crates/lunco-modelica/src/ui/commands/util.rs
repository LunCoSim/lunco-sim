//! Modelica's part of shutting down.
//!
//! The `Exit` COMMAND lives in `lunco_api::session` — shutting down is a session
//! concern and must exist on every binary, including one built without this
//! crate's `ui` feature. What is here is genuinely Modelica's: cancelling
//! in-flight compiles, arming the wedge watchdog, and — when there is a human at
//! a window — the dirty-document save prompt.
//!
//! Registered by `ModelicaCorePlugin`, so a headless host still gets the
//! cancel/watchdog half; the prompt half is skipped for want of a window.

use bevy::prelude::*;
use lunco_api::session::Exit;
use lunco_core::{on_command, register_commands};

#[on_command(Exit)]
pub fn on_exit(
    trigger: On<Exit>,
    windows: Query<(), With<Window>>,
    mut commands: Commands,
) {
    // Whether or not this process is about to exit, a compile that never yields
    // can block Bevy's TaskPool join and wedge the shutdown. Signal in-flight
    // runs to cancel and arm the hard-exit watchdog on every path that exits.
    if trigger.event().force || windows.is_empty() {
        crate::ui::commands::lifecycle::arm_shutdown_watchdog();
        commands.queue(|world: &mut World| {
            crate::ui::commands::lifecycle::cancel_inflight_runs(world);
        });
        return;
    }
    // Interactive close: route through the dirty-document save-prompt flow, same
    // as the window-X button. Do NOT arm the watchdog here — `request_app_close`
    // arms it itself at the actual-exit commit points, after the human answers.
    commands.queue(|world: &mut World| {
        crate::ui::commands::lifecycle::request_app_close(world);
    });
}

register_commands!(on_exit);
