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
    // Spec 034 yield: control authority is vessel ownership, so the human keyboard
    // drives ONLY vessels the local session owns. A vessel owned by another actor
    // (another player, or an autopilot's `AiAgent` session) is driven by that actor
    // — the human yields on a single `owner_of` lookup, no per-frame arbiter. Both
    // `Option` so a controller-only test app without the session substrate runs.
    registry: Option<Res<lunco_core::SessionRegistry>>,
    local_session: Option<Res<lunco_core::LocalSession>>,
    q_ctrl: Query<(&ControllerLink, &ActionState<UserIntent>)>,
    q_binding: Query<&ControlBinding>,
    q_vessel: Query<(&lunco_core::GlobalEntityId, Has<lunco_core::OwnedLocally>)>,
    // Self-drivers: an entity that holds its OWN input + its own binding under no
    // external controller (the free avatar). Disjoint from `q_ctrl` (which requires
    // a `ControllerLink`), so no query conflict.
    q_self: Query<(Entity, &ActionState<UserIntent>, &ControlBinding), Without<ControllerLink>>,
    // egui keyboard capture (published by `lunco-workbench`). While a text field
    // is focused we treat every intent as released so a keypress typed into the UI
    // doesn't also drive the vessel — see the `held` closure below. `Option` so a
    // controller-only test app without the workbench still runs (no gate).
    egui_focus: Option<Res<lunco_core::EguiFocus>>,
    // Per-vessel "keys were active last tick" memory for the idle-yield below.
    mut was_active: Local<std::collections::HashMap<Entity, bool>>,
    mut commands: Commands,
) {
    let client = matches!(*role, lunco_core::NetworkRole::Client);

    // When egui holds the keyboard, no local key counts as pressed. `drive_from_
    // bindings` still runs and `resolve` still writes EVERY bound port — now all
    // 0 — so the vessel decelerates to a clean stop rather than latching its last
    // command (as it would if we simply skipped the system).
    let egui_keyboard = egui_focus.map_or(false, |f| f.wants_keyboard);
    let held = |intent, intents: &ActionState<UserIntent>| !egui_keyboard && intents.pressed(&intent);

    for (link, intents) in q_ctrl.iter() {
        // Stage 1 (key→intent) is the shared leafwing `InputMap<UserIntent>`;
        // stage 2 maps this vessel's active intents → summed, clamped port writes.
        // The binding is authored ON THE VESSEL as a USD `Controls` child scope
        // (referencing a shared profile) — skip a vessel that carries none.
        let Ok(binding) = q_binding.get(link.vessel_entity) else { continue };

        // The vessel's id (gid + is-it-locally-owned) — used both by the ownership
        // yield below and the client seq bookkeeping.
        let vessel_id = q_vessel.get(link.vessel_entity).ok();

        // Spec 034 yield: if this vessel is owned by a session OTHER than ours, that
        // actor (a remote player, or an autopilot's `AiAgent` session) is the single
        // writer this tick — stay silent so the two never fight (no jitter). Owner
        // `None` (unpossessed) or our own session → we drive. When an autopilot
        // yields the vessel, ownership clears and this stops matching.
        if let (Some(reg), Some(local), Some((gid, _))) =
            (registry.as_ref(), local_session.as_ref(), vessel_id)
        {
            if reg.owner_of(gid.get()).is_some_and(|owner| owner != local.0) {
                continue;
            }
        }

        let writes = binding.resolve(|intent| held(intent, intents));

        // Owned + predicted on a client → assign a real seq (buffered for replay
        // by `record_control_input`). seq MUST be stamped HERE (the origin)
        // because the wire-capture serializes the command we trigger below.
        let owned_gid = client
            .then(|| match vessel_id {
                Some((gid, true)) => Some(gid.get()),
                _ => None,
            })
            .flatten();
        // Spec-034 scope B (idle-yield): an idle possessing human used to write
        // every bound port as 0 EVERY tick, stomping any scripted/API `SetPorts`
        // on the same vessel — the "autopilot and avatar fight" (a tutorial's
        // debug autopilot could not drive a vessel the player possessed). Go
        // SILENT in steady idle and emit exactly ONE all-zero batch on the
        // active→idle edge — ports latch, so a single zero write still gives
        // the clean stop the every-tick stream provided. A pressed key resumes
        // writing immediately: the human always preempts a script mid-drive.
        //
        // TODO(spec-034): predicted CLIENTS (`owned_gid.is_some()`) are exempt
        // and keep the OLD behaviour — an unconditional per-tick `SetPorts`
        // batch (all zeros while idle), each stamped with an incrementing
        // `seq`. That stream exists for the prediction machinery, not for
        // control: `record_control_input` buffers one `InputFrame` per seq and
        // reconcile's input-replay assumes the seq stream is CONTIGUOUS — a
        // silent gap would read as lost inputs during rollback, and the host
        // ack watermark (`AppliedInputSeq`) would stall on the last idle frame.
        // Consequence: on a client, an idle possessing human still stomps
        // scripted drive of the possessed vessel (acceptable today — scripts
        // don't co-drive client-predicted vessels). To extend idle-yield to
        // clients, change this TOGETHER with the replay side, e.g.: (a) make
        // reconcile treat a seq gap as "hold last input" instead of loss, or
        // (b) keep per-tick frames in `OwnedInputLog` without emitting port
        // writes (split bookkeeping from actuation), or (c) send explicit
        // keep-alive frames flagged `idle` that the port path ignores. Until
        // then, single-player/host gets the arbiter; the wire keeps its
        // contiguous stream.
        let active = writes.iter().any(|(_, v)| v.abs() > f64::EPSILON);
        let prev = was_active.insert(link.vessel_entity, active).unwrap_or(false);
        if !active && !prev && owned_gid.is_none() {
            continue;
        }

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

    // Self-drive (the free avatar): drive the entity's OWN command surface from its
    // OWN input via its OWN binding — the identical `SetPorts` path, no bespoke
    // avatar movement code. Local & kinematic (`apply_fly`), so no seq/tick
    // prediction bookkeeping. `resolve` writes every bound port (0 when idle), so a
    // released key zeroes the port and motion stops.
    for (entity, intents, binding) in q_self.iter() {
        let writes = binding.resolve(|intent| held(intent, intents));
        commands.trigger(lunco_cosim::SetPorts { target: entity, writes, seq: 0, tick: 0 });
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

/// The bundled default keymap DATA — key→intent bindings live here as a file
/// (`assets/config/keybindings.json`), NOT hardcoded in Rust. Embedded at compile
/// time so it works on every target with zero IO. A vessel then maps these intents
/// to its ports via its USD `Controls` profile.
const KEYBINDINGS_JSON: &str = include_str!("../../../assets/config/keybindings.json");

/// Build an avatar `InputMap<UserIntent>` from a key→intent JSON object
/// (`{"MoveForward":["KeyW"], "Action":["KeyF","Space"], …}`; keys are bevy
/// `KeyCode` variant names, intents `UserIntent` variants via
/// [`lunco_core::parse_user_intent`]). Keys starting with `_` (e.g. `_comment`)
/// and unknown intents are skipped. The mouse `Look`/`Zoom` axes are not
/// key-bindable, so they're always added in code.
pub fn build_avatar_input_map(json: &str) -> leafwing_input_manager::prelude::InputMap<lunco_core::UserIntent> {
    use leafwing_input_manager::prelude::*;
    use lunco_core::UserIntent::{Look, Zoom};

    let mut input_map = InputMap::default();
    match serde_json::from_str::<serde_json::Value>(json) {
        Ok(serde_json::Value::Object(obj)) => {
            for (name, val) in &obj {
                if name.starts_with('_') {
                    continue;
                }
                let Some(intent) = lunco_core::parse_user_intent(name) else {
                    warn!("[keybindings] unknown intent '{name}' (skipped)");
                    continue;
                };
                match serde_json::from_value::<Vec<KeyCode>>(val.clone()) {
                    Ok(keys) => {
                        for k in keys {
                            input_map.insert(intent, k);
                        }
                    }
                    Err(e) => warn!("[keybindings] '{name}' keys unparseable ({e}); skipped"),
                }
            }
        }
        _ => error!("[keybindings] keybindings.json is not a JSON object; no key bindings loaded"),
    }
    // Mouse axes — not key-bindable, always present.
    input_map.insert_dual_axis(Look, MouseMove::default());
    input_map.insert_axis(Zoom, MouseScrollAxis::Y);
    input_map
}

/// The avatar/vessel input map, built from the bundled [`KEYBINDINGS_JSON`] data
/// file. Key bindings are data, not code — edit `assets/config/keybindings.json`
/// to rebind.
pub fn get_avatar_input_map() -> leafwing_input_manager::prelude::InputMap<lunco_core::UserIntent> {
    build_avatar_input_map(KEYBINDINGS_JSON)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_core::UserIntent;

    /// The bundled keybindings file parses, every entry is a known intent bound to
    /// real `KeyCode`s, and the builder runs — guards the data file against a typo
    /// silently emptying the keymap.
    #[test]
    fn bundled_keybindings_parse_and_build() {
        let v: serde_json::Value =
            serde_json::from_str(KEYBINDINGS_JSON).expect("keybindings.json must parse");
        let obj = v.as_object().expect("keybindings.json must be an object");
        let mut bound_keys = 0;
        for (name, val) in obj {
            if name.starts_with('_') {
                continue;
            }
            assert!(
                lunco_core::parse_user_intent(name).is_some(),
                "keybindings.json names unknown intent '{name}'"
            );
            let keys: Vec<KeyCode> =
                serde_json::from_value(val.clone()).expect("intent value must be a KeyCode array");
            bound_keys += keys.len();
        }
        assert!(bound_keys >= 8, "expected the default control keys to be present");
        // Builder runs end-to-end (also adds the mouse axes) without panicking.
        let _ = get_avatar_input_map();
        let _ = UserIntent::MoveForward;
    }
}
