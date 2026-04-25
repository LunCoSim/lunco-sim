//! Input mapping and controller translation for simulation vessels.
//!
//! This crate translates raw user input (Keyboard, Gamepad) into
//! typed command events that the Flight Software can consume.
//! It abstracts the UI/Input layer from the simulation core.

use bevy::prelude::*;
use leafwing_input_manager::prelude::*;
use lunco_mobility::{DriveRover, BrakeRover};
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

/// Translates abstract human WASD actions into typed command events.
///
/// This system implements the 'Level 4' Controller logic, mixing various
/// intents (like Forward + Left) into typed command structs.
///
/// **Latch (cruise control)**: `Shift + W/S/A/D` toggles a sticky setpoint on
/// that axis. While latched, the rover keeps driving/steering hands-off so you
/// can hold `Ctrl` to detach the camera and inspect rover behaviour. Re-tap
/// the same `Shift+key` to release, or press `Space` (brake) to clear all.
fn translate_intents_to_commands(
    q_controllers: Query<(Entity, &VesselIntentState, &ControllerLink)>,
    keys: Res<ButtonInput<KeyCode>>,
    mut commands: Commands,
    mut last_intents: Local<HashMap<Entity, (f64, f64, f64)>>,
    mut latches: Local<HashMap<Entity, (f64, f64)>>,
) {
    // Ctrl = camera free-look mode: live key signal stops flowing to the
    // vessel so WASD only moves the camera, not the rover. The latch
    // (Shift-toggled setpoint) bypasses this gate — once latched, the rover
    // keeps its commanded motion regardless of Ctrl.
    let ctrl_pressed = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let shift_pressed = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);

    for (ent, intent_state, link) in q_controllers.iter() {
        let latch = latches.entry(ent).or_insert((0.0, 0.0));

        // Shift + axis key toggles a latched setpoint on that axis.
        // Re-tapping the same direction clears it; the opposite direction
        // overrides the sign.
        if shift_pressed {
            if intent_state.just_pressed(&VesselIntent::DriveForward) {
                latch.0 = if latch.0 ==  1.0 { 0.0 } else {  1.0 };
            }
            if intent_state.just_pressed(&VesselIntent::DriveReverse) {
                latch.0 = if latch.0 == -1.0 { 0.0 } else { -1.0 };
            }
            if intent_state.just_pressed(&VesselIntent::SteerLeft) {
                latch.1 = if latch.1 == -1.0 { 0.0 } else { -1.0 };
            }
            if intent_state.just_pressed(&VesselIntent::SteerRight) {
                latch.1 = if latch.1 ==  1.0 { 0.0 } else {  1.0 };
            }
        }

        // Brake always clears latches — emergency stop.
        if intent_state.pressed(&VesselIntent::Brake) {
            *latch = (0.0, 0.0);
        }

        // Live keys add on top of the latch. Gated by:
        //   - Shift: would double-fire alongside the latch toggle.
        //   - Ctrl: free-look mode, signal must not flow to the vessel.
        // The latch itself (latch.0/.1) is read unconditionally — Shift+D
        // sets a setpoint that survives both modifiers.
        let mut forward_intent = latch.0;
        let mut steer_intent = latch.1;
        if !shift_pressed && !ctrl_pressed {
            if intent_state.pressed(&VesselIntent::DriveForward) { forward_intent += 1.0; }
            if intent_state.pressed(&VesselIntent::DriveReverse) { forward_intent -= 1.0; }
            if intent_state.pressed(&VesselIntent::SteerLeft) { steer_intent -= 1.0; }
            if intent_state.pressed(&VesselIntent::SteerRight) { steer_intent += 1.0; }
        }
        forward_intent = forward_intent.clamp(-1.0, 1.0);
        steer_intent = steer_intent.clamp(-1.0, 1.0);

        let brake_intent = if intent_state.pressed(&VesselIntent::Brake) { 1.0 } else { 0.0 };

        let current = (forward_intent, steer_intent, brake_intent);
        let prev = last_intents.get(&ent).copied();
        if prev.map_or(true, |last| last != current) {
            commands.trigger(DriveRover {
                target: link.vessel_entity,
                forward: forward_intent,
                steer: steer_intent,
            });

            commands.trigger(BrakeRover {
                target: link.vessel_entity,
                intensity: brake_intent,
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

