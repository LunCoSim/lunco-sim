//! Input mapping and controller translation for simulation vessels.
//!
//! This crate translates user input into the ONE generic vessel control command,
//! [`lunco_cosim::SetPorts`] — a batch of named input-port writes — through a
//! **two-stage, fully data-driven** mapping that reuses the existing
//! [`lunco_core::UserIntent`] input-abstraction (leafwing) rather than reading
//! raw keys:
//!
//! 1. **key → intent**: the possessed avatar's [`leafwing_input_manager`]
//!    [`InputMap<UserIntent>`](leafwing_input_manager::prelude::InputMap)
//!    (see [`get_avatar_input_map`]) turns keys/gamepad into semantic intents
//!    (`MoveForward`, `Action`, …). This is the ONLY place raw devices appear,
//!    it's shared with avatar locomotion, and — being a leafwing InputMap — it's
//!    serializable, so a saved keymap ("mapping file") rebinds every vessel.
//! 2. **intent → port** ([`ControlBinding`], per-vessel, authorable in USD/rhai):
//!    an active intent contributes `scale` to a named input port. A rover maps
//!    `MoveForward→throttle`; a cosim-flown lander maps `MoveForward→manual_pitch`.
//!    Same intent vocabulary, different actuation — no vessel-kind branch.
//!
//! [`drive_from_bindings`] composes the two every fixed tick (the controller's
//! [`ActionState<UserIntent>`](leafwing_input_manager::prelude::ActionState) →
//! summed port writes → `SetPorts`). Because control is keyed by *intent*,
//! anything internal (rhai, mission logic, AI) can drive a vessel by naming
//! intents — the same consistent vocabulary. All writes land through the same
//! [`lunco_core::ports::PortRegistry`].

use bevy::prelude::*;
use leafwing_input_manager::prelude::ActionState;
use lunco_core::UserIntent;

/// Plugin for managing vessel input and command translation.
pub struct LunCoControllerPlugin;

impl Plugin for LunCoControllerPlugin {
    fn build(&self, app: &mut App) {
        // NOTE: OwnedInputLog / AppliedInputSeq are always-on substrate owned by
        // LunCoCorePlugin (lunco-core). The controller's observers consume them
        // unconditionally, but it does NOT init them here — single source of
        // truth lives in lunco-core, which every consumer depends on.
        //
        // Input → port writes are EMITTED once per fixed tick (so the
        // prediction replay is a clean 1:1 loop over `InputFrame`s).
        app.add_systems(FixedUpdate, drive_from_bindings);
        // The SINGLE input-bookkeeping chokepoint: every `SetPorts` — keyboard,
        // API, or wire-replayed — flows through this observer, so the client
        // prediction log and the host reconcile-ack no longer depend on how the
        // command was produced.
        app.add_observer(record_control_input);
    }
}

/// A marker component mapping the controller Entity directly
/// to the Space System root Entity (the focus of the control).
#[derive(Component)]
pub struct ControllerLink {
    /// The entity representing the vehicle or vessel to be controlled.
    pub vessel_entity: Entity,
}

/// The per-vessel **intent → port** binding (stage 2) is [`lunco_core::ControlBinding`]
/// — pure data, authored on the VESSEL from USD (`lunco:controlBindings`) or
/// defaulted by topology at possess time. Re-exported for the possession code and
/// tests; the actual mapping/parse logic lives in `lunco-core` alongside
/// [`UserIntent`]. This crate only provides the SYSTEM that consumes it
/// ([`drive_from_bindings`]).
pub use lunco_core::ControlBinding;

/// Cap on the unacked input ring (~2 s at 60 Hz). The reconcile normally drains
/// it to the acked `seq` each snapshot; this only bounds a stalled/disconnected
/// client so the buffer can't grow without limit.
const MAX_INPUT_FRAMES: usize = 128;

/// Magnitude below which a control setpoint counts as "no input" for the
/// prediction-membership signal (`VesselInputLog::last_active_tick`). The
/// controller emits a `SetPorts` every fixed tick even when idle (all zeros), so
/// presence of writes is NOT an activity signal — the *value* is.
const INPUT_EPS: f64 = 1e-3;

