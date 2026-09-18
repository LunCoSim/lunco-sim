//! Generic runtime facts emitted by the core and projected by optional consumers.

use bevy::prelude::*;

/// A typed command was accepted by the runtime.
///
/// This is a core fact rather than a telemetry event. Telemetry, scripting, and
/// other observers may project it into their own channels without making the
/// engine core depend on any one consumer.
#[derive(Event, Debug, Clone)]
pub struct CommandOccurred {
    /// The stable command type name.
    pub name: String,
}

/// A recoverable runtime operation reported an error.
///
/// The owning subsystem supplies the stable name and detail. Presentation and
/// telemetry adapters decide how that fact is surfaced.
#[derive(Event, Debug, Clone)]
pub struct RuntimeError {
    /// Stable diagnostic name.
    pub name: String,
    /// Human-readable detail.
    pub message: String,
}

/// Report a recoverable runtime error to the generic event bus.
pub fn trigger_runtime_error(
    commands: &mut Commands,
    name: impl Into<String>,
    message: impl Into<String>,
) {
    commands.trigger(RuntimeError {
        name: name.into(),
        message: message.into(),
    });
}

/// A registered subsystem changed its enabled state.
#[derive(Event, Debug, Clone)]
pub struct SubsystemStateChanged {
    /// Registered subsystem key.
    pub name: String,
    /// New enabled state.
    pub on: bool,
}
