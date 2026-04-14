//! # LunCoSim UI Foundation
//!
//! A thin adapter layer on top of `bevy_workbench` that provides:
//! - **WidgetSystem** — O(1) cached widget pattern for 1,000s of graph/diagram widgets
//! - **Typed command integration** — all UI interactions flow through typed command events
//! - **3D World-Space UI** — in-cockpit panels, floating labels over celestial bodies
//!
//! Docking, theming, inspector, console, layout persistence — all provided by `bevy_workbench`.
//!
//! ## Architecture: Entity Viewers
//!
//! UI panels are **entity viewers** — they watch a selected entity and render its data.
//! The same panel works in a standalone workbench, a 3D overlay, or a mission dashboard.
//!
//! ```text
//!   Domain crate (lunco-modelica, lunco-fsw, etc.)
//!     ├── Defines entity component (ModelicaModel, FswConfig, etc.)
//!     ├── Defines viewer panel (DiagramPanel, CodeEditor, etc.)
//!     └── Panel watches WorkbenchState.selected_entity
//!           │
//!           ▼
//!   lunco-ui
//!     ├── Re-exports egui-snarl types for node graphs
//!     ├── Provides WidgetSystem for cached widget rendering
//!     └── Provides WorldPanel for 3D space panels
//! ```
//!
//! See `docs/research-ui-ux-architecture.md` for full architecture.

use bevy::prelude::*;

pub mod widget;
pub use widget::*;

pub mod components;
pub use components::*;

pub mod context;
pub use context::*;

pub mod helpers;
pub use helpers::*;

pub mod diagrams;
pub use diagrams::*;

pub mod mission_control;
pub use mission_control::*;

pub mod telemetry;
pub use telemetry::*;

/// Common exports. Use `use lunco_ui::prelude::*;`
pub mod prelude {
    pub use bevy_egui::egui;
    pub use crate::WidgetId;
    pub use crate::WidgetSystem;
    pub use crate::widget;
    pub use crate::WidgetCache;
    pub use crate::UiContext;
    pub use crate::UiSelection;
    pub use crate::WorldPanel;
    pub use crate::Label3D;
    pub use crate::diagrams::{
        time_series_plot, ChartSeries,
        Snarl, SnarlViewer, NodeId, InPin, InPinId, OutPin, OutPinId,
    };
}

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
