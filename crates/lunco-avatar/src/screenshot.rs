//! Screenshot capture functionality for the avatar system.

use bevy::prelude::*;
use bevy::render::view::screenshot::Screenshot;
use lunco_core::{Command, SceneViewport, on_command};

/// Command to capture a screenshot from the primary window.
#[Command(default)]
pub struct CaptureScreenshot {}

/// System to trigger screenshot capture of the primary window.
#[on_command(CaptureScreenshot)]
pub fn on_capture_screenshot(
    trigger: On<CaptureScreenshot>,
    mut commands: Commands,
) {
    // Spawn a transient entity — do NOT insert on the camera, which carries
    // FloatingOrigin and would confuse BigSpace if its components are touched.
    commands.spawn(Screenshot::primary_window());
}

/// Capture a screenshot from a specific camera. With `target = None` (the
/// default) captures from the active scene camera (`SceneViewport::active_camera`),
/// falling back to the primary window; with `target = Some(vessel)` captures
/// from a `Camera3d` mounted on that vessel (found by walking the vessel's
/// descendant hierarchy). The latter is what a `run_tool("science::take_photo")`
/// leaf fires to photograph from a rover's viewpoint.
///
/// The captured frame flows through the existing `ScreenshotCaptured` observer
/// in `lunco-api` (file-save / PNG-encode) for the HTTP path; in-process/rhai
/// captures without the API just spawn the `Screenshot` (matching
/// [`CaptureScreenshot`]'s behaviour today).
// `default`: `target` must have a reflect default, or the executor's
// constructibility guard drops a no-param call — `photo()` in `control.rhai`
// sends `{}`, which could never be deserialized into this struct. Same reason
// `CaptureScreenshot` above carries it. The default (`None`) is exactly the
// documented "capture from the active scene camera".
#[Command(default)]
pub struct CaptureFromCamera {
    /// Vessel whose mounted camera to capture from. `None` → the active scene
    /// camera, falling back to the primary window when none is bound.
    pub target: Option<Entity>,
}

#[on_command(CaptureFromCamera)]
pub fn on_capture_from_camera(
    trigger: On<CaptureFromCamera>,
    viewport: Option<Res<SceneViewport>>,
    // `RenderTarget` is a separate component (see `camera_switch.rs`), not a
    // field on `Camera` — query it alongside so we know which window to capture.
    cameras: Query<(&Camera, &Camera3d, &bevy::camera::RenderTarget)>,
    children: Query<&Children>,
    mut commands: Commands,
) {
    let target = trigger.event().target;
    // Resolve the camera entity to capture from.
    let camera_entity = match target {
        // A specific vessel → find a Camera3d among its descendants (its USD
        // `def Camera` mount). Generic over `MountedCamera` (which lives in
        // `lunco-usd-bevy`, not a dependency here).
        Some(vessel) => find_descendant_camera(vessel, &cameras, &children),
        // No target → the active scene camera, else None (→ primary window).
        None => viewport.as_deref().and_then(|v| v.active_camera),
    };

    // Distinguish "explicit target requested but not found" (a vessel with no
    // camera — capturing the primary window would silently photograph the WRONG
    // viewport, which is incorrect data for a science instrument) from "no
    // target requested" (the active-camera/primary-window fallback is intended).
    // The former warns + no-ops; the latter proceeds to the fallback.
    let Some(camera_entity) = camera_entity else {
        if target.is_some() {
            warn!(
                "[CaptureFromCamera] target vessel has no Camera3d descendant; \
                 not capturing (would photograph the wrong viewport)"
            );
            return;
        }
        // No target requested and no active camera → primary window.
        commands.spawn(Screenshot::primary_window());
        return;
    };

    // Bevy's `Screenshot` captures a *render target* (window/image), not a
    // camera directly. Resolve the chosen camera's target; if it renders to a
    // window, capture that window.
    let Some((cam, _, rt)) = cameras.get(camera_entity).ok() else {
        commands.spawn(Screenshot::primary_window());
        return;
    };

    // Capturing a WINDOW captures whatever camera is actually drawing it — not
    // necessarily the camera we resolved. A vessel's mounted camera is usually
    // INACTIVE (the operator is flying the free camera), so capturing the window
    // here would photograph the operator's viewport and pass it off as the
    // vessel's instrument data — silently wrong, which for a science instrument is
    // worse than no data. Refuse instead.
    //
    // Making this actually capture an inactive mounted camera needs a
    // render-to-image target (`RenderTarget::Image` + `Screenshot::image`), so the
    // camera renders its own view off-screen regardless of what the window shows.
    // Until then an explicit vessel capture only succeeds when its camera is live.
    if target.is_some() && !cam.is_active {
        warn!(
            "[CaptureFromCamera] target vessel's camera is not active; not capturing \
             (a window capture would photograph the operator's viewport, not the \
             vessel's). Needs a render-to-image target for inactive mounted cameras."
        );
        return;
    }
    let screenshot = match rt {
        bevy::camera::RenderTarget::Window(w) => match w {
            bevy::window::WindowRef::Primary => Screenshot::primary_window(),
            bevy::window::WindowRef::Entity(entity) => Screenshot::window(*entity),
        },
        // Image/texture-view targets aren't capturable via `Screenshot`
        // (they'd double-render); fall back to the primary window.
        _ => Screenshot::primary_window(),
    };
    commands.spawn(screenshot);
}

/// Walk `root`'s descendants (BFS) and return the first `Camera3d` found — the
/// mounted camera of a vessel.
fn find_descendant_camera(
    root: Entity,
    cameras: &Query<(&Camera, &Camera3d, &bevy::camera::RenderTarget)>,
    children: &Query<&Children>,
) -> Option<Entity> {
    let mut stack = vec![root];
    while let Some(entity) = stack.pop() {
        if cameras.get(entity).is_ok() {
            return Some(entity);
        }
        if let Ok(kids) = children.get(entity) {
            stack.extend(kids.iter());
        }
    }
    None
}
