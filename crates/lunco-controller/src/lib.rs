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
        // NOTE: OwnedInputLog / AppliedInputSeq are always-on substrate owned by
        // LunCoCorePlugin (lunco-core). The controller's observers consume them
        // unconditionally, but it does NOT init them here — single source of
        // truth lives in lunco-core, which every consumer depends on.
        app.add_plugins(InputManagerPlugin::<VesselIntent>::default())
           // Input is SENSED at frame rate (leafwing's `just_pressed` latch edges
           // only work in `Update`) but EMITTED once per fixed tick, so the
           // prediction replay is a clean 1:1 loop over `InputFrame`s.
           .add_systems(Update, compute_vessel_input)
           .add_systems(FixedUpdate, emit_vessel_input);
        // The SINGLE input-bookkeeping chokepoint: every `DriveRover`/`BrakeRover`
        // — keyboard, API, or wire-replayed — flows through these observers, so the
        // client prediction log and the host reconcile-ack no longer depend on how
        // the command was produced. (Was previously split between `emit_vessel_input`
        // and `apply_sync_command`.)
        app.add_observer(record_drive_input);
        app.add_observer(record_brake_input);
    }
}

/// The single chokepoint where a [`DriveRover`] records its input bookkeeping,
/// regardless of origin (local keyboard via [`emit_vessel_input`], the HTTP/MCP
/// API, or a wire-replayed remote input). Unifying it here is what makes
/// "DriveRover and inputs go through the same path": prediction logging (client)
/// and the reconcile ack (host) no longer depend on *how* the command was made.
fn record_drive_input(
    trigger: On<DriveRover>,
    role: Res<lunco_core::NetworkRole>,
    mut owned_log: ResMut<lunco_core::OwnedInputLog>,
    mut applied: ResMut<lunco_core::AppliedInputSeq>,
    q: Query<(&lunco_core::GlobalEntityId, Has<lunco_core::OwnedLocally>)>,
) {
    let cmd = trigger.event();
    let Ok((gid, owned)) = q.get(cmd.target) else { return };
    let g = gid.get();
    if role.is_host() {
        // Host ack (moved out of `apply_sync_command`): highest applied seq per gid,
        // stamped into snapshots so the owning client can drop acked inputs.
        let slot = applied.0.entry(g).or_insert(0);
        *slot = (*slot).max(cmd.seq);
        return;
    }
    // --- Client ---
    if owned && cmd.seq != 0 {
        // Client predict (moved out of `emit_vessel_input`): buffer the frame keyed
        // by seq so `record_predicted_state` keys its pose and reconcile can prune.
        let entry = owned_log.0.entry(g).or_default();
        if entry.frames.back().map_or(true, |f| f.seq != cmd.seq) {
            entry.frames.push_back(lunco_core::InputFrame {
                seq: cmd.seq,
                tick: cmd.tick,
                forward: cmd.forward,
                steer: cmd.steer,
                // Brake rides `BrakeRover`; the frame's brake is unused by the
                // current positional reconcile (awaits true input-replay).
                brake: 0.0,
            });
            while entry.frames.len() > MAX_INPUT_FRAMES {
                entry.frames.pop_front();
            }
        }
    }
    // Prediction-membership signal (Phase A): record activity on ANY nonzero
    // setpoint, independent of `owned`/`seq`, so the first real input can bootstrap
    // prediction even while the body is still an interpolated proxy.
    // `maintain_owned_locally` predicts the body only while this tick is recent.
    if cmd.forward.abs() > INPUT_EPS || cmd.steer.abs() > INPUT_EPS {
        owned_log.0.entry(g).or_default().last_active_tick = cmd.tick;
    }
}

