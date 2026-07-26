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

/// API readiness probe. Answers as soon as the command core is up, on every
/// build — windowed, headless, or wasm.
#[Command(default)]
pub struct Ping {}

#[on_command(Ping)]
pub fn on_ping(_trigger: On<Ping>) {
    // Intentional no-op: resolving IS the answer.
}

register_commands!(on_ping);
