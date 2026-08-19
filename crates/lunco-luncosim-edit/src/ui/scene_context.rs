//! Pointer policies for USD-authored scene markers.

use bevy::picking::Pickable;
use bevy::prelude::*;
use lunco_core::{PointerInteraction, ScenePointerPolicy};

/// Translate the render-free USD policy into Bevy mesh-picking behavior.
///
/// `should_block_lower = false` is the engine's native click-through behavior:
/// the marker still emits pointer events, but the vessel/rover underneath is
/// also hovered and receives the primary click.  This is intentionally a
/// change-driven system; interaction policy is authored topology, not a
/// per-frame computation.
pub fn apply_pointer_policies(
    mut commands: Commands,
    q_policy: Query<(Entity, &ScenePointerPolicy), Added<ScenePointerPolicy>>,
) {
    for (entity, policy) in q_policy.iter() {
        commands.entity(entity).try_insert(Pickable {
            should_block_lower: policy.left != PointerInteraction::PassThrough,
            is_hoverable: true,
        });
    }
}
