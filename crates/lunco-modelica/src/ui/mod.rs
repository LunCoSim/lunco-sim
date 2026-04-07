//! UI plugin architecture for the Modelica workbench.
//!
//! This module implements a **panel-based plugin system** where each UI window
//! is an independent Bevy plugin. This enables:
//! - External crates to add their own panels by implementing a Plugin
//! - Runtime show/hide toggling via the Panel Registry
//! - Per-panel state isolation (each panel owns its state)
//!
//! ## Architecture
//!
//! ```text
//! ModelicaUiPlugin (meta-plugin)
//! ├── WorkbenchState (shared simulation state)
//! ├── PanelRegistry (tracks all registered panels + visibility)
//! ├── LibraryBrowserPanel  ─┐
//! ├── ModelEditorPanel      │
//! ├── TelemetryPanel        ├─ Built-in panel plugins
//! ├── GraphsPanel           │
//! ├── LogsPanel            ─┘
//! ├── PanelBar (toggle window)
//! └── render_panel_bar system
//! ```
//!
//! ## Adding an External Panel
//!
//! Any crate can add a panel by implementing a Plugin:
//!
//! ```ignore
//! use lunco_modelica::ui::{PanelRegistry, PanelDescriptor, WorkbenchState};
//!
//! pub struct MyCustomPanel;
//! impl Plugin for MyCustomPanel {
//!     fn build(&self, app: &mut App) {
//!         if let Some(mut registry) = app.world_mut().get_resource_mut::<PanelRegistry>() {
//!             registry.register(PanelDescriptor {
//!                 id: "my_custom".into(),
//!                 title: "🔧 My Panel".into(),
//!                 default_pos: [10.0, 10.0],
//!                 default_size: [300.0, 400.0],
//!                 ..Default::default()
//!             });
//!         }
//!         app.add_systems(EguiPrimaryContextPass, render_my_panel);
//!     }
//! }
//! ```

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};

mod panels;
mod state;

pub use panels::*;
pub use state::*;

/// A descriptor for a UI panel that can be shown/hidden at runtime.
///
/// Panels register themselves with the `PanelRegistry` during plugin initialization.
/// The registry tracks visibility and default layout information.
#[derive(Clone)]
pub struct PanelDescriptor {
    /// Unique identifier for this panel (used for visibility toggling).
    pub id: String,
    /// Display title shown in the panel bar toggle.
    pub title: String,
    /// Default top-left position `[x, y]` in screen coordinates.
    pub default_pos: [f32; 2],
    /// Default window size `[width, height]`.
    pub default_size: [f32; 2],
    /// Whether this panel is currently visible.
    pub visible: bool,
    /// If set, the window cannot be resized wider than this value.
    pub max_width: Option<f32>,
    /// If set, the window cannot be resized narrower than this value.
    pub min_width: Option<f32>,
}

impl Default for PanelDescriptor {
    fn default() -> Self {
        Self {
            id: String::new(),
            title: String::new(),
            default_pos: [10.0, 10.0],
            default_size: [300.0, 400.0],
            visible: true,
            max_width: None,
            min_width: None,
        }
    }
}

/// Central registry of all UI panels.
///
/// Tracks which panels are registered and their visibility state.
/// Panels register themselves during `Plugin::build()`.
/// The panel bar system uses this to render show/hide checkboxes.
#[derive(Resource, Default)]
pub struct PanelRegistry {
    panels: Vec<PanelDescriptor>,
}

impl PanelRegistry {
    /// Register a new panel descriptor. Called by panel plugins during `build()`.
    pub fn register(&mut self, panel: PanelDescriptor) {
        // Don't duplicate
        if self.panels.iter().any(|p| p.id == panel.id) {
            return;
        }
        self.panels.push(panel);
    }

    /// Check if a panel with the given ID is currently visible.
    pub fn is_visible(&self, id: &str) -> bool {
        self.panels.iter().any(|p| p.id == id && p.visible)
    }

    /// Toggle visibility of a panel by ID. Returns true if now visible.
    pub fn toggle(&mut self, id: &str) -> bool {
        if let Some(panel) = self.panels.iter_mut().find(|p| p.id == id) {
            panel.visible = !panel.visible;
            panel.visible
        } else {
            false
        }
    }

    /// Get a mutable reference to a panel descriptor by ID.
    pub fn get_mut(&mut self, id: &str) -> Option<&mut PanelDescriptor> {
        self.panels.iter_mut().find(|p| p.id == id)
    }

    /// Get a shared reference to a panel descriptor by ID.
    pub fn get(&self, id: &str) -> Option<&PanelDescriptor> {
        self.panels.iter().find(|p| p.id == id)
    }

    /// Iterate over all registered panels.
    pub fn iter(&self) -> impl Iterator<Item = &PanelDescriptor> {
        self.panels.iter()
    }
}

/// Meta-plugin that assembles the Modelica workbench UI from individual panel plugins.
///
/// This plugin:
/// 1. Initializes `WorkbenchState` (shared simulation state)
/// 2. Initializes `PanelRegistry` (panel tracking)
/// 3. Adds all built-in panel plugins
/// 4. Adds the panel bar toggle window
///
/// External crates can add their own panels by calling
/// `app.add_plugins(MyCustomPanel)` after `ModelicaUiPlugin`.
pub struct ModelicaUiPlugin;

impl Plugin for ModelicaUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WorkbenchState>()
            .init_resource::<PanelRegistry>()
            .add_plugins(LibraryBrowserPanel)
            .add_plugins(ModelEditorPanel)
            .add_plugins(TelemetryPanel)
            .add_plugins(GraphsPanel)
            .add_plugins(LogsPanel)
            .add_systems(EguiPrimaryContextPass, render_panel_bar);
    }
}

/// Panel bar toggle window — shows checkboxes for all registered panels.
///
/// This small window lets users control which panels are visible at runtime.
fn render_panel_bar(
    mut contexts: EguiContexts,
    mut registry: ResMut<PanelRegistry>,
) {
    let Ok(ctx) = contexts.ctx_mut() else { return };

    egui::Window::new("📋 Panels")
        .default_pos([10.0, 380.0])
        .default_size([250.0, 30.0])
        .auto_sized()
        .resizable(false)
        .collapsible(true)
        .show(ctx, |ui| {
            ui.heading("Panel Visibility");
            ui.separator();
            // Collect IDs and visibility first to avoid borrow conflict
            let panels: Vec<_> = registry.iter().map(|p| (p.id.clone(), p.visible, p.title.clone())).collect();
            for (id, visible, title) in &panels {
                let mut v = *visible;
                ui.checkbox(&mut v, title);
                if v != *visible {
                    registry.toggle(id);
                }
            }
        });
}
