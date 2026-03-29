use bevy::prelude::*;
use leafwing_input_manager::prelude::*;
use lunco_sim_core::architecture::CommandMessage;

pub struct LunCoSimControllerPlugin;

impl Plugin for LunCoSimControllerPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(InputManagerPlugin::<SpaceSystemAction>::default())
           .add_systems(Update, translate_intents_to_commands);
    }
}

#[derive(Actionlike, PartialEq, Eq, Hash, Clone, Copy, Debug, Reflect)]
pub enum SpaceSystemAction {
    DriveForward,
    DriveReverse,
    SteerLeft,
    SteerRight,
    Brake,
}

/// A marker component mapping the controller Entity (which has Leafwing) directly 
/// to the Space System root Entity (which has the Flight Software observer).
#[derive(Component)]
pub struct ControllerLink {
    pub vessel_entity: Entity,
}

/// Level 4 (Controller) translation logic.
/// Translates abstract human WASD actions into standardized FSW string intent.
fn translate_intents_to_commands(
    q_controllers: Query<(&ActionState<SpaceSystemAction>, &ControllerLink)>,
    mut commands: Commands,
) {
    for (action_state, link) in q_controllers.iter() {
        // Forward/Reverse Intent Mixing
        let mut forward_intent = 0.0;
        if action_state.pressed(&SpaceSystemAction::DriveForward) { forward_intent += 100.0; }
        if action_state.pressed(&SpaceSystemAction::DriveReverse) { forward_intent -= 100.0; }
        
        // Steering Intent Mixing
        let mut steer_intent = 0.0;
        if action_state.pressed(&SpaceSystemAction::SteerLeft) { steer_intent -= 100.0; }
        if action_state.pressed(&SpaceSystemAction::SteerRight) { steer_intent += 100.0; }

        if forward_intent != 0.0 || steer_intent != 0.0 {
            commands.trigger(CommandMessage {
                source: Entity::PLACEHOLDER,
                target: link.vessel_entity,
                name: "DRIVE_ROVER".to_string(),
                args: vec![forward_intent as f32 / 100.0, steer_intent as f32 / 100.0],
            });
        }

        if action_state.pressed(&SpaceSystemAction::Brake) {
            commands.trigger(CommandMessage {
                source: Entity::PLACEHOLDER,
                target: link.vessel_entity,
                name: "BRAKE_ROVER".to_string(),
                args: vec![],
            });
        }
    }
}
