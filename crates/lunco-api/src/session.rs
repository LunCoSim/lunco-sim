//! Session commands that must exist on **every** binary that can be called.
//!
//! WHY HERE. `Ping` used to live in `lunco-modelica::ui::commands::util` and was
//! registered by `ModelicaCommandsPlugin` — a plugin a headless host never adds.
//! So a `--no-ui` server answered its own readiness probe with
//! `Command 'Ping' not found or not API-accessible`, which is the one answer a
//! readiness probe must never give: indistinguishable from "wrong name" and from
//! "not up yet". A probe of the API belongs to the API, and registering it with
//! the command core makes "the API is reachable" and "`Ping` resolves" the same
//! fact rather than two that can drift.

use bevy::prelude::*;
use lunco_core::{on_command, register_commands, Command};

/// Shut down the application.
///
/// `force = true`: exit immediately. The reliable path for automation.
///
/// `force = false`: close the way a user would — route through the interactive
/// dirty-document save prompt, which a windowed host installs an observer for
/// (`lunco_modelica::ui::commands::util`). **On a host with no window there is
/// nobody to answer that prompt**, so this exits directly rather than waiting
/// forever for a modal that will never be drawn.
///
/// WHY HERE. The whole command used to live behind the Modelica `ui` feature,
/// which meant the one caller who cannot fall back to closing a window — a
/// headless server — was the one caller it did not exist for. Shutting down is a
/// session concern, so it lives with the session; hosts that have extra work to
/// do on the way out (cancel in-flight compiles, prompt to save) observe the
/// same command and do their part.
#[Command(default)]
pub struct Exit {
    /// Skip the interactive save prompt and exit immediately.
    pub force: bool,
}

#[on_command(Exit)]
pub fn on_exit(
    trigger: On<Exit>,
    windows: Query<(), With<Window>>,
    mut exit: MessageWriter<bevy::app::AppExit>,
) {
    if trigger.event().force {
        info!("[Exit] force — exiting immediately (no save prompt)");
    } else if windows.is_empty() {
        info!("[Exit] no window — nobody to answer a save prompt, exiting");
    } else {
        // A windowed host owns this path: it prompts, then exits itself.
        info!("[Exit] requested — routing through the app-close flow");
        return;
    }
    exit.write(bevy::app::AppExit::Success);
}

/// API readiness probe. Answers as soon as the command core is up, on every
/// build — windowed, headless, or wasm.
#[Command(default)]
pub struct Ping {}

#[on_command(Ping)]
pub fn on_ping(_trigger: On<Ping>) {
    // Intentional no-op: resolving IS the answer.
}

register_commands!(on_ping, on_exit);
