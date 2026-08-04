//! Small UI selection resource shared by domain panels.

use bevy::prelude::*;

/// Tracks which entity is currently selected in the UI.
#[derive(Resource, Default)]
pub struct UiSelection {
    pub entity: Option<Entity>,
}
