//! 3D world-space UI components — panels and labels in the scene.
//!
//! Used for in-cockpit displays, body labels, orbit annotations.
//! These render as textured quads in the 3D world, not as screen overlays.

use bevy::prelude::*;
use bevy::math::DVec3;

/// Distance-based LOD configuration for world-space UI.
#[derive(Clone, Copy)]
pub struct WorldLod {
    /// Start fading out at this distance (meters).
    pub fade_start: f64,
    /// Fully invisible beyond this distance.
    pub fade_end: f64,
}

impl WorldLod {
    /// Compute opacity factor (0.0 = hidden, 1.0 = fully visible).
    pub fn opacity(&self, distance: f64) -> f32 {
        if distance <= self.fade_start {
            1.0
        } else if distance >= self.fade_end {
            0.0
        } else {
            (1.0 - (distance - self.fade_start) / (self.fade_end - self.fade_start)) as f32
        }
    }

    /// Whether this widget should be visible at the given distance.
    pub fn visible(&self, distance: f64) -> bool {
        distance < self.fade_end
    }
}

/// Marks an entity as a world-space UI panel.
#[derive(Component, Clone)]
pub struct WorldPanel {
    /// Size of the panel in world units (meters).
    pub size: Vec2,
    /// Offset from the parent entity in big_space coordinates.
    pub offset: DVec3,
    /// LOD configuration. If None, always visible.
    pub lod: Option<WorldLod>,
}

/// A 3D text label floating in world space.
#[derive(Component, Clone)]
pub struct Label3D {
    pub text: String,
    pub offset: DVec3,
    pub font_size: f32,
    pub color: Color,
    pub billboard: bool,
    pub lod: Option<WorldLod>,
}

impl Label3D {
    pub fn new(text: impl Into<String>, theme: &lunco_theme::Theme) -> Self {
        Self {
            text: text.into(),
            offset: DVec3::ZERO,
            font_size: 16.0,
            color: Color::srgb_u8(
                theme.colors.text.r(),
                theme.colors.text.g(),
                theme.colors.text.b(),
            ),
            billboard: true,
            lod: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lod_visible_when_close() {
        let lod = WorldLod { fade_start: 1000.0, fade_end: 5000.0 };
        assert!(lod.visible(500.0));
        assert_eq!(lod.opacity(500.0), 1.0);
    }

    #[test]
    fn test_lod_fading() {
        let lod = WorldLod { fade_start: 1000.0, fade_end: 5000.0 };
        assert_eq!(lod.opacity(1000.0), 1.0);
        assert_eq!(lod.opacity(5000.0), 0.0);
        let mid = lod.opacity(3000.0);
        assert!((mid - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_lod_hidden_when_far() {
        let lod = WorldLod { fade_start: 1000.0, fade_end: 5000.0 };
        assert!(!lod.visible(6000.0));
        assert_eq!(lod.opacity(10000.0), 0.0);
    }
}
