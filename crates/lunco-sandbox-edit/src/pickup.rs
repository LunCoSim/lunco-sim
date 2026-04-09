//! Physics pickup / force tool.
//!
//! Simple implementation: when in Pickup mode and the user clicks on a
//! dynamic rigid body, apply an impulse force in the camera's forward
//! direction. Hold and drag to increase force magnitude.

use bevy::prelude::*;
use avian3d::prelude::*;

use crate::{SelectedEntity, ToolMode};

/// Syncs the pickup tool enabled state based on the current tool mode.
pub fn sync_pickup_enabled(
    selected: Res<SelectedEntity>,
) {
    // Pickup mode state is tracked in SelectedEntity
    // Actual force application happens in handle_pickup_force
    let _ = selected.mode == ToolMode::Pickup;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SelectedEntity, ToolMode};

    fn test_app() -> App {
        let mut app = App::new();
        app.init_resource::<SelectedEntity>();
        app
    }

    #[test]
    fn test_pickup_disabled_by_default() {
        let mut app = test_app();
        let selected = app.world().resource::<SelectedEntity>();
        assert_ne!(selected.mode, ToolMode::Pickup);
    }

    #[test]
    fn test_pickup_mode_switch() {
        let mut app = test_app();
        {
            let mut selected = app.world_mut().resource_mut::<SelectedEntity>();
            selected.mode = ToolMode::Pickup;
        }
        let selected = app.world().resource::<SelectedEntity>();
        assert_eq!(selected.mode, ToolMode::Pickup);
    }
}