/// Fixed-tick input emission for prediction. Emits exactly one [`lunco_cosim::SetPorts`]
/// per fixed tick per controller, from its [`ControlBinding`] and the currently
/// held keys, stamped with a dense per-vessel `seq` + `SimTick`. Every bound port
/// name is written every tick (0 on release) so setpoints zero out cleanly. For a
/// vessel this client owns + predicts ([`lunco_core::OwnedLocally`]) the frame is
/// buffered for replay by the [`record_control_input`] observer; on host/standalone
/// the command still fires with `seq = 0`.
fn drive_from_bindings(
    role: Res<lunco_core::NetworkRole>,
    tick: Res<lunco_core::SimTick>,
    mut log: ResMut<lunco_core::OwnedInputLog>,
    q_ctrl: Query<(&ControllerLink, &ActionState<UserIntent>)>,
    q_binding: Query<&ControlBinding>,
    q_vessel: Query<(&lunco_core::GlobalEntityId, Has<lunco_core::OwnedLocally>)>,
    mut commands: Commands,
) {
    let client = matches!(*role, lunco_core::NetworkRole::Client);

    for (link, intents) in q_ctrl.iter() {
        // Stage 1 (key→intent) is the shared leafwing `InputMap<UserIntent>`;
        // stage 2 maps this vessel's active intents → summed, clamped port writes.
        // The binding is authored ON THE VESSEL (USD `lunco:controlBindings`, or a
        // topology default stamped at possess) — skip a vessel that carries none.
        let Ok(binding) = q_binding.get(link.vessel_entity) else { continue };
        let writes = binding.resolve(|intent| intents.pressed(&intent));

        // Owned + predicted on a client → assign a real seq (buffered for replay
        // by `record_control_input`). seq MUST be stamped HERE (the origin)
        // because the wire-capture serializes the command we trigger below.
        let owned_gid = client
            .then(|| match q_vessel.get(link.vessel_entity) {
                Ok((gid, true)) => Some(gid.get()),
                _ => None,
            })
            .flatten();
        let seq = if let Some(g) = owned_gid {
            let entry = log.0.entry(g).or_default();
            let s = entry.next_seq.wrapping_add(1); // seq 0 reserved = "no input yet"
            entry.next_seq = s;
            s
        } else {
            0
        };

        commands.trigger(lunco_cosim::SetPorts {
            target: link.vessel_entity,
            writes,
            seq,
            tick: tick.0,
        });
    }
}

/// The single chokepoint where a [`lunco_cosim::SetPorts`] records its input
/// bookkeeping, regardless of origin (local keyboard via [`drive_from_bindings`],
/// the HTTP/MCP API, or a wire-replayed remote input). Unifying it here is what
/// keeps control and prediction on the same path: prediction logging (client) and
/// the reconcile ack (host) no longer depend on *how* the command was made.
fn record_control_input(
    trigger: On<lunco_cosim::SetPorts>,
    role: Res<lunco_core::NetworkRole>,
    mut owned_log: ResMut<lunco_core::OwnedInputLog>,
    mut applied: ResMut<lunco_core::AppliedInputSeq>,
    q: Query<(&lunco_core::GlobalEntityId, Has<lunco_core::OwnedLocally>)>,
) {
    let cmd = trigger.event();
    let Ok((gid, owned)) = q.get(cmd.target) else { return };
    let g = gid.get();
    if role.is_host() {
        // Host ack: highest applied seq per gid, stamped into snapshots so the
        // owning client can drop acked inputs.
        let slot = applied.0.entry(g).or_insert(0);
        *slot = (*slot).max(cmd.seq);
        return;
    }
    // --- Client ---
    if owned && cmd.seq != 0 {
        // Buffer the frame keyed by seq so `record_predicted_state` keys its pose
        // and reconcile can prune. The forward/steer/brake payload is unused by
        // the current positional reconcile (awaits true input-replay).
        let entry = owned_log.0.entry(g).or_default();
        if entry.frames.back().map_or(true, |f| f.seq != cmd.seq) {
            entry.frames.push_back(lunco_core::InputFrame {
                seq: cmd.seq,
                tick: cmd.tick,
                forward: 0.0,
                steer: 0.0,
                brake: 0.0,
            });
            while entry.frames.len() > MAX_INPUT_FRAMES {
                entry.frames.pop_front();
            }
        }
    }
    // Prediction-membership signal (Phase A): record activity on ANY nonzero
    // write, independent of `owned`/`seq`, so the first real input can bootstrap
    // prediction even while the body is still an interpolated proxy.
    if cmd.writes.iter().any(|(_, v)| v.abs() > INPUT_EPS) {
        owned_log.0.entry(g).or_default().last_active_tick = cmd.tick;
    }
}

/// Provides a standard WASD + EQ + Space mapping for generic avatar movement.
pub fn get_avatar_input_map() -> leafwing_input_manager::prelude::InputMap<lunco_core::UserIntent> {
    use leafwing_input_manager::prelude::*;
    use lunco_core::UserIntent::*;
    let mut input_map = InputMap::new([
        (MoveForward, KeyCode::KeyW),
        (MoveBackward, KeyCode::KeyS),
        (MoveLeft, KeyCode::KeyA),
        (MoveRight, KeyCode::KeyD),
        (MoveUp, KeyCode::KeyE),
        (MoveDown, KeyCode::KeyQ),
        (Action, KeyCode::KeyF),
        // Space also fires `Action`: for a possessed vessel that's brake (rover)
        // / arm-manual+full-throttle (lander) via its `ControlBinding`; for a
        // free avatar it's the same context interaction as F.
        (Action, KeyCode::Space),
        (SwitchMode, KeyCode::KeyV),
        (Pause, KeyCode::KeyP),
    ]);
    input_map.insert_dual_axis(Look, MouseMove::default());
    input_map.insert_axis(Zoom, MouseScrollAxis::Y);
    input_map
}
