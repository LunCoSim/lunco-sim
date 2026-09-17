//! Renderer-independent measured viewport geometry.
//!
//! [`PanelRect`] is the physical-pixel footprint of a scene or panel inside a
//! window. The Workbench records collections of these footprints, while
//! render-free scene and camera contracts can carry one without importing
//! egui, a dock implementation, or a renderer.

use bevy::prelude::UVec2;

/// One panel's footprint inside the window, in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PanelRect {
    /// Top-left of the panel rect inside the window framebuffer.
    pub origin: UVec2,
    /// Width × height of the rect. It is never zero.
    pub size: UVec2,
}
