//! UI context — unified params passed to panel render functions.
//!
//! Provides ergonomic access to resources and CommandMessage triggering.
//! For panels with many query fields, prefer defining a custom
//! `#[derive(SystemParam)]` struct + `WidgetSystem` impl directly.

use bevy::prelude::*;
use bevy_egui::egui;
use crate::{WidgetId, widget, WidgetSystem};

/// Tracks which entity is currently selected in the UI.
#[derive(Resource, Default)]
pub struct UiSelection {
    pub entity: Option<Entity>,
}

/// Unified context passed to panel render functions.
pub struct UiContext<'w, 's> {
    /// The egui context for this frame.
    pub egui: &'w mut egui::Context,
    /// Access to the Bevy world for queries.
    pub world: &'w mut World,
    /// Commands for triggering actions.
    pub commands: Commands<'w, 's>,
}

impl<'w, 's> UiContext<'w, 's> {
    /// Get immutable access to a resource.
    pub fn resource<R: Resource>(&self) -> &R {
        self.world.resource::<R>()
    }

    /// Get mutable access to a resource.
    pub fn resource_mut<R: Resource>(&mut self) -> Mut<'_, R> {
        self.world.resource_mut::<R>()
    }

    /// Check if the world contains an entity.
    pub fn has_entity(&self, entity: Entity) -> bool {
        self.world.get_entity(entity).is_ok()
    }

    /// Get mutable access to commands for triggering events.
    /// Use `ctx.commands.trigger(TypedCommand { ... })` to fire commands.
    pub fn commands(&mut self) -> &mut Commands<'w, 's> {
        &mut self.commands
    }

    /// Render a nested widget using the WidgetSystem pattern.
    /// This provides composability — panels can contain other widgets.
    pub fn render<W: WidgetSystem + 'static>(
        &mut self,
        ui: &mut egui::Ui,
        id: WidgetId,
    ) {
        widget::<W>(self.world, ui, id);
    }
}
