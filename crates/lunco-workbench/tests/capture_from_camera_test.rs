//! TDD for [`CaptureFromCamera`] — the typed command a `science::take_photo`
//! tool fires. The load-bearing case is the B3 fix: when `target: Some(vessel)`
//! and the vessel has NO `Camera3d` descendant, the observer must NOT silently
//! capture the primary window (that would photograph the wrong viewport);
//! it warns + no-ops.

use bevy::prelude::*;
use bevy::render::view::screenshot::Screenshot;
use lunco_workbench::screenshot::CaptureFromCamera;

/// Wire just the capture observers (no full `WorkbenchPlugin` — keeps the test
/// focused + fast, and avoids standing up egui/winit).
fn wire(app: &mut App) {
    lunco_workbench::screenshot::register_all_commands(app);
}

#[test]
fn vessel_with_no_camera_does_not_capture() {
    // B3: a vessel with no Camera3d descendant → no Screenshot spawned (would
    // otherwise photograph the wrong viewport — incorrect data for a science
    // instrument).
    let mut app = App::new();
    wire(&mut app);
    let vessel = app.world_mut().spawn_empty().id(); // no Camera3d child
    app.world_mut().trigger(CaptureFromCamera {
        target: Some(vessel),
    });
    app.world_mut().flush();
    let screenshots = app
        .world_mut()
        .query::<&Screenshot>()
        .iter(app.world())
        .count();
    assert_eq!(
        screenshots, 0,
        "vessel with no camera must not spawn a Screenshot"
    );
}

#[test]
fn no_target_falls_back_to_primary_window() {
    // The intended fallback: `target: None` captures the primary window
    // (the active-scene-camera case). Even with no active camera, the None
    // branch proceeds to a primary-window screenshot.
    let mut app = App::new();
    wire(&mut app);
    app.world_mut().trigger(CaptureFromCamera { target: None });
    app.world_mut().flush();
    let screenshots = app
        .world_mut()
        .query::<&Screenshot>()
        .iter(app.world())
        .count();
    assert_eq!(
        screenshots, 1,
        "target: None falls back to a primary-window capture"
    );
}
