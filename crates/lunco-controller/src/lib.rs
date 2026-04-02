use bevy::prelude::*;
use leafwing_input_manager::prelude::*;
use lunco_core::architecture::CommandMessage;

pub struct LunCoControllerPlugin;

impl Plugin for LunCoControllerPlugin {
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
    keys: Res<ButtonInput<KeyCode>>,
    mut commands: Commands,
    mut last_intents: Local<Option<(f32, f32, f32)>>,
) {
    let ctrl_pressed = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);

    for (action_state, link) in q_controllers.iter() {
        // Forward/Reverse Intent Mixing (Inhibited by CTRL)
        let mut forward_intent = 0.0;
        if !ctrl_pressed {
            if action_state.pressed(&SpaceSystemAction::DriveForward) { forward_intent += 1.0; }
            if action_state.pressed(&SpaceSystemAction::DriveReverse) { forward_intent -= 1.0; }
        }
        
        // Steering Intent Mixing (Inhibited by CTRL)
        let mut steer_intent = 0.0;
        if !ctrl_pressed {
            if action_state.pressed(&SpaceSystemAction::SteerLeft) { steer_intent -= 1.0; }
            if action_state.pressed(&SpaceSystemAction::SteerRight) { steer_intent += 1.0; }
        }

        // Brake Intent (Stateful, Inhibited by CTRL)
        let brake_intent = if !ctrl_pressed && action_state.pressed(&SpaceSystemAction::Brake) { 1.0 } else { 0.0 };

        let current = (forward_intent, steer_intent, brake_intent);
        if last_intents.map_or(true, |last| last != current) {
            // DRIVE_ROVER (includes steering)
            commands.trigger(CommandMessage {
                source: Entity::PLACEHOLDER,
                target: link.vessel_entity,
                name: "DRIVE_ROVER".to_string(),
                args: vec![forward_intent, steer_intent],
            });

            // BRAKE_ROVER (Refined to pass duty/state)
            commands.trigger(CommandMessage {
                source: Entity::PLACEHOLDER,
                target: link.vessel_entity,
                name: "BRAKE_ROVER".to_string(),
                args: vec![brake_intent],
            });

            *last_intents = Some(current);
        }
    }
}

pub fn get_default_input_map() -> InputMap<SpaceSystemAction> {
    use SpaceSystemAction::*;
    InputMap::new([
        (DriveForward, KeyCode::KeyW),
        (DriveReverse, KeyCode::KeyS),
        (SteerLeft, KeyCode::KeyA),
        (SteerRight, KeyCode::KeyD),
        (Brake, KeyCode::Space),
    ])
}
