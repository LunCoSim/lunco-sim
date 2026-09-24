//! Session commands that must exist on **every** binary that can be called.
//!
//! WHY HERE. Which commands exist is decided by which PLUGINS a host adds, so a
//! command registered from a UI plugin does not exist on a windowless one — and
//! the executor reports that exactly as it reports a typo. These belong to the
//! API and the session, not to any UI, so they register with the command CORE:
//! "the API is reachable" and "`Ping` resolves" are then one fact, not two that
//! can drift.

use bevy::prelude::*;
use lunco_core::{Command, on_command, register_commands};

/// Records that an application-level interactive close-flow adapter owns the
/// non-forced exit path. The adapter sets this when it registers its observer;
/// the API session handler otherwise remains the headless exit owner.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct InteractiveExitHandler {
    /// Whether a UI/application adapter will consume non-forced exits.
    pub installed: bool,
}

/// Shut down the application.
///
/// `force = true`: exit immediately. The reliable path for automation.
///
/// `force = false`: close the way a user would — route through the interactive
/// dirty-document save prompt, which a windowed host installs an observer for
/// (`lunco_modelica_ui::ui::commands::util`). **On a host with no window there is
/// nobody to answer that prompt**, so this exits directly rather than waiting
/// forever for a modal that will never be drawn.
///
/// Shutting down is a session concern, so it lives with the session and exists
/// on every binary — a windowless host cannot fall back to closing a window.
/// Hosts with extra work to do on the way out (cancel in-flight compiles, prompt
/// to save) observe the same command and do their part.
#[Command(default)]
pub struct Exit {
    /// Skip the interactive save prompt and exit immediately.
    pub force: bool,
}

#[on_command(Exit)]
pub fn on_exit(
    trigger: On<Exit>,
    handler: Option<Res<InteractiveExitHandler>>,
    mut exit: MessageWriter<bevy::app::AppExit>,
) {
    if trigger.event().force {
        info!("[Exit] force — exiting immediately (no save prompt)");
    } else if handler.is_some_and(|handler| handler.installed) {
        // An application adapter owns the interactive close flow and will
        // either prompt or emit AppExit after its policy settles.
        info!("[Exit] requested — routing through the app-close flow");
        return;
    } else {
        info!("[Exit] no interactive close handler — exiting");
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
