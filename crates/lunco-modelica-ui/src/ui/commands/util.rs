//! Modelica's part of shutting down.
//!
//! The `Exit` COMMAND lives in `lunco_api::session` — shutting down is a session
//! concern and must exist on every binary, including one built without this
//! crate's `ui` feature. What is here is genuinely Modelica's: cancelling
//! in-flight compiles and — when there is a human at a window — the
//! dirty-document save prompt.
//!
//! Registered by `ModelicaCorePlugin`, so a headless host still gets the
//! cancellation path; the prompt half is skipped for want of a window.

use bevy::prelude::*;
use lunco_api::session::Exit;
use lunco_core::{on_command, register_commands};

#[on_command(Exit)]
pub fn on_exit(trigger: On<Exit>, windows: Query<(), With<Window>>, mut commands: Commands) {
    // Signal in-flight runs before every non-interactive exit path.
    if trigger.event().force || windows.is_empty() {
        commands.queue(|world: &mut World| {
            crate::ui::commands::lifecycle::cancel_inflight_runs(world);
        });
        return;
    }
    // Interactive close: route through the dirty-document save-prompt flow, same
    // as the window-X button. Cancellation happens after the human resolves the
    // save prompt at the actual-exit commit point.
    commands.queue(|world: &mut World| {
        crate::ui::commands::lifecycle::request_app_close(world);
    });
}

register_commands!(on_exit);
