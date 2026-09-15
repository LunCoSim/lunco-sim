//! Typed commands implemented by the avatar UI-facing runtime.

use bevy::prelude::*;
use lunco_core::Command;

/// Show a transient on-screen notification (toast) to the player.
///
/// Pushes onto the [`crate::ScreenNotifications`] resource; the optional
/// `lunco-avatar-ui` adapter renders active toasts top-center and fades them
/// out. Headless hosts accept the command (and log it) but draw nothing. Fired
/// from rhai via `notify(msg)` / `notify_kind(msg, kind)` (see the prelude) so a
/// scenario can announce each phase without touching Rust.
#[Command(default)]
pub struct ShowNotification {
    /// The message text.
    pub text: String,
    /// Visual style: "info" (default), "success", "warn", or "error".
    #[serde(default)]
    #[reflect(default)]
    pub kind: String,
    /// Seconds to display; `0` uses the default (~4.5s).
    #[serde(default)]
    #[reflect(default)]
    pub secs: f32,
}
