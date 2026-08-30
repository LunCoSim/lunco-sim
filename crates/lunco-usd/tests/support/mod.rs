//! Shared helpers for production USD integration tests.

use bevy::prelude::{App, Or, With};
use lunco_usd_bevy::{UsdAwaitingStage, UsdVisualProjectionQueued};

/// Advance the production projection pipeline until no USD prim is waiting for
/// a stage or visual projection, including the dependent observer frame.
pub(crate) fn settle_visual_projection(app: &mut App) {
    let mut empty_frames = 0;
    for _ in 0..1000 {
        app.update();
        let pending = {
            let world = app.world_mut();
            let mut query = world.query_filtered::<(), Or<(
                With<UsdVisualProjectionQueued>,
                With<UsdAwaitingStage>,
            )>>();
            query.iter(world).next().is_some()
        };
        if pending {
            empty_frames = 0;
        } else {
            empty_frames += 1;
            if empty_frames == 2 {
                return;
            }
        }
    }
    panic!("USD visual projection did not settle within 1000 frames");
}
