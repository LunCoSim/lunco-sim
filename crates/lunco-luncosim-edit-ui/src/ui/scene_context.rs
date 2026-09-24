//! Pointer policies for USD-authored scene markers.

use bevy::picking::Pickable;
use bevy::picking::pointer::PointerButton;
use bevy::prelude::*;
use lunco_interaction_core::{PointerInteraction, ScenePointerPolicy};

/// The mouse button whose policy is currently used by the picking backend.
/// Bevy's `Pickable` blocking flag is shared by all pointer buttons, so the
/// editor resolves the authored per-button policy immediately before hits are
/// computed.
#[derive(Resource, Clone, Copy, PartialEq, Eq)]
pub struct ActivePointerButton(PointerButton);

impl Default for ActivePointerButton {
    fn default() -> Self {
        Self(PointerButton::Primary)
    }
}

fn interaction_for_button(policy: ScenePointerPolicy, button: PointerButton) -> PointerInteraction {
    match button {
        PointerButton::Primary => policy.left,
        PointerButton::Secondary => policy.right,
        // The authored contract currently defines primary and secondary
        // behavior. Middle-click therefore keeps the engine's blocking default.
        PointerButton::Middle => PointerInteraction::Block,
    }
}

fn active_pointer_button(buttons: &ButtonInput<MouseButton>) -> PointerButton {
    if buttons.pressed(MouseButton::Right) || buttons.just_released(MouseButton::Right) {
        PointerButton::Secondary
    } else if buttons.pressed(MouseButton::Middle) || buttons.just_released(MouseButton::Middle) {
        PointerButton::Middle
    } else {
        PointerButton::Primary
    }
}

/// Keep Bevy's generic hit ordering in sync with the active mouse gesture.
/// This runs before the backend, and only touches scene entities when the
/// active button changes; stable frames perform no scene query iteration.
pub fn sync_active_pointer_policy(
    buttons: Res<ButtonInput<MouseButton>>,
    mut active: ResMut<ActivePointerButton>,
    mut q_policy: Query<(&ScenePointerPolicy, &mut Pickable)>,
) {
    let button = active_pointer_button(&buttons);
    if active.0 == button {
        return;
    }
    active.0 = button;
    for (policy, mut pickable) in &mut q_policy {
        pickable.should_block_lower =
            interaction_for_button(*policy, button) != PointerInteraction::PassThrough;
    }
}

/// Translate the render-free USD policy into Bevy mesh-picking behavior.
///
/// `should_block_lower = false` is the engine's native click-through behavior:
/// the marker still emits pointer events, but the vessel/rover underneath is
/// also hovered and receives the primary click. The observer applies the
/// currently active button policy when a USD prim is inserted or reauthored;
/// `sync_active_pointer_policy` switches hit blocking before the next backend
/// pass when the physical mouse button changes.
pub fn apply_pointer_policy(
    trigger: On<Insert, ScenePointerPolicy>,
    q_policy: Query<&ScenePointerPolicy>,
    active: Res<ActivePointerButton>,
    mut commands: Commands,
) {
    let entity = trigger.entity;
    let Ok(policy) = q_policy.get(entity) else {
        return;
    };
    commands.entity(entity).try_insert(Pickable {
        should_block_lower: interaction_for_button(*policy, active.0)
            != PointerInteraction::PassThrough,
        is_hoverable: true,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_policy_stays_in_sync_when_reauthored() {
        let mut app = App::new();
        app.init_resource::<ActivePointerButton>();
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

    #[test]
    fn hit_blocking_follows_the_authored_button_policy_through_release() {
        let policy = ScenePointerPolicy {
            left: PointerInteraction::PassThrough,
            right: PointerInteraction::Context,
        };
        let mut app = App::new();
        app.init_resource::<ActivePointerButton>()
            .init_resource::<ButtonInput<MouseButton>>()
            .add_systems(Update, sync_active_pointer_policy);
        let entity = app
            .world_mut()
            .spawn((
                policy,
                Pickable {
                    should_block_lower: false,
                    is_hoverable: true,
                },
            ))
            .id();

        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Right);
        app.update();
        assert!(
            app.world()
                .get::<Pickable>(entity)
                .unwrap()
                .should_block_lower
        );

        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .release(MouseButton::Right);
        app.update();
        assert!(
            app.world()
                .get::<Pickable>(entity)
                .unwrap()
                .should_block_lower
        );

        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .clear();
        app.update();
        assert!(
            !app.world()
                .get::<Pickable>(entity)
                .unwrap()
                .should_block_lower
        );
        assert_eq!(
            interaction_for_button(policy, PointerButton::Middle),
            PointerInteraction::Block
        );
    }
}
