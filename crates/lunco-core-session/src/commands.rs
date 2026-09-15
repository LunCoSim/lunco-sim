//! Typed commands owned by the session and identity subsystem.

use bevy::prelude::*;
use lunco_core::Command;

/// Update the display name associated with the active user session.
#[Command(default)]
pub struct UpdateProfile {
    /// New session display name.
    pub name: String,
}
