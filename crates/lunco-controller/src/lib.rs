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
use lunco_core::{on_command, register_commands, Command, UserIntent};

/// Intents forced held by [`SimulateIntent`], **keyed by the vessel they drive** —
/// a headless stand-in for the keyboard.
///
/// `drive_from_bindings` treats a member exactly as a held key: it OR's into the
/// `held` test, so a simulated intent flows through the SAME two-stage binding path a
/// real keypress does (intent → `ControlBinding` → `SetPorts`). This is how a test, a
/// script, or the API drives a possessed vessel with no physical keyboard.
///
/// **Per-entity, not global.** A held intent is addressed to the ONE vessel it
/// controls. This used to be a bare `HashSet<UserIntent>` consulted for every vessel
/// `drive_from_bindings` iterated, so a single simulated press drove EVERY controlled
/// vessel at once: two spawns of the same asset (two landers — byte-identical prim
/// paths, distinct entities) could not be flown independently, and "control" meant
/// "whatever happens to be possessed". Keying by the vessel entity makes the signal
/// name its subject, exactly as the wire endpoints do via `GlobalEntityId`.
#[derive(Resource, Default)]
pub struct SimulatedIntents(
    pub std::collections::HashMap<Entity, std::collections::HashSet<UserIntent>>,
);

/// Force an intent held or released, as if a key were pressed — the headless way to
/// drive a possessed vessel over the API or from rhai.
///
/// `held = true` is "stuck" (the key is down and stays down); `held = false` is
/// "unstuck" (released). A momentary "one" press is `held:true` then `held:false`.
/// The named intent is the USD control vocabulary (`forward`, `action`, `yaw_left`,
/// …), parsed by [`lunco_core::parse_user_intent`], so it matches whatever a vessel's
/// `Controls` profile binds.
#[Command]
pub struct SimulateIntent {
    /// Intent name (`forward`, `backward`, `left`, `right`, `yaw_left`, `yaw_right`,
    /// `action`, `release`, …).
    pub intent: String,
    /// `true` = hold it down, `false` = release it.
    pub held: bool,
    /// The **vessel this intent drives**. An intent is meaningless without the thing
    /// it controls: two spawns of one asset are two distinct vessels, and a targetless
    /// intent drove both (see [`SimulatedIntents`]). Over the API this takes the
    /// vessel's `api_id` — the `GlobalEntityId` `ListEntities` reports, the same
    /// identity the cosim wires resolve by — and is resolved to the live entity.
    pub target: Entity,
}

impl Default for SimulateIntent {
    fn default() -> Self {
        Self { intent: String::new(), held: false, target: Entity::PLACEHOLDER }
    }
}

#[on_command(SimulateIntent)]
fn on_simulate_intent(
    trigger: On<SimulateIntent>,
    mut sim: ResMut<SimulatedIntents>,
) {
    let cmd = trigger.event();
    let Some(intent) = lunco_core::parse_user_intent(&cmd.intent) else {
        warn!("[simulate-intent] unknown intent '{}'", cmd.intent);
        return;
    };
    // No target = no subject. Refuse rather than fall back to "every vessel": a
    // silent broadcast is what made two landers fly as one.
    if cmd.target == Entity::PLACEHOLDER {
        warn!(
            "[simulate-intent] '{}' names no `target` vessel — an intent must name the \
             entity it drives (pass the vessel's api_id); ignoring",
            cmd.intent
        );
        return;
    }
    if cmd.held {
        sim.0.entry(cmd.target).or_default().insert(intent);
    } else if let Some(set) = sim.0.get_mut(&cmd.target) {
        set.remove(&intent);
        // Don't leak an empty set per vessel ever simulated.
        if set.is_empty() {
            sim.0.remove(&cmd.target);
        }
    }
    info!(
        "[simulate-intent] {} → {} on {:?}",
        cmd.intent,
        if cmd.held { "HELD" } else { "released" },
        cmd.target
    );
}

