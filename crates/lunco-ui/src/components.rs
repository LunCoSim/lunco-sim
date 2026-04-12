//! 3D world-space UI components — panels and labels in the scene.
//!
//! Used for in-cockpit displays, body labels, orbit annotations.
//! These render as textured quads in the 3D world, not as screen overlays.

use bevy::prelude::*;
use bevy::math::DVec3;

/// Marks an entity as a world-space UI panel.
#[derive(Component, Clone)]
pub struct WorldPanel {
    /// Size of the panel in world units (meters).
    pub size: Vec2,
    /// Offset from the parent entity in big_space coordinates.
    pub offset: DVec3,
}

/// A 3D text label floating in world space.
#[derive(Component, Clone)]
pub struct Label3D {
    pub text: String,
    pub offset: DVec3,
    pub font_size: f32,
    pub color: Color,
    pub billboard: bool,
}
