//! Input mapping and controller translation for simulation vessels.
//!
//! This crate translates raw user input (Keyboard, Gamepad) into 
//! high-level [CommandMessage] events that the Flight Software can consume.
//! It abstracts the UI/Input layer from the simulation core.

use bevy::prelude::*;
use leafwing_input_manager::prelude::*;
use lunco_core::architecture::CommandMessage;
use smallvec::smallvec;
use std::collections::HashMap;

/// Plugin for managing vessel input and command translation.
pub struct LunCoControllerPlugin;

impl Plugin for LunCoControllerPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(InputManagerPlugin::<VesselIntent>::default())
           .add_systems(Update, translate_intents_to_commands);
    }
}

/// Abstract intents specifically for controlling a vessel's movement.
#[derive(Actionlike, PartialEq, Eq, Hash, Clone, Copy, Debug, Reflect)]
pub enum VesselIntent {
    /// Request forward longitudinal movement.
    DriveForward,
    /// Request backward longitudinal movement.
    DriveReverse,
    /// Request lateral rotation to the left.
    SteerLeft,
    /// Request lateral rotation to the right.
    SteerRight,
    /// Request activation of the braking system.
    Brake,
}

/// Alias for [ActionState] specialized for [VesselIntent].
pub type VesselIntentState = ActionState<VesselIntent>;

/// A marker component mapping the controller Entity directly 
/// to the Space System root Entity (the focus of the control).
#[derive(Component)]
pub struct ControllerLink {
    /// The entity representing the vehicle or vessel to be controlled.
    pub vessel_entity: Entity,
}

/// Translates abstract human WASD actions into standardized [CommandMessage] events.
///
/// This system implements the 'Level 4' Controller logic, mixing various
/// intents (like Forward + Left) into single [CommandMessage] packets.
///
/// **Why `last_intents` per-entity**: When possession switches to a different
/// vessel, the intent state is fresh but `last_intents` still holds stale
/// values from the previous vessel. Tracking per-controller-entity ensures
/// the new vessel responds immediately to held keys.
fn translate_intents_to_commands(
    q_controllers: Query<(Entity, &VesselIntentState, &ControllerLink)>,
    keys: Res<ButtonInput<KeyCode>>,
    mut commands: Commands,
    mut last_intents: Local<HashMap<Entity, (f64, f64, f64)>>,
    mut id_counter: Local<u64>,
) {
    let ctrl_pressed = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);

    for (ent, intent_state, link) in q_controllers.iter() {
        // Forward/Reverse Intent Mixing (Inhibited by CTRL to ensure keys aren't
        // processed twice if used for other hotkeys).
        let mut forward_intent = 0.0;
        if !ctrl_pressed {
            if intent_state.pressed(&VesselIntent::DriveForward) { forward_intent += 1.0; }
            if intent_state.pressed(&VesselIntent::DriveReverse) { forward_intent -= 1.0; }
        }

        // Steering Intent Mixing.
        let mut steer_intent = 0.0;
        if !ctrl_pressed {
            if intent_state.pressed(&VesselIntent::SteerLeft) { steer_intent -= 1.0; }
            if intent_state.pressed(&VesselIntent::SteerRight) { steer_intent += 1.0; }
        }

        // Brake Intent.
        let brake_intent = if !ctrl_pressed && intent_state.pressed(&VesselIntent::Brake) { 1.0 } else { 0.0 };

        let current = (forward_intent, steer_intent, brake_intent);
        let prev = last_intents.get(&ent).copied();
        if prev.map_or(true, |last| last != current) {
            // DRIVE_ROVER command: Mixed longitudinal and lateral intent.
            *id_counter += 1;
            commands.trigger(CommandMessage {
                id: *id_counter,
                source: Entity::PLACEHOLDER,
                target: link.vessel_entity,
                name: "DRIVE_ROVER".to_string(),
                args: smallvec![forward_intent, steer_intent],
            });

            // BRAKE_ROVER command: Explicit braking request.
            *id_counter += 1;
            commands.trigger(CommandMessage {
                id: *id_counter,
                source: Entity::PLACEHOLDER,
                target: link.vessel_entity,
                name: "BRAKE_ROVER".to_string(),
                args: smallvec![brake_intent],
            });

            last_intents.insert(ent, current);
        }
    }
}

/// Provides a standard WASD + Space mapping for vessel control.
pub fn get_default_input_map() -> InputMap<VesselIntent> {
    use VesselIntent::*;
    InputMap::new([
        (DriveForward, KeyCode::KeyW),
        (DriveReverse, KeyCode::KeyS),
        (SteerLeft, KeyCode::KeyA),
        (SteerRight, KeyCode::KeyD),
        (Brake, KeyCode::Space),
    ])
}

/// Provides a standard WASD + EQ + Space mapping for generic avatar movement.
pub fn get_avatar_input_map() -> InputMap<lunco_core::UserIntent> {
    use lunco_core::UserIntent::*;
    let mut input_map = InputMap::new([
        (MoveForward, KeyCode::KeyW),
        (MoveBackward, KeyCode::KeyS),
        (MoveLeft, KeyCode::KeyA),
        (MoveRight, KeyCode::KeyD),
        (MoveUp, KeyCode::KeyE),
        (MoveDown, KeyCode::KeyQ),
        (Action, KeyCode::KeyF),
        (SwitchMode, KeyCode::KeyV),
        (Pause, KeyCode::Space),
    ]);
    input_map.insert_dual_axis(Look, MouseMove::default());
    input_map.insert_axis(Zoom, MouseScrollAxis::Y);
    input_map
}

