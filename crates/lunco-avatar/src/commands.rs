//! Typed commands for avatar camera control and vessel possession.

use bevy::prelude::*;
use lunco_core::Command;

/// Possess a vessel, taking direct control of it.
///
/// Switches the avatar to a vessel-locked camera mode and inserts a
/// `ControllerLink` so that input events are forwarded to the vessel.
#[Command]
pub struct PossessVessel {
    /// The avatar entity taking possession — this process's *local* embodiment
    /// in the world, used only to bind the chase camera. `None` for headless or
    /// direct API control with no camera binding.
    #[sync_local]
    #[serde(default)]
    #[reflect(default)]
    pub avatar: Option<Entity>,
    /// The entity to possess (becomes the controlled vessel).
    pub target: Entity,
    /// Whether possession also rebinds the avatar's camera to the chase rig —
    /// the default, interactive behaviour. `false` claims control authority
    /// only: what a recording scenario wants, where the script drives the
    /// vessel through ports while an authored camera path owns the view.
    /// A camera bind with no explicit avatar resolves only the authoritative
    /// `TheLocalAvatar` slot. It fails visibly when that slot is empty; it never
    /// selects an arbitrary `Avatar` entity.
    #[serde(default = "default_true")]
    #[reflect(default = "default_true")]
    pub bind_camera: bool,
}

fn default_true() -> bool {
    true
}

/// Release possession of the currently controlled vessel.
///
/// Removes the `ControllerLink` and returns the avatar to free-flight mode.
/// Keeps the camera at its current position — no jarring teleport.
#[Command]
pub struct ReleaseVessel {
    /// The avatar entity releasing possession.
    pub target: Entity,
}

/// Return the local camera from a celestial orbit view to the exact camera
/// mode and BigSpace frame from which that view was entered.
///
/// Unlike [`ReleaseVessel`], this is a presentation transition: it does not
/// release control authority or remove a `ControllerLink`.
#[Command]
pub struct ReturnFromOrbit {
    /// The local avatar camera returning from orbit view.
    pub target: Entity,
}

/// Focus on a target without taking control.
///
/// Switches the avatar to `OrbitCamera` mode centered on the target.
#[Command]
pub struct FocusTarget {
    /// The avatar entity that is focusing (local camera representation). `None`
    /// for headless/direct control with no avatar.
    #[sync_local]
    #[serde(default)]
    #[reflect(default)]
    pub avatar: Option<Entity>,
    /// The entity to focus on.
    pub target: Entity,
}

/// Tune pointer-to-camera response while the application is running.
///
/// Omitted fields retain their current persisted value. The same typed
/// [`crate::CameraInputSettings`] resource drives free, surface, chase, and
/// body-orbit cameras; this command is the API/script boundary for changing it
/// without introducing a second transient set of camera constants.
#[Command(default)]
pub struct SetCameraInput {
    /// Camera radians per pointer-motion unit before behavior-specific scaling.
    pub look_radians_per_pointer_unit: Option<f32>,
    /// Lower bound for orbital rotation at the body's surface, in `[0, 1]`.
    pub orbit_surface_min_scale: Option<f64>,
    /// Positive exponent shaping the apparent-horizon distance response.
    pub orbit_distance_curve_exponent: Option<f64>,
}

/// Follow a target with the chase camera, without taking control.
///
/// Inserts `SpringArmCamera` so the camera tracks the target's heading,
/// but omits `ControllerLink` and vessel input bindings — keyboard input
/// stays inert toward the target. Use this for non-vessel objects (balloons,
/// props, observation targets) where the player wants to ride along but
/// not drive. `PossessVessel` is conceptually `FollowTarget` plus a
/// controller binding.
#[Command]
pub struct FollowTarget {
    /// The avatar entity that will follow (local camera representation). `None`
    /// for headless/direct control with no avatar.
    #[sync_local]
    #[serde(default)]
    #[reflect(default)]
    pub avatar: Option<Entity>,
    /// The entity to follow.
    pub target: Entity,
}

/// Update the profile name for the active user session.
#[Command(default)]
pub struct UpdateProfile {
    pub name: String,
}

/// Show a transient on-screen notification (toast) to the player.
///
/// Pushes onto the [`crate::ScreenNotifications`] resource; the ui-gated
/// `draw_notifications` overlay renders active toasts top-center and fades them
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
