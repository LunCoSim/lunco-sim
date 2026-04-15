//! Screenshot capture functionality for the avatar system.

use bevy::prelude::*;
use bevy::render::view::screenshot::Screenshot;
use lunco_core::{Command, on_command};

/// Command to capture a screenshot from the primary window.
#[Command(default)]
pub struct CaptureScreenshot {}

/// System to trigger screenshot capture on a camera entity.
#[on_command(CaptureScreenshot)]
pub fn on_capture_screenshot(
    _cmd: CaptureScreenshot,
    mut commands: Commands,
    q_cameras: Query<Entity, With<Camera3d>>,
) {
    // Grab first camera and trigger screenshot
    if let Some(entity) = q_cameras.iter().next() {
        commands.entity(entity).insert(Screenshot::primary_window());
    }
}
