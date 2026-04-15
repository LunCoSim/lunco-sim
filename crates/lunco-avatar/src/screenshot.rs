//! Screenshot capture functionality for the avatar system.

use bevy::prelude::*;
use bevy::render::view::screenshot::Screenshot;
use lunco_core::{Command, on_command};

/// Command to capture a screenshot from the primary window.
#[Command(default)]
pub struct CaptureScreenshot {}

/// System to trigger screenshot capture of the primary window.
#[on_command(CaptureScreenshot)]
pub fn on_capture_screenshot(
    _cmd: CaptureScreenshot,
    mut commands: Commands,
) {
    // Spawn a transient entity — do NOT insert on the camera, which carries
    // FloatingOrigin and would confuse BigSpace if its components are touched.
    commands.spawn(Screenshot::primary_window());
}
