//! Input mapping and controller translation for simulation vessels.
//!
//! This crate translates raw user input (Keyboard) into the ONE generic vessel
//! control command, [`lunco_cosim::SetPorts`] — a batch of named input-port
//! writes. It abstracts the UI/Input layer from the simulation core: a possessed
//! vessel carries a [`ControlBinding`] declaring which key contributes what to
//! which named port, and [`drive_from_bindings`] turns held keys into port writes
//! every fixed tick. There is no vessel-kind vocabulary — a rover binds
//! `throttle`/`steer`/`brake`, a cosim-flown lander binds its Modelica `manual_*`
//! inputs, both through the same command and the same [`lunco_core::ports::PortRegistry`].

use bevy::prelude::*;
use std::collections::HashMap;

/// Plugin for managing vessel input and command translation.
pub struct LunCoControllerPlugin;

impl Plugin for LunCoControllerPlugin {
    fn build(&self, app: &mut App) {
        // NOTE: OwnedInputLog / AppliedInputSeq are always-on substrate owned by
        // LunCoCorePlugin (lunco-core). The controller's observers consume them
        // unconditionally, but it does NOT init them here — single source of
        // truth lives in lunco-core, which every consumer depends on.
        //
        // Keyboard → port writes are EMITTED once per fixed tick (so the
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

/// Keyboard→port binding for a possessed vessel: while a key is pressed, it
/// contributes `value` to the named input port. Multiple entries may name the
/// same key (e.g. Space arming both `manual` and `manual_throttle`) or the same
/// port (e.g. W/S summing into `throttle`). Selected by TOPOLOGY at possess time
/// ([`rover_binding`] vs [`flight_binding`]) — see `lunco_avatar::on_possess_command`.
#[derive(Component)]
pub struct ControlBinding {
    /// `(key, port_name, contribution)` — each held key adds its contribution to
    /// the port; contributions to one port are summed then clamped to [-1, 1].
    pub binds: Vec<(KeyCode, String, f64)>,
}

impl ControlBinding {
    /// Wheeled rover: W/S → `throttle`, A/D → `steer`, Space → `brake`.
    pub fn rover_binding() -> ControlBinding {
        ControlBinding {
            binds: vec![
                (KeyCode::KeyW, "throttle".into(), 1.0),
                (KeyCode::KeyS, "throttle".into(), -1.0),
                (KeyCode::KeyA, "steer".into(), -1.0),
                (KeyCode::KeyD, "steer".into(), 1.0),
                (KeyCode::Space, "brake".into(), 1.0),
            ],
        }
    }

    /// Cosim-flown lander: W/S → pitch, A/D → roll, Q/E → yaw, Space arms manual
    /// mode AND fires full throttle (`manual` + `manual_throttle`, mirroring
    /// `Lander.mo`). Port names match the Modelica `SimComponent.inputs`.
    pub fn flight_binding() -> ControlBinding {
        ControlBinding {
            binds: vec![
                (KeyCode::KeyW, "manual_pitch".into(), -1.0),
                (KeyCode::KeyS, "manual_pitch".into(), 1.0),
                (KeyCode::KeyA, "manual_roll".into(), 1.0),
                (KeyCode::KeyD, "manual_roll".into(), -1.0),
                (KeyCode::KeyQ, "manual_yaw".into(), 1.0),
                (KeyCode::KeyE, "manual_yaw".into(), -1.0),
                (KeyCode::Space, "manual".into(), 1.0),
                (KeyCode::Space, "manual_throttle".into(), 1.0),
            ],
        }
    }
}

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
    keys: Res<ButtonInput<KeyCode>>,
    q_ctrl: Query<(&ControllerLink, &ControlBinding)>,
    q_vessel: Query<(&lunco_core::GlobalEntityId, Has<lunco_core::OwnedLocally>)>,
    mut commands: Commands,
) {
    let client = matches!(*role, lunco_core::NetworkRole::Client);
    for (link, binding) in q_ctrl.iter() {
        // Sum each bound key's contribution into its port; keep a 0.0 entry for
        // every bound port so a released key writes 0 (clears the setpoint).
        let mut values: HashMap<String, f64> = HashMap::new();
        for (_key, port, _v) in &binding.binds {
            values.entry(port.clone()).or_insert(0.0);
        }
        for (key, port, v) in &binding.binds {
            if keys.pressed(*key) {
                *values.get_mut(port).unwrap() += *v;
            }
        }
        let writes: Vec<(String, f64)> = values
            .into_iter()
            .map(|(name, v)| (name, v.clamp(-1.0, 1.0)))
            .collect();

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
        (SwitchMode, KeyCode::KeyV),
        (Pause, KeyCode::KeyP),
    ]);
    input_map.insert_dual_axis(Look, MouseMove::default());
    input_map.insert_axis(Zoom, MouseScrollAxis::Y);
    input_map
}