/// Sibling of [`record_drive_input`] for [`BrakeRover`], so a brake-only input
/// still advances the host ack (mirrors the old `DriveRover | BrakeRover` ack).
fn record_brake_input(
    trigger: On<BrakeRover>,
    role: Res<lunco_core::NetworkRole>,
    mut applied: ResMut<lunco_core::AppliedInputSeq>,
    mut owned_log: ResMut<lunco_core::OwnedInputLog>,
    q: Query<&lunco_core::GlobalEntityId>,
) {
    let cmd = trigger.event();
    let Ok(gid) = q.get(cmd.target) else { return };
    let g = gid.get();
    if role.is_host() {
        let slot = applied.0.entry(g).or_insert(0);
        *slot = (*slot).max(cmd.seq);
        return;
    }
    // Client: holding the brake (e.g. on a slope) is active control — keep the
    // vessel in the predicted set so it stays crisp (see `record_drive_input`).
    if cmd.intensity.abs() > INPUT_EPS {
        owned_log.0.entry(g).or_default().last_active_tick = cmd.tick;
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

/// The latest control setpoint computed for a controller's vessel this frame.
/// Written by [`compute_vessel_input`] (frame rate, edge-safe) and consumed by
/// [`emit_vessel_input`] (fixed tick). Decouples input *sensing* from input
/// *emission* so per-tick emission (needed for prediction replay) doesn't run in
/// `FixedUpdate`, where leafwing's `just_pressed` latch edges would misfire.
#[derive(Component, Clone, Copy, Default)]
pub struct VesselInput {
    /// Longitudinal throttle, −1..=1.
    pub forward: f64,
    /// Steering, −1..=1.
    pub steer: f64,
    /// Brake, 0..=1.
    pub brake: f64,
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
fn compute_vessel_input(
    mut q_controllers: Query<
        (Entity, &VesselIntentState, Option<&mut VesselInput>),
        With<ControllerLink>,
    >,
    keys: Res<ButtonInput<KeyCode>>,
    mut commands: Commands,
    mut latches: Local<HashMap<Entity, (f64, f64)>>,
) {
    // Ctrl = camera free-look mode: live key signal stops flowing to the
    // vessel so WASD only moves the camera, not the rover. The latch
    // (Shift-toggled setpoint) bypasses this gate — once latched, the rover
    // keeps its commanded motion regardless of Ctrl.
    let ctrl_pressed = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let shift_pressed = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);

    for (ent, intent_state, vi_opt) in q_controllers.iter_mut() {
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

        let setpoint = VesselInput {
            forward: forward_intent,
            steer: steer_intent,
            brake: brake_intent,
        };
        match vi_opt {
            Some(mut vi) => *vi = setpoint,
            None => {
                commands.entity(ent).insert(setpoint);
            }
        }
    }
}

/// Cap on the unacked input ring (~2 s at 60 Hz). The reconcile normally drains
/// it to the acked `seq` each snapshot; this only bounds a stalled/disconnected
/// client so the buffer can't grow without limit.
const MAX_INPUT_FRAMES: usize = 128;

/// Magnitude below which a control setpoint counts as "no input" for the
/// prediction-membership signal (`VesselInputLog::last_active_tick`). The
/// controller emits a `DriveRover` every fixed tick even when idle (`forward = 0`),
/// so presence of frames is NOT an activity signal — the *value* is.
const INPUT_EPS: f64 = 1e-3;

/// Fixed-tick input emission for prediction. Emits exactly one [`DriveRover`] +
/// [`BrakeRover`] per fixed tick from each controller's latest [`VesselInput`]
/// setpoint, stamped with a dense per-vessel `seq` + `SimTick`. For a vessel this
/// client owns + predicts ([`lunco_core::OwnedLocally`]), the frame is also
/// buffered in [`lunco_core::OwnedInputLog`] so the reconcile can replay it. On
/// host/standalone the commands still fire (driving the rover) with `seq = 0` and
/// no buffering.
fn emit_vessel_input(
    role: Res<lunco_core::NetworkRole>,
    tick: Res<lunco_core::SimTick>,
    mut log: ResMut<lunco_core::OwnedInputLog>,
    q_ctrl: Query<(&VesselInput, &ControllerLink)>,
    q_vessel: Query<(&lunco_core::GlobalEntityId, Has<lunco_core::OwnedLocally>)>,
    mut commands: Commands,
) {
    let client = matches!(*role, lunco_core::NetworkRole::Client);
    for (vi, link) in q_ctrl.iter() {
        // Owned + predicted on a client → assign a real seq and buffer for replay.
        let owned_gid = client
            .then(|| match q_vessel.get(link.vessel_entity) {
                Ok((gid, true)) => Some(gid.get()),
                _ => None,
            })
            .flatten();

        // Assign a dense per-vessel input seq for an owned+predicted client vessel.
        // seq MUST be stamped HERE (the origin) because the wire-capture serializes
        // the command we trigger below. The actual input-frame buffering (and the
        // host-side ack) is NOT done here any more — it happens in the single
        // `record_drive_input` observer on `DriveRover`, so a drive from ANY source
        // (this keyboard path, the API, or a wire-replayed remote input) records
        // identically. That is what keeps "DriveRover and inputs on the same path".
        let seq = if let Some(g) = owned_gid {
            let entry = log.0.entry(g).or_default();
            let s = entry.next_seq.wrapping_add(1); // seq 0 reserved = "no input yet"
            entry.next_seq = s;
            s
        } else {
            0
        };

        commands.trigger(DriveRover {
            target: link.vessel_entity,
            forward: vi.forward,
            steer: vi.steer,
            seq,
            tick: tick.0,
        });
        commands.trigger(BrakeRover {
            target: link.vessel_entity,
            intensity: vi.brake,
            seq,
            tick: tick.0,
        });
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
        (Pause, KeyCode::KeyP),
    ]);
    input_map.insert_dual_axis(Look, MouseMove::default());
    input_map.insert_axis(Zoom, MouseScrollAxis::Y);
    input_map
}

