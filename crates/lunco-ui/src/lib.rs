//! # LunCoSim UI Foundation
//!
//! A thin reusable UI layer that provides:
//! - **WidgetSystem** — O(1) cached widget pattern for 1,000s of graph/diagram widgets
//! - **Typed command integration** — all UI interactions flow through typed command events
//! - **3D World-Space UI** — in-cockpit panels, floating labels over celestial bodies
//!
//! Docking and layout are provided by the concrete workbench shell; this
//! package only depends on renderer-neutral contracts and reusable widgets.
//!
//! ## Architecture: Entity Viewers
//!
//! UI panels are **entity viewers** — they watch a selected entity and render its data.
//! The same panel works in a standalone workbench, a 3D overlay, or a mission dashboard.
//!
//! ```text
//!   Domain crate (lunco-modelica-core, lunco-mobility, etc.)
//!     ├── Defines entity component (ModelicaModel, joint/port state, etc.)
//!     ├── Defines viewer panel (DiagramPanel, CodeEditor, etc.)
//!     └── Panel watches WorkbenchState.selected_entity
//!           │
//!           ▼
//!   lunco-ui
//!     ├── Provides WidgetSystem for cached widget rendering
//!     └── Provides WorldPanel for 3D space panels
//! ```
//!
//! See `docs/architecture/11-workbench.md` for the current UI architecture.

use bevy::prelude::*;

pub mod widget;
pub use widget::*;

pub mod components;
pub use components::*;

pub mod context;
pub use context::*;

pub mod theme {
    pub use lunco_theme::*;
}

pub mod diagrams;
pub use diagrams::*;

pub mod log;

pub mod mission_control;
pub use mission_control::*;

pub mod busy;
pub mod modal;

/// Common exports. Use `use lunco_ui::prelude::*;`
pub mod prelude {
    pub use crate::diagrams::{time_series_plot, ChartSeries};
    pub use crate::widget;
    pub use crate::Label3D;
    pub use crate::UiSelection;
    pub use crate::WidgetCache;
    pub use crate::WidgetId;
    pub use crate::WidgetSystem;
    pub use crate::WorldPanel;
    pub use bevy_egui::egui;
    pub use lunco_theme::{Theme, ThemeMode, ThemePlugin};
}

/// Minimal plugin that initializes LunCoSim-specific UI resources.
/// The concrete workbench shell owns docking and layout; this plugin only
/// initializes LunCoSim-specific UI resources.
#[derive(Default)]
pub struct LuncoUiPlugin;

impl Plugin for LuncoUiPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<lunco_avatar_core::roles::AvatarCorePlugin>() {
            app.add_plugins(lunco_avatar_core::roles::AvatarCorePlugin);
        }
        app.init_resource::<UiSelection>();
        // WP-8 view-model for Mission Control — collapses the panel's
        // per-frame world scans into one change-gated producer.
        app.init_resource::<mission_control::MissionControlView>()
            .add_observer(mission_control::on_set_possession_policy)
            .add_systems(Update, mission_control::populate_mission_control_view);
        // Modal host: single-source-of-truth for dialogs. Panels never
        // call `egui::Window::show` directly; they push to ModalQueue
        // and the host renders the head with `egui::Modal`. Registered
        // in `EguiPrimaryContextPass` because the host pulls
        // `EguiContexts` to paint on the active egui context.
        app.init_resource::<modal::ModalQueue>().add_systems(
            bevy_egui::EguiPrimaryContextPass,
            modal::host::render_modal_host
                .in_set(lunco_workbench_core::ApplicationOverlayRenderSet),
        );
        modal::register_all_commands(app);
    }
}
