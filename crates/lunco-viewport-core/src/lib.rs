//! Renderer-independent measured viewport geometry.
//!
//! [`PanelRect`] is the physical-pixel footprint of a scene or panel inside a
//! window. The Workbench records collections of these footprints, while
//! render-free scene and camera contracts can carry one without importing
//! egui, a dock implementation, or a renderer.

use bevy::prelude::{Camera, Entity, GlobalTransform, Ray3d, Resource, SystemSet, UVec2, Vec2};

/// One panel's footprint inside the window, in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PanelRect {
    /// Top-left of the panel rect inside the window framebuffer.
    pub origin: UVec2,
    /// Width × height of the rect. It is never zero.
    pub size: UVec2,
}

/// The main window's 3D viewport: its explicit camera binding, visibility,
/// and optional physical-pixel sub-rect.
#[derive(Resource, Debug, Clone)]
pub struct SceneViewport {
    /// The camera bound to the main window. `None` is an intentional no-camera
    /// state and is never replaced by an implicit camera choice.
    pub active_camera: Option<Entity>,
    /// Whether the 3D scene is visible at all.
    pub visible: bool,
    /// Physical `(position, size)` sub-rect inside the window, or `None` for
    /// the full window.
    pub rect: Option<(UVec2, UVec2)>,
}

impl Default for SceneViewport {
    fn default() -> Self {
        Self {
            active_camera: None,
            visible: true,
            rect: None,
        }
    }
}

/// Ordering boundary for the main presentation viewport.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SceneViewportSet {
    /// Publish visibility and layout data into [`SceneViewport`].
    Publish,
    /// Reconcile the explicit binding into render-camera state.
    Reconcile,
}

/// Camera ray for a discrete scene click.
pub fn scene_click_ray(
    pointer_blocked: bool,
    camera: &Camera,
    cam_gtf: &GlobalTransform,
    cursor: Vec2,
) -> Option<Ray3d> {
    if pointer_blocked {
        return None;
    }
    let local = camera
        .logical_viewport_rect()
        .map_or(cursor, |rect| cursor - rect.min);
    camera.viewport_to_world(cam_gtf, local).ok()
}
