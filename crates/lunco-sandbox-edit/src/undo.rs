//! Undo system for sandbox editing operations.

use bevy::prelude::*;

/// Stack of undoable actions.
#[derive(Resource)]
pub struct UndoStack {
    actions: Vec<UndoAction>,
    max_depth: usize,
}

impl Default for UndoStack {
    fn default() -> Self {
        Self {
            actions: Vec::new(),
            max_depth: 100,
        }
    }
}

/// An undoable operation.
#[derive(Clone, Debug)]
pub enum UndoAction {
    /// An entity was spawned. Undo = despawn.
    Spawned { entity: Entity },
    /// An entity's transform was changed. Undo = restore old transform.
    TransformChanged {
        entity: Entity,
        old_translation: Vec3,
        old_rotation: Quat,
    },
}

impl UndoStack {
    /// Push an action onto the stack.
    pub fn push(&mut self, action: UndoAction) {
        self.actions.push(action);
        if self.actions.len() > self.max_depth {
            self.actions.drain(..self.actions.len() - self.max_depth);
        }
    }

    /// Check if the stack has actions to undo.
    pub fn can_undo(&self) -> bool {
        !self.actions.is_empty()
    }

    /// Clear the undo stack.
    pub fn clear(&mut self) {
        self.actions.clear();
    }
}

/// Handles Ctrl+Z input to undo the last action.
pub fn handle_undo_input(
    keys: Res<ButtonInput<KeyCode>>,
    mut undo_stack: ResMut<UndoStack>,
    mut commands: Commands,
    q_children: Query<&Children>,
    mut q_transforms: Query<&mut Transform>,
) {
    if keys.just_pressed(KeyCode::KeyZ)
        && (keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight))
    {
        let Some(action) = undo_stack.actions.pop() else {
            info!("Nothing to undo");
            return;
        };

        match action {
            UndoAction::Spawned { entity } => {
                despawn_recursive(entity, &mut commands, &q_children);
                info!("Undo: despawned entity {:?}", entity);
            }
            UndoAction::TransformChanged { entity, old_translation, old_rotation } => {
                if let Ok(mut tf) = q_transforms.get_mut(entity) {
                    tf.translation = old_translation;
                    tf.rotation = old_rotation;
                    info!("Undo: restored transform for entity {:?}", entity);
                }
            }
        }
        info!("Undo performed");
    }
}

/// Recursively despawns an entity and all its children.
fn despawn_recursive(
    entity: Entity,
    commands: &mut Commands,
    q_children: &Query<&Children>,
) {
    if let Ok(children) = q_children.get(entity) {
        let child_list: Vec<Entity> = children.iter().collect();
        for child in child_list {
            despawn_recursive(child, commands, q_children);
        }
    }
    if commands.get_entity(entity).is_ok() {
        commands.entity(entity).despawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_undo_stack_empty() {
        let stack = UndoStack::default();
        assert!(!stack.can_undo());
    }

    #[test]
    fn test_undo_push_and_clear() {
        let mut stack = UndoStack::default();
        stack.push(UndoAction::Spawned { entity: Entity::PLACEHOLDER });
        assert!(stack.can_undo());
        stack.clear();
        assert!(!stack.can_undo());
    }

    #[test]
    fn test_undo_max_depth() {
        let mut stack = UndoStack {
            actions: Vec::new(),
            max_depth: 5,
        };
        for i in 0..10u32 {
            stack.push(UndoAction::Spawned { entity: Entity::PLACEHOLDER });
        }
        assert_eq!(stack.actions.len(), 5);
    }
}
