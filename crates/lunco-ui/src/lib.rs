//! # LunCoSim UI Foundation
//!
//! A thin adapter layer on top of `bevy_workbench` that provides:
//! - **WidgetSystem** — O(1) cached widget pattern for 1,000s of graph/diagram widgets
//! - **CommandMessage integration** — all UI interactions flow through CommandMessage events
//! - **3D World-Space UI** — in-cockpit panels, floating labels over celestial bodies
//!
//! Docking, theming, inspector, console, layout persistence — all provided by `bevy_workbench`.

pub mod widget;
pub use widget::*;

pub mod components;
pub use components::*;

pub mod context;
pub use context::*;

pub mod helpers;
pub use helpers::*;

/// Common exports. Use `use lunco_ui::prelude::*;`
pub mod prelude {
    pub use bevy_egui::egui;
    pub use crate::WidgetId;
    pub use crate::WidgetSystem;
    pub use crate::widget;
    pub use crate::WidgetCache;
    pub use crate::UiContext;
    pub use crate::UiSelection;
    pub use crate::CommandBuilder;
    pub use crate::WorldPanel;
    pub use crate::Label3D;
}

use bevy::prelude::*;

/// Minimal plugin that initializes LunCoSim-specific UI resources.
/// The heavy lifting (docking, themes, inspector, console) is done by `bevy_workbench`.
#[derive(Default)]
pub struct LuncoUiPlugin;

impl Plugin for LuncoUiPlugin {
    fn build(&self, app: &mut App) {
        // LunCoSim-specific resources (no overlap with bevy_workbench)
        app.init_resource::<UiSelection>();
    }
}