register_commands!(on_simulate_intent);

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
        // Suppressed during a rollback replay: re-simulation feeds the RECORDED
        // input for each replayed tick, so regenerating input from the live keyboard
        // mid-replay would overwrite the very history we are replaying (and mint new
        // seqs for ticks that already happened).
        app.init_resource::<SimulatedIntents>();
        register_all_commands(app);
        app.add_systems(
            FixedUpdate,
            drive_from_bindings.run_if(lunco_core::not_rolling_back),
        );
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
    // Intents forced by `SimulateIntent` — the headless/API/rhai stand-in for keys.
    sim_intents: Option<Res<SimulatedIntents>>,
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
    let sim_intents = sim_intents.as_deref();
    // A simulated intent counts as held regardless of the egui gate (it is not a
    // physical key that a focused text field could be swallowing).
    //
    // Scoped to `vessel`: a simulated intent drives ONLY the vessel it was addressed
    // to. The keyboard half stays per-vessel too — it always was, via that vessel's
    // own `ActionState`. Before, the sim half was a global set consulted inside this
    // same loop, so one `SimulateIntent` pressed the key on EVERY controlled vessel.
    let held = |vessel: Entity, intent, intents: &ActionState<UserIntent>| {
        sim_intents
            .is_some_and(|s| s.0.get(&vessel).is_some_and(|set| set.contains(&intent)))
            || (!egui_keyboard && intents.pressed(&intent))
    };

    for (link, intents) in q_ctrl.iter() {
        // Stage 1 (key→intent) is the shared leafwing `InputMap<UserIntent>`;
        // stage 2 maps this vessel's active intents → summed, clamped port writes.
        // The binding is authored ON THE VESSEL as a USD `Controls` child scope
        // (referencing a shared profile) — skip a vessel that carries none.
        let Ok(binding) = q_binding.get(link.vessel_entity) else {
            continue;
        };

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
            let owner = reg.owner_of(gid.get());
            if owner.is_some_and(|owner| owner != local.0) {
                continue;
            }
        }

        let writes = binding.resolve(|intent| held(link.vessel_entity, intent, intents));

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
        // A self-driver IS its own vessel, so it is its own intent subject.
        let writes = binding.resolve(|intent| held(entity, intent, intents));
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
    sim_tick: Res<lunco_core::SimTick>,
    mut owned_log: ResMut<lunco_core::OwnedInputLog>,
    mut applied: ResMut<lunco_core::AppliedInputSeq>,
    // Latest local drive input per gid — the render-lead reads it to visually
    // anticipate the rover's motion (presentational only; see `LocalDriveInput`).
    mut drive_input: ResMut<lunco_core::LocalDriveInput>,
    // Host-side per-tick input buffer + ownership table: a forwarded client input
    // is queued by seq so `apply_buffered_client_inputs` steps EXACTLY ONE per
    // fixed tick — matching the client's one-input-per-tick prediction, so the two
    // deterministic sims stay in lockstep (no divergence → gentle reconcile).
    reg: Res<lunco_core::SessionRegistry>,
    local: Res<lunco_core::LocalSession>,
    mut buffered: ResMut<lunco_core::BufferedClientInputs>,
    q: Query<(&lunco_core::GlobalEntityId, Has<lunco_core::OwnedLocally>)>,
) {
    let cmd = trigger.event();
    let Ok((gid, owned)) = q.get(cmd.target) else { return };
    let g = gid.get();
    // Capture throttle/steer for the render-lead (both roles harmless; the lead
    // system is client-only). Undeclared names default to the prior value.
    {
        let entry = drive_input.0.entry(g).or_insert((0.0, 0.0));
        for (name, v) in &cmd.writes {
            match name.as_str() {
                "throttle" | "forward" => entry.0 = *v,
                "steer" => entry.1 = *v,
                _ => {}
            }
        }
    }
    if role.is_host() {
        let owner = reg.owner_of(g);
        // Queue a REMOTE-owned rover's forwarded input for per-tick application, so
        // the host integrates the same input sequence one-per-tick as the client
        // predicted (its own drives — owner == host — apply immediately, unbuffered).
        if cmd.seq != 0 && owner.is_some_and(|o| o != local.0) {
            // ACK DISCIPLINE (review N2): do NOT ack here. This observer runs on the
            // RENDER clock (`drain_sync_inbox` is in `Update`), so a host whose
            // `Update` is slower than its `FixedUpdate` drains K of the client's
            // per-tick `SetPorts` in one frame. Acking `max(seq)` here claimed all K
            // were applied while physics had integrated only the one that
            // `apply_buffered_client_inputs` consumes this fixed tick — the client
            // then dropped K−1 predicted frames it had actually simulated, and the
            // divergence scaled with input VARIABILITY (i.e. showed up on turns and
            // stops: the "post-turn oscillation"). The ack is now stamped by the
            // consumer, from the seq it really integrated.
            buffered.push(g, cmd.seq, cmd.writes.clone());
        } else {
            // Host-local / API drive: applied straight to the ports, so the ack is
            // immediate. `record` binds the slot to its owner and rejects an
            // implausible seq jump (review N1).
            applied.record(g, owner, cmd.seq);
        }
        return;
    }
    // --- Client ---
    if owned && cmd.seq != 0 {
        // Buffer the frame keyed by seq so `record_predicted_state` keys its pose
        // and reconcile can prune. The forward/steer/brake payload is unused by
        // the current positional reconcile (awaits true input-replay).
        let entry = owned_log.0.entry(g).or_default();
        if entry.frames.back().map_or(true, |f| f.seq != cmd.seq) {
            // Capture the REAL actuation for deterministic input-replay rollback.
            // `drive_from_bindings` resolves every bound port each tick, so the
            // owned-client stream carries the full set; latch from the prior frame
            // for any name a given command happens to omit (API/partial writes).
            let prev = entry.frames.back();
            let mut forward = prev.map_or(0.0, |f| f.forward);
            let mut steer = prev.map_or(0.0, |f| f.steer);
            let mut brake = prev.map_or(0.0, |f| f.brake);
            for (name, v) in &cmd.writes {
                match name.as_str() {
                    "throttle" | "forward" => forward = *v,
                    "steer" => steer = *v,
                    "brake" => brake = *v,
                    _ => {}
                }
            }
            entry.frames.push_back(lunco_core::InputFrame {
                seq: cmd.seq,
                tick: cmd.tick,
                forward,
                steer,
                brake,
            });
            while entry.frames.len() > MAX_INPUT_FRAMES {
                entry.frames.pop_front();
            }
        }
    }
    // Prediction-membership signal (Phase A): record activity on ANY nonzero
    // write, independent of `owned`/`seq`, so the first real input can bootstrap
    // prediction even while the body is still an interpolated proxy. Stamp the
    // CURRENT sim tick, NOT `cmd.tick`: the tick field is the caller's ordering
    // hint and is 0 for host-local scenario/API drives (the `drive()` prelude,
    // HTTP `SetPorts`), which would pin `last_active_tick` at 0 forever and never
    // promote the body to predicted. `drive_from_bindings` already sends the real
    // tick, so keyboard behaviour is unchanged.
    if cmd.writes.iter().any(|(_, v)| v.abs() > INPUT_EPS) {
        owned_log.0.entry(g).or_default().last_active_tick = sim_tick.0;
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
mod input_ack_tests {
    use super::*;
    use lunco_core::{
        AppliedInputSeq, BufferedClientInputs, GlobalEntityId, LocalDriveInput, LocalSession,
        NetworkRole, OwnedInputLog, SessionId, SessionRegistry, SimTick,
    };

    const HOST: SessionId = SessionId(0);
    const CLIENT_A: SessionId = SessionId(11);
    const CLIENT_B: SessionId = SessionId(22);

    /// A host app carrying just the substrate `record_control_input` touches, plus
    /// the observer itself — no physics, no wire.
    fn host_app(owner: SessionId, gid: u64) -> (App, Entity) {
        let mut app = App::new();
        app.insert_resource(NetworkRole::Host)
            .insert_resource(LocalSession(HOST))
            .init_resource::<SimTick>()
            .init_resource::<OwnedInputLog>()
            .init_resource::<AppliedInputSeq>()
            .init_resource::<LocalDriveInput>()
            .init_resource::<BufferedClientInputs>()
            .init_resource::<SessionRegistry>();
        app.world_mut()
            .resource_mut::<SessionRegistry>()
            .claim(owner, gid)
            .expect("claim");
        app.add_observer(record_control_input);
        let e = app.world_mut().spawn(GlobalEntityId::from_raw(gid)).id();
        (app, e)
    }

    fn drive(app: &mut App, target: Entity, seq: u32, steer: f64) {
        app.world_mut().trigger(lunco_cosim::SetPorts {
            target,
            writes: vec![("steer".to_string(), steer)],
            seq,
            tick: seq as u64,
        });
        app.update();
    }

    /// The host's fixed-tick consumer, in miniature: exactly what
    /// `apply_buffered_client_inputs` (lunco-sandbox-edit) does to the ack.
    fn integrate_one_fixed_tick(app: &mut App, gid: u64) {
        let owner = app.world().resource::<SessionRegistry>().owner_of(gid);
        let mut buf = app.world_mut().resource_mut::<BufferedClientInputs>();
        let consumed = buf.next_for_tick(gid, 8).is_some();
        let cursor = buf.cursor(gid);
        if consumed {
            app.world_mut()
                .resource_mut::<AppliedInputSeq>()
                .record(gid, owner, cursor);
        }
    }

    /// **N2 — the host must not ack input it has not integrated.** The wire is drained
    /// on the RENDER clock, so one frame can deliver K of the client's per-fixed-tick
    /// `SetPorts`; physics runs ONE per fixed tick. The old code stamped `max(seq)`
    /// into the snapshot the moment the command arrived — claiming all K applied.
    /// The client then dropped K−1 predicted frames it had genuinely simulated, and
    /// the resulting divergence scaled with how much the input CHANGED across them:
    /// i.e. it appeared on turns and stops. That is the reported "post-turn
    /// oscillation", and the widened reconcile dead-zone was a band-aid over it.
    #[test]
    fn host_acks_only_the_input_it_actually_integrated() {
        let gid = 0xBEEF_0001;
        let (mut app, e) = host_app(CLIENT_A, gid);

        // One slow render frame delivers three ticks of a TURN (steer sweeping).
        drive(&mut app, e, 1, 0.0);
        drive(&mut app, e, 2, 0.5);
        drive(&mut app, e, 3, 1.0);

        // Nothing has been integrated yet — physics has not run a fixed tick.
        assert_eq!(
            app.world().resource::<AppliedInputSeq>().ack(gid),
            0,
            "receiving an input is not applying it (this was `max(seq)` = 3)"
        );
        assert_eq!(
            app.world().resource::<BufferedClientInputs>().pending[&gid].len(),
            3,
            "all three inputs are queued for per-tick consumption"
        );

        // Each fixed tick integrates exactly one, and the ack follows it.
        for expected in 1..=3u32 {
            integrate_one_fixed_tick(&mut app, gid);
            assert_eq!(
                app.world().resource::<AppliedInputSeq>().ack(gid),
                expected,
                "the ack must name the seq physics ran on tick {expected}"
            );
        }
    }

    /// **N1 — the bug users hit in ordinary play.** Client A drives the rover to a
    /// high `seq` and releases; client B possesses it and starts from `seq = 1`. The
    /// gid-only watermark kept stamping A's 5000 into every snapshot, which B's
    /// reconcile latched as `last_reconciled` — after which every ack from B's own
    /// stream was `<=` it and reconciliation early-returned FOREVER. B's rover then
    /// drifts, unreconciled, with no attacker and no packet loss involved.
    #[test]
    fn repossession_resets_the_ack_so_the_new_owner_is_reconciled() {
        let gid = 0xBEEF_0002;
        let (mut app, e) = host_app(CLIENT_A, gid);

        // A drives a long way into its seq stream (and the host integrates it).
        for seq in 1..=50u32 {
            drive(&mut app, e, seq, 1.0);
            integrate_one_fixed_tick(&mut app, gid);
        }
        assert_eq!(app.world().resource::<AppliedInputSeq>().ack(gid), 50);

        // A releases, B possesses — the ownership table changed, so the host re-keys
        // its watermarks (`sync_applied_seq_owners`, LunCoCorePlugin/FixedFirst).
        {
            let mut reg = app.world_mut().resource_mut::<SessionRegistry>();
            reg.release_session(CLIENT_A);
            reg.claim(CLIENT_B, gid).expect("B claims the rover");
        }
        app.add_systems(FixedFirst, lunco_core::sync_applied_seq_owners);
        app.world_mut().run_schedule(FixedFirst);

        assert_eq!(
            app.world().resource::<AppliedInputSeq>().ack(gid),
            0,
            "the snapshot must stop advertising the PREVIOUS owner's seq the moment \
             the vessel changes hands — otherwise B latches it and never reconciles again"
        );

        // B's stream starts at 1 and is acked from there — reconciliation lives.
        drive(&mut app, e, 1, 0.3);
        integrate_one_fixed_tick(&mut app, gid);
        assert_eq!(app.world().resource::<AppliedInputSeq>().ack(gid), 1);
        drive(&mut app, e, 2, 0.6);
        integrate_one_fixed_tick(&mut app, gid);
        assert_eq!(app.world().resource::<AppliedInputSeq>().ack(gid), 2);
    }

    /// A hostile/corrupt `SetPorts { seq: u32::MAX }` must not poison the gid — for
    /// this owner or any future one. Under the old rule nothing could ever exceed the
    /// watermark again, so no ack was ever "new" and the owner's reconcile
    /// early-returned for the life of the process.
    #[test]
    fn a_wild_seq_cannot_poison_the_vessel() {
        let gid = 0xBEEF_0003;
        let (mut app, e) = host_app(CLIENT_A, gid);
        drive(&mut app, e, 1, 0.0);
        integrate_one_fixed_tick(&mut app, gid);

        drive(&mut app, e, u32::MAX, 1.0);
        integrate_one_fixed_tick(&mut app, gid);
        assert_eq!(
            app.world().resource::<AppliedInputSeq>().ack(gid),
            1,
            "u32::MAX must never become the watermark"
        );

        // …and the vessel still works: the next genuine input is consumed and acked.
        drive(&mut app, e, 2, 0.2);
        integrate_one_fixed_tick(&mut app, gid);
        assert_eq!(app.world().resource::<AppliedInputSeq>().ack(gid), 2);
    }
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
