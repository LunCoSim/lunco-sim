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

pub mod diagrams;
pub use diagrams::*;

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
    pub use crate::diagrams::{
        time_series_plot, ChartSeries,
        Snarl, SnarlViewer, NodeId, InPin, InPinId, OutPin, OutPinId,
    };
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

        // 3D UI LOD system — updates visibility based on camera distance
        app.add_systems(PostUpdate, update_3d_ui_lod);
    }
}

/// System that updates 3D UI visibility based on LOD and camera distance.
/// Runs in PostUpdate after transforms are propagated.
fn update_3d_ui_lod(
    mut q_panels: Query<(&WorldPanel, &mut Visibility, &GlobalTransform)>,
    mut q_labels: Query<(&Label3D, &mut Visibility, &GlobalTransform)>,
    q_camera: Query<&GlobalTransform, With<lunco_core::Avatar>>,
) {
    let camera_pos = q_camera.single().ok().map(|gt| gt.translation());

    for (panel, mut visibility, gt) in q_panels.iter_mut() {
        let Some(cam) = camera_pos else { continue };
        let dist = gt.translation().distance(cam);
        if let Some(lod) = panel.lod {
            if crate::components::WorldLod::visible(&lod, dist as f64) {
                *visibility = Visibility::Visible;
            } else {
                *visibility = Visibility::Hidden;
            }
        }
    }

    for (label, mut visibility, gt) in q_labels.iter_mut() {
        let Some(cam) = camera_pos else { continue };
        let dist = gt.translation().distance(cam);
        if let Some(lod) = label.lod {
            if crate::components::WorldLod::visible(&lod, dist as f64) {
                *visibility = Visibility::Visible;
            } else {
                *visibility = Visibility::Hidden;
            }
        }
    }
}
