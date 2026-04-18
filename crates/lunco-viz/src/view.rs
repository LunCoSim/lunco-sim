//! View targets — *where* a visualization renders.
//!
//! A viz is a data-to-geometry transform; a view is a rendering surface.
//! Separating them means one viz kind can potentially render into more
//! than one view (e.g. a trajectory drawn in both the main 3D viewport
//! and a mini-panel), and one view can host many vizes (a 2D plot panel
//! with multiple overlaid line plots).
//!
//! Only `Panel2D` is fully wired in v0.1. The 3D variants are kept in
//! the enum so callers can pattern-match exhaustively and so viz kinds
//! can declare compatibility against a stable set; the render paths for
//! them land in a later milestone.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// Concrete rendering target for one visualization.
///
/// `Panel2D` / `Panel3D` don't carry a panel id — each viz instance
/// gets its own [`VizPanel`](crate::panel::VizPanel), addressed by its
/// [`VizId`](crate::viz::VizId). Keeping this enum flat makes it
/// trivially serializable (workspace files).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ViewTarget {
    /// Render into the viz's own 2D egui panel.
    Panel2D,
    /// Render as an overlay in the primary Bevy 3D viewport. Used for
    /// gizmos (arrows, frames) and managed entities (trajectories).
    /// Not yet implemented.
    Viewport3D,
    /// Render into a render-to-texture sub-scene displayed inside the
    /// viz's own egui panel. Used when a 3D view wants to be its own
    /// tab rather than layered on the main scene. Not yet implemented.
    Panel3D,
}

/// Taxonomy of what a view can host. Viz kinds declare
/// `compatible_views() -> &[ViewKind]` so the inspector UI can filter
/// the "where to render" menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ViewKind {
    Panel2D,
    Viewport3D,
    Panel3D,
}

impl ViewTarget {
    pub fn kind(&self) -> ViewKind {
        match self {
            ViewTarget::Panel2D => ViewKind::Panel2D,
            ViewTarget::Viewport3D => ViewKind::Viewport3D,
            ViewTarget::Panel3D => ViewKind::Panel3D,
        }
    }
}

/// Context passed to a viz kind's 2D render path. Just the egui `Ui`
/// for now; richer context (shared cursors, linked axes, theme) gets
/// added here as features land.
pub struct Panel2DCtx<'a> {
    pub ui: &'a mut bevy_egui::egui::Ui,
    pub world: &'a mut World,
}
