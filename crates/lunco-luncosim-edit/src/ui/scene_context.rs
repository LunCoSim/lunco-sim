//! Pointer policies for USD-authored scene markers.

use bevy::picking::Pickable;
use bevy::prelude::*;
use lunco_core::{PointerInteraction, ScenePointerPolicy};

/// Translate the render-free USD policy into Bevy mesh-picking behavior.
///
/// `should_block_lower = false` is the engine's native click-through behavior:
/// the marker still emits pointer events, but the vessel/rover underneath is
/// also hovered and receives the primary click. The bridge is an insertion
/// observer rather than a frame query so a recomposed or reauthored USD prim
/// cannot retain stale picking state and stable scenes do no work.
pub fn apply_pointer_policy(
    trigger: On<Insert, ScenePointerPolicy>,
    q_policy: Query<&ScenePointerPolicy>,
    mut commands: Commands,
) {
    let entity = trigger.entity;
    let Ok(policy) = q_policy.get(entity) else {
        return;
    };
    commands.entity(entity).try_insert(Pickable {
        should_block_lower: policy.left != PointerInteraction::PassThrough,
        is_hoverable: true,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_policy_stays_in_sync_when_reauthored() {
        let mut app = App::new();
        app.add_observer(apply_pointer_policy);

        let entity = app.world_mut().spawn_empty().id();
        app.world_mut()
            .entity_mut(entity)
            .insert(ScenePointerPolicy {
                left: PointerInteraction::PassThrough,
                right: PointerInteraction::Context,
            });
        app.world_mut().flush();
        assert_eq!(
            app.world().get::<Pickable>(entity),
            Some(&Pickable {
                should_block_lower: false,
                is_hoverable: true,
            })
        );

        app.world_mut()
            .entity_mut(entity)
            .insert(ScenePointerPolicy {
                left: PointerInteraction::Block,
                right: PointerInteraction::Context,
            });
        app.world_mut().flush();
        assert_eq!(
            app.world().get::<Pickable>(entity),
            Some(&Pickable {
                should_block_lower: true,
                is_hoverable: true,
            })
        );
    }
}
