//! Backend-neutral transient notification contracts.

use bevy::prelude::*;
use lunco_core::Command;

/// Show a transient on-screen notification (toast) to the player.
///
/// The application runtime owns the command observer and headless queue lifecycle;
/// optional UI adapters render active toasts. Fired from
/// rhai via `notify(msg)` / `notify_kind(msg, kind)` (see the prelude) so a
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

/// One active on-screen toast.
#[derive(Clone, Debug)]
pub struct Toast {
    /// Message text.
    pub text: String,
    /// Visual style: "info" | "success" | "warn" | "error".
    pub kind: String,
    /// Seconds left before it disappears.
    pub remaining: f32,
}

/// Queue of transient on-screen notifications.
///
/// It is always present, including in headless hosts, so the command has one
/// authoritative runtime sink while presentation remains optional.
#[derive(Resource, Default)]
pub struct ScreenNotifications {
    /// Pending toasts, oldest first.
    pub toasts: Vec<Toast>,
}
