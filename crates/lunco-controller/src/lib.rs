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
//!    ([`InputBindingsSettings::input_map`]) turns keys/gamepad into semantic intents
//!    (`MoveForward`, `Action`, …). This is the ONLY place raw devices appear,
//!    it's shared with avatar locomotion, and — being a leafwing InputMap — it's
//!    serializable, so a saved keymap ("mapping file") rebinds every vessel.
//! 2. **intent → port** ([`ControlBinding`], per-vessel, authorable in USD/rhai):
//!    an active intent contributes `scale` to a named input port. A rover maps
//!    `MoveForward→throttle`; a cosim-flown lander maps `MoveForward→manual_pitch`.
//!    Same intent vocabulary, different actuation — no vessel-kind branch.
//!
//! Two systems compose the stages, split by WHAT they drive — the cadence follows
//! from that, it is not a policy knob:
//!
//! * [`drive_from_bindings`] — **vessels**, in `FixedUpdate`. One `SetPorts` per
//!   fixed tick per controller, seq-stamped for prediction/rollback. Pauses with the
//!   sim, because a paused rover must not move.
//! * [`drive_self_drivers`] — the **free avatar**, in
//!   [`lunco_time::InteractionSchedule`]. Kinematic, client-local, never predicted, so
//!   it belongs on the unpausable presentation step: pausing the simulation must not
//!   paralyse the user. Possessing a vessel adds a [`ControllerLink`], which moves that
//!   entity to the first system by query, not by a flag.
//!
//! Both share stage 1 ([`intent_held`]) and stage 2 ([`ControlBinding::resolve`]).
//! Because control is keyed by *intent*, anything internal (rhai, mission logic, AI)
//! can drive a vessel by naming intents — the same consistent vocabulary. All writes
//! land through the same [`lunco_core::ports::PortRegistry`].

use bevy::input::mouse::MouseButton;
use bevy::prelude::*;
use leafwing_input_manager::prelude::ActionState;
use lunco_core::{on_command, register_commands, Command, UserIntent};
use lunco_settings::{AppSettingsExt, SettingsSection};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Intents forced held by [`SimulateIntent`], **keyed by the entity they drive** —
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
    /// The **entity this intent drives** (normally a vessel or avatar command
    /// surface). An intent is meaningless without its target: two spawns of one
    /// asset are two distinct entities, and a targetless intent is rejected. Over
    /// the API this takes the target's `api_id` — the `GlobalEntityId` reported by
    /// `ListEntities` — and is resolved to the live entity.
    pub target: Entity,
}

impl Default for SimulateIntent {
    fn default() -> Self {
        Self {
            intent: String::new(),
            held: false,
            target: Entity::PLACEHOLDER,
        }
    }
}

#[on_command(SimulateIntent)]
fn on_simulate_intent(trigger: On<SimulateIntent>, mut sim: ResMut<SimulatedIntents>) {
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

/// Declare that commands can (or cannot) currently reach `target`.
///
/// The generic verb behind [`lunco_core::session::ControlPathRegistry`]. A mission
/// script computes the DOMAIN fact and states the CONSEQUENCE here; an authored
/// policy ([`lunco_core::session::AUTHORIZE_HOOK`]) then decides what to refuse.
/// Space School does exactly that — `ss3_radio_shadow.rhai` reads real link geometry
/// with `can_reach(radio, "earth")` and calls this — which keeps doc 49's split one
/// layer up: the kernel computes geometry, the script decides what it means, and
/// nothing in Rust ever concludes "no link ⇒ no control" (a store-and-forward
/// mission would disagree).
///
/// It lives here rather than in `lunco-core` for a mechanical reason: `#[Command]`
/// expands to `lunco_core::…` paths, so a command cannot be declared inside that
/// crate. Beside `drive_from_bindings` is the right second choice — this is the path
/// it gates.
#[Command]
pub struct SetControlPath {
    /// The vessel commands cannot reach.
    #[authz_target]
    pub target: Entity,
    /// `true` ⇒ commands do not reach `target`.
    pub down: bool,
}

// `Entity` has no `Default`, so this is hand-written rather than `#[Command(default)]`
// — the same shape `SimulateIntent` above uses. `PLACEHOLDER` never resolves to a
// real vessel, so a `SetControlPath` that arrives without a target is inert.
impl Default for SetControlPath {
    fn default() -> Self {
        Self {
            target: Entity::PLACEHOLDER,
            down: false,
        }
    }
}

#[on_command(SetControlPath)]
fn on_set_control_path(
    trigger: On<SetControlPath>,
    q_gid: Query<&lunco_core::GlobalEntityId>,
    mut paths: ResMut<lunco_core::session::ControlPathRegistry>,
) {
    let cmd = trigger.event();
    // No gid ⇒ no stable identity to key the blackout on, and a fabricated key
    // would mis-bind across peers and reloads. Skip rather than guess — the same
    // rule the link kernel applies to a node whose identity has not minted yet.
    let Ok(gid) = q_gid.get(cmd.target) else {
        warn!("[control-path] target has no GlobalEntityId — ignoring");
        return;
    };
    paths.set(gid.get(), cmd.down);
    info!(
        "[control-path] gid {} {}",
        gid.get(),
        if cmd.down {
            "DOWN — commands will not reach it"
        } else {
            "restored"
        }
    );
}

register_commands!(on_simulate_intent, on_set_control_path);

/// Plugin for managing vessel input and command translation.
pub struct LunCoControllerPlugin;

/// Clear controller state that names scene entities. A released intent or
/// control-path blackout must not be applied to a replacement scene that reuses
/// the same entity slot or global id.
fn reset_scene_control_state(
    mut intents: ResMut<SimulatedIntents>,
    mut paths: ResMut<lunco_core::session::ControlPathRegistry>,
) {
    intents.0.clear();
    paths.clear();
}

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
        app.init_resource::<SimulatedIntents>()
            .register_type::<InputBindingsSettings>()
            .register_settings_section::<InputBindingsSettings>();
        app.add_systems(lunco_core::SceneTeardown, reset_scene_control_state);
        // The blackout table the authorization gate reads. Empty by default, so an
        // app that never declares one is byte-for-byte unchanged.
        app.init_resource::<lunco_core::session::ControlPathRegistry>();
        register_all_commands(app);
        app.add_systems(
            FixedUpdate,
            // Ahead of wire propagation, so the `Port` writes this tick emits
            // reach their wired targets in the same tick. Unordered, propagation
            // may read the port before or after this system depending on the
            // schedule's parallel layout, and prediction diverges from the host
            // on that coin flip.
            drive_from_bindings
                .run_if(lunco_core::not_rolling_back)
                .run_if(lunco_time::simulation_is_running)
                .before(lunco_core::ControlDacSet),
        );
        // The SELF-DRIVER half runs on the INTERACTION cadence, not the sim tick.
        //
        // A self-driver is the free avatar: kinematic, client-local, not part of the
        // simulation and not predicted. Riding `FixedUpdate` gave it the sim's pause
        // for free — `Time<Virtual>` pauses ⇒ `FixedUpdate` stops ⇒ the avatar's
        // `InputPorts` froze at their last value, so a paused world could not be
        // flown or walked around even though every camera system already ran on the
        // wall clock. That is cadence standing in for clock again (see
        // `lunco_time::interaction`): the fix is the cadence that is unpausable *by
        // construction*, not a `run_if(paused)` twin or a raw `Time<Real>` bypass.
        //
        // Pause still means what it says for the SIMULATION: a possessed vessel gets
        // a `ControllerLink`, which excludes it from `q_self`, so its input keeps
        // riding `FixedUpdate` and a paused rover stays put.
        if !app.is_plugin_added::<lunco_time::TimePlugin>() {
            app.add_plugins(lunco_time::TimePlugin);
        }
        app.configure_sets(lunco_time::InteractionSchedule, InteractionControlSet);
        app.add_systems(
            lunco_time::InteractionSchedule,
            drive_self_drivers.in_set(InteractionControlSet),
        );
        app.add_systems(Update, refresh_live_input_maps);
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

/// Interaction-schedule boundary for the avatar's command producer.
///
/// The free avatar consumes its command ports in `lunco-avatar` on the same
/// unpausable cadence. Keeping the producer in a named set lets that consumer
/// establish a real dependency, which also gives Bevy an `ApplyDeferred` sync
/// point for the `SetPorts` observer before movement reads the ports.
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone)]
pub struct InteractionControlSet;

/// Cap on the unacked input ring (~2 s at 60 Hz). The reconcile normally drains
/// it to the acked `seq` each snapshot; this only bounds a stalled/disconnected
/// client so the buffer can't grow without limit.
const MAX_INPUT_FRAMES: usize = 128;

/// Magnitude below which a control setpoint counts as "no input" for the
/// prediction-membership signal (`VesselInputLog::last_active_tick`). The
/// controller emits a `SetPorts` every fixed tick even when idle (all zeros), so
/// presence of writes is NOT an activity signal — the *value* is.
const INPUT_EPS: f64 = 1e-3;

/// Fixed-tick input emission for prediction. Emits a [`lunco_cosim::SetPorts`]
/// while a controller is active and once on the active→idle edge, from its
/// [`ControlBinding`] and held keys, stamped with a per-vessel `seq` + `SimTick`.
/// For a vessel this client owns + predicts ([`lunco_core::OwnedLocally`]) the
/// frame is buffered for replay by [`record_control_input`]; on host/standalone
/// the command uses `seq = 0`. Silent idle ticks leave the domain writer alone.
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
    // The authored authorization POLICY applies to the local keyboard too — see the
    // gate below. All `Option` so a controller-only test app without the session
    // substrate still runs ungated.
    rbac: Option<Res<lunco_core::session::SessionRbac>>,
    control_paths: Option<Res<lunco_core::session::ControlPathRegistry>>,
    q_ctrl: Query<(&ControllerLink, &ActionState<UserIntent>)>,
    q_binding: Query<&ControlBinding>,
    q_vessel: Query<(&lunco_core::GlobalEntityId, Has<lunco_core::OwnedLocally>)>,
    // egui keyboard capture (published by `lunco-workbench`). While a text field
    // is focused we treat every intent as released so a keypress typed into the UI
    // doesn't also drive the vessel — see the `held` closure below. `Option` so a
    // controller-only test app without the workbench still runs (no gate).
    egui_focus: Option<Res<lunco_core::EguiFocus>>,
    // Intents forced by `SimulateIntent` — the headless/API/rhai stand-in for keys.
    sim_intents: Option<Res<SimulatedIntents>>,
    // Per-vessel "keys were active last tick" memory for the idle-yield below.
    mut was_active: Local<std::collections::HashMap<Entity, bool>>,
    // Despawned vessels leave `was_active` — pruned below so a recycled
    // Entity id can't inherit a stale flag and mistime the all-zero batch.
    mut removed_bindings: RemovedComponents<ControlBinding>,
    mut commands: Commands,
) {
    // Prune despawned/unbound vessels before reading edges: a recycled Entity
    // id must start from "idle", not the previous vessel's last state.
    for vessel in removed_bindings.read() {
        was_active.remove(&vessel);
    }

    let client = matches!(*role, lunco_core::NetworkRole::Client);

    // When egui holds the keyboard, no local key counts as pressed. `drive_from_
    // bindings` still runs and `resolve` still writes EVERY bound port — now all
    // 0 — so the vessel decelerates to a clean stop rather than latching its last
    // command (as it would if we simply skipped the system).
    let egui_keyboard = egui_focus.is_some_and(|f| f.wants_keyboard);
    let sim_intents = sim_intents.as_deref();
    let held = |vessel: Entity, intent, intents: &ActionState<UserIntent>| {
        intent_held(vessel, intent, intents, sim_intents, egui_keyboard)
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

        // The authored authorization policy ([`AUTHORIZE_HOOK`]) gates the LOCAL
        // keyboard, not just the wire and script paths. Without this a policy like
        // "refuse tele-op while the control path is down" was true only for remote
        // and scripted commands, while the student at the keyboard drove straight
        // through it — `authorize()` sits on `sync.rs` and `bridge_core.rs`, and
        // this system triggers `SetPorts` directly.
        //
        // `authorize_policy`, NOT the full `authorize`: the role/ownership floor is a
        // wire concern. This loop deliberately drives an UNPOSSESSED vessel (owner
        // `None`, per the yield above), which the ownership-gated floor would refuse
        // — gating the floor here would break ordinary local play. The policy is what
        // must bind everywhere; the floor stays where it belongs.
        if let (Some(rbac), Some(paths), Some(local), Some((gid, _))) = (
            rbac.as_ref(),
            control_paths.as_ref(),
            local_session.as_ref(),
            vessel_id,
        ) {
            let owns = registry
                .as_ref()
                .is_some_and(|reg| reg.owns(local.0, gid.get()));
            if lunco_core::session::authorize_policy(
                rbac,
                paths,
                local.0,
                "SetPorts",
                Some(gid.get()),
                owns,
            )
            .is_err()
            {
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
        let active = writes.iter().any(|(_, v)| v.abs() > f64::EPSILON);
        let prev = was_active
            .insert(link.vessel_entity, active)
            .unwrap_or(false);
        // An idle client does not own the control surface merely because it is
        // predicted. The active→idle edge above emits one real zero batch so
        // the actuator stops; subsequent idle ticks are silent and cannot
        // overwrite a scripted/API controller. Prediction bookkeeping resumes
        // on the next active edge with a fresh contiguous input sequence.
        if !active && !prev {
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
}

/// Stage 1 of the mapping, shared by both cadences: is `intent` held *for this
/// vessel* right now?
///
/// A simulated intent counts as held regardless of the egui gate (it is not a
/// physical key that a focused text field could be swallowing).
///
/// Scoped to `vessel`: a simulated intent drives ONLY the vessel it was addressed
/// to. The keyboard half is per-vessel too, via that vessel's own `ActionState`.
/// (The sim half used to be a global set consulted inside the drive loop, so one
/// `SimulateIntent` pressed the key on EVERY controlled vessel.)
fn intent_held(
    vessel: Entity,
    intent: UserIntent,
    intents: &ActionState<UserIntent>,
    sim_intents: Option<&SimulatedIntents>,
    egui_keyboard: bool,
) -> bool {
    sim_intents.is_some_and(|s| s.0.get(&vessel).is_some_and(|set| set.contains(&intent)))
        || (!egui_keyboard && intents.pressed(&intent))
}

/// Self-drive (the free avatar): drive the entity's OWN command surface from its
/// OWN input via its OWN binding — the identical `SetPorts` path, no bespoke avatar
/// movement code. Local & kinematic (`apply_fly` integrates the ports), so no
/// seq/tick prediction bookkeeping. `resolve` writes every bound port (0 when idle),
/// so a released key zeroes the port and motion stops.
///
/// Runs in [`lunco_time::InteractionSchedule`] — the constant-rate, never-paused
/// presentation step — so pausing the SIMULATION does not paralyse the user. Nothing
/// here reads a `dt`: the binding maps held intents to setpoints, and the consumer
/// (`apply_fly`) integrates them on the interaction clock.
///
/// Disjoint from [`drive_from_bindings`]'s query by `Without<ControllerLink>`: an
/// avatar that possesses a vessel is no longer a self-driver, so its input goes back
/// on the sim tick and freezes with the sim, as pause is meant to.
fn drive_self_drivers(
    q_self: Query<(Entity, &ActionState<UserIntent>, &ControlBinding), Without<ControllerLink>>,
    egui_focus: Option<Res<lunco_core::EguiFocus>>,
    sim_intents: Option<Res<SimulatedIntents>>,
    mut commands: Commands,
) {
    let egui_keyboard = egui_focus.is_some_and(|f| f.wants_keyboard);
    let sim_intents = sim_intents.as_deref();
    for (entity, intents, binding) in q_self.iter() {
        // A self-driver IS its own vessel, so it is its own intent subject.
        let writes = binding
            .resolve(|intent| intent_held(entity, intent, intents, sim_intents, egui_keyboard));
        commands.trigger(lunco_cosim::SetPorts {
            target: entity,
            writes,
            seq: 0,
            tick: 0,
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
    sim_tick: Res<lunco_core::SimTick>,
    virtual_time: Option<Res<Time<Virtual>>>,
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
    if virtual_time.is_some_and(|time| time.is_paused()) {
        return;
    }
    let Ok((gid, owned)) = q.get(cmd.target) else {
        return;
    };
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
        if entry.frames.back().is_none_or(|f| f.seq != cmd.seq) {
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

fn default_look_button() -> String {
    "Right".into()
}

/// The resolved semantic input map shared by avatar control, UI help, and
/// tutorials.
///
/// The bundled keymap is the default value. A user may override this typed
/// settings section in settings.json; omitted semantic bindings inherit the
/// current bundled values while an explicit empty array remains unbound. No
/// consumer gets a separate copy of the bindings. The bundled JSON contains
/// only settings data; explanatory text belongs in the asset and crate
/// documentation rather than in the map.
#[derive(Resource, Reflect, Serialize, Clone, PartialEq, Debug)]
#[reflect(Resource)]
pub struct InputBindingsSettings {
    /// Semantic intent name → key names understood by Bevy.
    #[serde(flatten)]
    pub bindings: BTreeMap<String, Vec<KeyCode>>,
    /// Pointer button activating the semantic look intent.
    #[serde(default = "default_look_button")]
    pub look_button: String,
}

#[derive(Deserialize)]
struct InputBindingsFile {
    #[serde(flatten)]
    bindings: BTreeMap<String, Vec<KeyCode>>,
    #[serde(default = "default_look_button")]
    look_button: String,
}

fn bundled_input_bindings() -> InputBindingsFile {
    serde_json::from_str(KEYBINDINGS_JSON)
        .expect("assets/config/keybindings.json must be valid input settings")
}

impl Default for InputBindingsSettings {
    fn default() -> Self {
        let bundled = bundled_input_bindings();
        Self {
            bindings: bundled.bindings,
            look_button: bundled.look_button,
        }
    }
}

impl<'de> Deserialize<'de> for InputBindingsSettings {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct StoredInputBindings {
            #[serde(flatten)]
            bindings: BTreeMap<String, Vec<KeyCode>>,
            #[serde(default = "default_look_button")]
            look_button: String,
        }

        let stored = StoredInputBindings::deserialize(deserializer)?;
        let mut bindings = bundled_input_bindings().bindings;
        // A settings file is an override layer, not a second copy of the
        // bundled schema. Missing semantic inputs inherit the current authored
        // defaults; an explicit empty array still means "unbound".
        bindings.extend(stored.bindings);
        Ok(Self {
            bindings,
            look_button: stored.look_button,
        })
    }
}

impl SettingsSection for InputBindingsSettings {
    const KEY: &'static str = "input_bindings";

    fn validate_section(&self) -> Result<(), String> {
        if self.look_button_value().is_none() {
            return Err(format!("invalid look_button '{}'", self.look_button));
        }
        for intent in self.bindings.keys() {
            if lunco_core::parse_user_intent(intent).is_none() {
                return Err(format!("unknown input intent '{intent}'"));
            }
        }
        Ok(())
    }
}

impl InputBindingsSettings {
    /// Build the live leafwing map from the resolved settings section.
    pub fn input_map(
        &self,
    ) -> Result<leafwing_input_manager::prelude::InputMap<UserIntent>, String> {
        let bindings = self.key_bindings()?;
        let Some(button) = self.look_button_value() else {
            return Err(format!("invalid look_button '{}'", self.look_button));
        };
        Ok(build_input_map(bindings, button))
    }

    /// Return the resolved key bindings for help, tutorials, and input injection.
    pub fn key_bindings(&self) -> Result<Vec<(UserIntent, Vec<KeyCode>)>, String> {
        self.bindings
            .iter()
            .map(|(name, keys)| {
                lunco_core::parse_user_intent(name)
                    .map(|intent| (intent, keys.clone()))
                    .ok_or_else(|| format!("unknown input intent '{name}'"))
            })
            .collect()
    }

    /// Resolve a compact or Bevy spelling against the current settings.
    pub fn key_code(&self, label: &str) -> Result<Option<KeyCode>, String> {
        let needle = label.trim();
        Ok(self
            .key_bindings()?
            .into_iter()
            .flat_map(|(_, keys)| keys.into_iter())
            .find(|key| {
                let debug = format!("{key:?}");
                debug.eq_ignore_ascii_case(needle)
                    || key_label(std::slice::from_ref(key)).eq_ignore_ascii_case(needle)
            }))
    }

    /// Human-readable key or pointer labels for tutorial/help copy.
    pub fn label(&self, binding: &str) -> Option<String> {
        if binding == "look_button" {
            self.look_button_value()?;
            return Some(format!("{} mouse button", self.look_button.to_lowercase()));
        }
        let keys = self.bindings.get(binding)?;
        (!keys.is_empty()).then(|| key_label(keys))
    }

    /// Resolve the display label for a semantic intent used by a vessel.
    pub fn label_for_intent(&self, intent: UserIntent) -> Result<String, String> {
        let name = self
            .key_bindings()?
            .into_iter()
            .find_map(|(candidate, keys)| (candidate == intent).then_some(keys))
            .filter(|keys| !keys.is_empty())
            .map_or_else(|| "unbound".to_owned(), |keys| key_label(&keys));
        Ok(name)
    }

    fn look_button_value(&self) -> Option<MouseButton> {
        parse_look_button(&self.look_button)
    }
}

/// Resolve the user-facing label for a semantic intent from the one shared
/// input-bindings resource. Invalid settings are never projected into a live
/// input map, but keeping the semantic label here makes help surfaces safe for
/// a partially loaded settings resource as well.
pub fn resolved_input_label(settings: &InputBindingsSettings, intent: UserIntent) -> String {
    settings
        .label_for_intent(intent)
        .unwrap_or_else(|_| intent.to_string())
}

/// Read the pointer button that activates the semantic `Look` intent.
///
/// Pointer bindings live beside the keyboard bindings because they are part of
/// the same user input map. The documented default is the secondary button; an
/// authored keymap can select another button without changing camera code.
///
/// An omitted field is the documented semantic default. An invalid explicit
/// value is rejected instead of silently changing the user's control scheme.
pub fn parse_look_button(name: &str) -> Option<MouseButton> {
    match name.trim().to_ascii_lowercase().as_str() {
        "left" => Some(MouseButton::Left),
        "middle" => Some(MouseButton::Middle),
        "back" => Some(MouseButton::Back),
        "forward" => Some(MouseButton::Forward),
        "right" => Some(MouseButton::Right),
        _ => None,
    }
}

/// Compact user-facing spelling for a key list from a data-driven convention.
/// Help and accessibility consumers use this instead of reformatting Bevy's
/// `KeyCode` names independently.
pub fn key_label(keys: &[KeyCode]) -> String {
    keys.iter()
        .map(|key| {
            let name = format!("{key:?}");
            name.strip_prefix("Key").unwrap_or(&name).to_string()
        })
        .collect::<Vec<_>>()
        .join(" / ")
}

/// Build an avatar `InputMap<UserIntent>` from a key/pointer→intent JSON object
/// (`{"forward":["KeyW"], "action":["KeyF"], "thrust":["Space"], …}`).
/// Keys are Bevy `KeyCode` variant names, intents are canonical USD control
/// names, and `look_button` selects the button that chords the `Look` axis.
pub fn build_avatar_input_map(
    json: &str,
) -> Result<leafwing_input_manager::prelude::InputMap<lunco_core::UserIntent>, String> {
    let settings: InputBindingsSettings =
        serde_json::from_str(json).map_err(|error| format!("invalid input bindings: {error}"))?;
    settings.input_map()
}

fn build_input_map(
    bindings: Vec<(UserIntent, Vec<KeyCode>)>,
    button: MouseButton,
) -> leafwing_input_manager::prelude::InputMap<lunco_core::UserIntent> {
    use leafwing_input_manager::prelude::*;
    use lunco_core::UserIntent::{Look, Zoom};

    let mut input_map = InputMap::default();
    for (intent, keys) in bindings {
        for key in keys {
            input_map.insert(intent, key);
        }
    }
    // This chord is the complete look binding. Camera behaviour consumes only
    // the resulting semantic axis, so no downstream system needs a raw button
    // gate.
    input_map.insert_dual_axis(Look, DualAxislikeChord::new(button, MouseMove::default()));
    input_map.insert_axis(Zoom, MouseScrollAxis::Y);
    input_map
}

/// Apply a changed settings section to every live local input surface.
///
/// Input maps are components because leafwing reads them from the same entity
/// as its ActionState. The settings resource remains the owner; this observer
/// only projects its current value and never maintains a second binding table.
fn refresh_live_input_maps(
    settings: Res<InputBindingsSettings>,
    mut maps: Query<&mut leafwing_input_manager::prelude::InputMap<UserIntent>>,
) {
    if !settings.is_changed() {
        return;
    }
    let Ok(map) = settings.input_map() else {
        error!("[input] refusing to project invalid input bindings settings");
        return;
    };
    for mut live in &mut maps {
        *live = map.clone();
    }
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
    /// `apply_buffered_client_inputs` (lunco-luncosim-edit) does to the ack.
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

    #[derive(Resource, Default)]
    struct InteractionObserved(Option<(f64, f64, f64)>);

    fn observe_interaction_ports(
        q: Query<&lunco_core::InputPorts>,
        mut observed: ResMut<InteractionObserved>,
    ) {
        let inputs = q.single().expect("the free avatar input surface");
        observed.0 = Some((
            inputs.cmd("forward"),
            inputs.cmd("up"),
            inputs.cmd("speed_boost"),
        ));
    }

    /// The configured pointer button is part of the same semantic `Look`
    /// binding as the mouse axis. This prevents camera code from silently
    /// reintroducing a raw secondary-button assumption.
    #[test]
    fn look_axis_uses_the_configured_pointer_button() {
        use bevy::input::mouse::MouseButton;
        use bevy::input::InputPlugin;
        use leafwing_input_manager::prelude::{
            Buttonlike, DualAxislike, InputManagerPlugin, MouseMove,
        };

        let mut app = App::new();
        app.add_plugins((
            bevy::time::TimePlugin,
            InputPlugin,
            InputManagerPlugin::<UserIntent>::default(),
        ));
        let entity = app
            .world_mut()
            .spawn((
                ActionState::<UserIntent>::default(),
                build_avatar_input_map(r#"{"look_button":"Middle"}"#)
                    .expect("valid pointer binding"),
            ))
            .id();

        MouseButton::Right.press(app.world_mut());
        MouseMove::default().set_axis_pair(app.world_mut(), Vec2::new(4.0, -2.0));
        app.update();
        assert_eq!(
            app.world()
                .entity(entity)
                .get::<ActionState<UserIntent>>()
                .expect("action state")
                .axis_pair(&UserIntent::Look),
            Vec2::ZERO,
            "an unconfigured button must not activate Look"
        );

        MouseButton::Middle.press(app.world_mut());
        MouseMove::default().set_axis_pair(app.world_mut(), Vec2::new(4.0, -2.0));
        app.update();
        assert_eq!(
            app.world()
                .entity(entity)
                .get::<ActionState<UserIntent>>()
                .expect("action state")
                .axis_pair(&UserIntent::Look),
            Vec2::new(4.0, -2.0),
            "the configured button must activate Look"
        );
    }

    #[test]
    fn bundled_right_button_look_binding_reaches_the_semantic_axis() {
        use bevy::input::mouse::MouseButton;
        use bevy::input::InputPlugin;
        use leafwing_input_manager::prelude::{
            Buttonlike, DualAxislike, InputManagerPlugin, MouseMove,
        };

        let mut app = App::new();
        app.add_plugins((
            bevy::time::TimePlugin,
            InputPlugin,
            InputManagerPlugin::<UserIntent>::default(),
        ));
        let entity = app
            .world_mut()
            .spawn((
                ActionState::<UserIntent>::default(),
                InputBindingsSettings::default().input_map().unwrap(),
            ))
            .id();

        MouseButton::Right.press(app.world_mut());
        MouseMove::default().set_axis_pair(app.world_mut(), Vec2::new(3.0, -2.0));
        app.update();

        assert_eq!(
            app.world()
                .entity(entity)
                .get::<ActionState<UserIntent>>()
                .expect("action state")
                .axis_pair(&UserIntent::Look),
            Vec2::new(3.0, -2.0),
            "the shipped right-button binding must produce the Look intent"
        );
    }

    #[test]
    fn invalid_pointer_binding_does_not_rebind_look() {
        assert!(parse_look_button("sideways").is_none());
        assert!(build_avatar_input_map(r#"{"look_button":"sideways"}"#).is_err());
    }

    #[test]
    fn simulated_key_labels_are_resolved_from_the_keymap() {
        let settings = InputBindingsSettings::default();
        assert_eq!(settings.key_code("W").unwrap(), Some(KeyCode::KeyW));
        assert_eq!(settings.key_code("KeyG").unwrap(), Some(KeyCode::KeyG));
        assert_eq!(
            settings.key_code("AltLeft").unwrap(),
            Some(KeyCode::AltLeft)
        );
        assert_eq!(
            settings.key_code("AltRight").unwrap(),
            Some(KeyCode::AltRight)
        );
        assert_eq!(settings.key_code("not-bound").unwrap(), None);
    }

    #[test]
    fn lander_intent_labels_and_port_signs_are_data_driven() {
        let settings = InputBindingsSettings::default();
        assert_eq!(
            resolved_input_label(&settings, UserIntent::MoveForward),
            "W"
        );
        assert_eq!(
            resolved_input_label(&settings, UserIntent::MoveBackward),
            "S"
        );
        assert_eq!(resolved_input_label(&settings, UserIntent::MoveLeft), "A");
        assert_eq!(resolved_input_label(&settings, UserIntent::MoveRight), "D");
        assert_eq!(resolved_input_label(&settings, UserIntent::MoveDown), "Q");
        assert_eq!(resolved_input_label(&settings, UserIntent::MoveUp), "E");

        let rebound: InputBindingsSettings =
            serde_json::from_str(r#"{"forward":["KeyI"],"yaw_left":[]}"#)
                .expect("valid input override");
        assert_eq!(
            resolved_input_label(&rebound, UserIntent::MoveForward),
            "I",
            "help labels must follow the persisted semantic rebind"
        );
        assert_eq!(
            resolved_input_label(&rebound, UserIntent::MoveDown),
            "unbound"
        );

        let binding = ControlBinding::from_intent_entries(&[
            ("forward".into(), "pitch".into(), -1.0),
            ("backward".into(), "pitch".into(), 1.0),
            ("left".into(), "roll".into(), 1.0),
            ("right".into(), "roll".into(), -1.0),
            ("yaw_left".into(), "yaw".into(), 1.0),
            ("yaw_right".into(), "yaw".into(), -1.0),
            ("thrust".into(), "external_throttle".into(), 1.0),
        ])
        .expect("lander controls");

        let ports = |active: &[UserIntent]| binding.resolve(|intent| active.contains(&intent));
        assert_eq!(
            ports(&[UserIntent::MoveForward]),
            vec![
                ("pitch".into(), -1.0),
                ("roll".into(), 0.0),
                ("yaw".into(), 0.0),
                ("external_throttle".into(), 0.0),
            ]
        );
        assert_eq!(
            ports(&[UserIntent::MoveBackward]),
            vec![
                ("pitch".into(), 1.0),
                ("roll".into(), 0.0),
                ("yaw".into(), 0.0),
                ("external_throttle".into(), 0.0),
            ]
        );
        assert_eq!(
            ports(&[UserIntent::MoveForward, UserIntent::MoveBackward]),
            vec![
                ("pitch".into(), 0.0),
                ("roll".into(), 0.0),
                ("yaw".into(), 0.0),
                ("external_throttle".into(), 0.0),
            ],
            "opposite pitch inputs must cancel"
        );
        assert_eq!(
            ports(&[
                UserIntent::MoveLeft,
                UserIntent::MoveRight,
                UserIntent::MoveDown,
                UserIntent::MoveUp,
                UserIntent::Thrust,
            ]),
            vec![
                ("pitch".into(), 0.0),
                ("roll".into(), 0.0),
                ("yaw".into(), 0.0),
                ("external_throttle".into(), 1.0),
            ],
            "opposite attitude inputs cancel while thrust remains active"
        );
        assert!(
            ports(&[]).into_iter().all(|(_, value)| value == 0.0),
            "release must clear every authored lander command port"
        );
    }

    #[test]
    fn both_alt_keys_reach_the_place_waypoint_intent() {
        use bevy::input::InputPlugin;
        use leafwing_input_manager::prelude::InputManagerPlugin;

        let mut app = App::new();
        app.add_plugins((
            bevy::time::TimePlugin,
            InputPlugin,
            InputManagerPlugin::<UserIntent>::default(),
        ));
        let entity = app
            .world_mut()
            .spawn((
                ActionState::<UserIntent>::default(),
                InputBindingsSettings::default().input_map().unwrap(),
            ))
            .id();

        for key in [KeyCode::AltLeft, KeyCode::AltRight] {
            app.world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .press(key);
            app.update();
            assert!(app
                .world()
                .entity(entity)
                .get::<ActionState<UserIntent>>()
                .expect("input manager action state")
                .pressed(&UserIntent::PlaceWaypoint));
            app.world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .release(key);
            app.update();
        }
    }

    #[test]
    fn autopilot_action_is_separate_from_contextual_space_controls() {
        let bindings = InputBindingsSettings::default().key_bindings().unwrap();
        let action = bindings
            .iter()
            .find(|(intent, _)| *intent == UserIntent::Action)
            .map(|(_, keys)| keys.clone());
        let thrust = bindings
            .iter()
            .find(|(intent, _)| *intent == UserIntent::Thrust)
            .map(|(_, keys)| keys.clone());
        let brake = bindings
            .iter()
            .find(|(intent, _)| *intent == UserIntent::Brake)
            .map(|(_, keys)| keys.clone());
        assert_eq!(action, Some(vec![KeyCode::KeyF]));
        assert_eq!(thrust, Some(vec![KeyCode::Space]));
        assert_eq!(brake, Some(vec![KeyCode::Space]));

        let boost = bindings
            .iter()
            .find(|(intent, _)| *intent == UserIntent::SpeedBoost)
            .map(|(_, keys)| keys.clone());
        assert_eq!(
            boost,
            Some(vec![KeyCode::ShiftLeft, KeyCode::ShiftRight]),
            "free-flight boost must come from the shared semantic keymap"
        );

        let input_map = InputBindingsSettings::default().input_map().unwrap();
        assert_eq!(
            input_map
                .get_buttonlike(&UserIntent::SpeedBoost)
                .map(Vec::len),
            Some(2)
        );
        assert_eq!(
            input_map.get_buttonlike(&UserIntent::Thrust).map(Vec::len),
            Some(1),
            "Space must remain bound to the lander thrust intent"
        );
        assert_eq!(
            input_map.get_buttonlike(&UserIntent::Brake).map(Vec::len),
            Some(1),
            "Space must remain bound to the rover brake intent"
        );
    }

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
            if name == "look_button" {
                assert_eq!(val.as_str(), Some("Right"));
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
        assert!(
            bound_keys >= 8,
            "expected the default control keys to be present"
        );
        // Builder runs end-to-end (also adds the mouse axes) without panicking.
        let _ = InputBindingsSettings::default().input_map().unwrap();
        let _ = UserIntent::MoveForward;
    }

    #[test]
    fn persisted_keymap_inherits_new_defaults_without_overwriting_intentional_empty() {
        let settings: InputBindingsSettings =
            serde_json::from_str(r#"{"forward":["KeyI"],"speed_boost":[]}"#).unwrap();

        assert_eq!(settings.key_code("KeyI").unwrap(), Some(KeyCode::KeyI));
        assert_eq!(settings.key_code("KeyW").unwrap(), None);
        assert_eq!(settings.key_code("ShiftLeft").unwrap(), None);
        assert_eq!(settings.key_code("KeyS").unwrap(), Some(KeyCode::KeyS));
    }

    /// Opposing movement axes must be independent when held together.  In
    /// particular, Q+W is a valid down/forward diagonal just like Q+S and
    /// W+E; a regression in the key-to-intent layer must not drop the forward
    /// intent merely because the vertical key is pressed at the same time.
    #[test]
    fn diagonal_keyboard_intents_keep_both_axes_active() {
        use bevy::input::InputPlugin;
        use leafwing_input_manager::prelude::InputManagerPlugin;

        let mut app = App::new();
        app.add_plugins((
            bevy::time::TimePlugin,
            InputPlugin,
            InputManagerPlugin::<UserIntent>::default(),
        ));
        let entity = app
            .world_mut()
            .spawn((
                ActionState::<UserIntent>::default(),
                InputBindingsSettings::default().input_map().unwrap(),
            ))
            .id();

        // Exercise the real transition order: W is already held when Q goes
        // down.  Pressing both in one input batch does not cover the
        // just-pressed/held-state path used by a player.
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyW);
        app.update();

        let state = app
            .world()
            .entity(entity)
            .get::<ActionState<UserIntent>>()
            .expect("input manager action state");
        assert!(state.pressed(&UserIntent::MoveForward));
        assert!(!state.pressed(&UserIntent::MoveDown));

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyQ);
        app.update();
        let state = app
            .world()
            .entity(entity)
            .get::<ActionState<UserIntent>>()
            .expect("action state after Q transition");
        assert!(state.pressed(&UserIntent::MoveDown));
        assert!(state.pressed(&UserIntent::MoveForward));

        // Check the other diagonal with the same vertical direction.  This
        // catches an axis implementation that accidentally treats forward as
        // mutually exclusive with one of the elevation signs.
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::KeyW);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyS);
        app.update();
        let state = app
            .world()
            .entity(entity)
            .get::<ActionState<UserIntent>>()
            .expect("input manager action state");
        assert!(state.pressed(&UserIntent::MoveDown));
        assert!(state.pressed(&UserIntent::MoveBackward));
    }

    #[derive(Resource, Default)]
    struct VesselControlObserved(Vec<(Entity, Vec<(String, f64)>)>);

    fn observe_vessel_control(
        trigger: On<lunco_cosim::SetPorts>,
        mut observed: ResMut<VesselControlObserved>,
    ) {
        let event = trigger.event();
        observed.0.push((event.target, event.writes.clone()));
    }

    /// Possession redirects the shared keyboard action state to the authored
    /// vessel binding. This is the complete desktop control seam: a physical
    /// `KeyW` updates the avatar's leafwing state, `ControllerLink` selects the
    /// vessel, and `ControlBinding` produces its named command port.
    #[test]
    fn possessed_avatar_keyboard_drives_the_authored_vessel() {
        use bevy::input::InputPlugin;
        use leafwing_input_manager::prelude::InputManagerPlugin;

        let mut app = App::new();
        app.add_plugins((
            bevy::time::TimePlugin,
            InputPlugin,
            InputManagerPlugin::<UserIntent>::default(),
        ));
        app.insert_resource(lunco_core::NetworkRole::Host)
            .init_resource::<lunco_core::SimTick>()
            .init_resource::<lunco_core::OwnedInputLog>()
            .init_resource::<VesselControlObserved>()
            .add_observer(observe_vessel_control)
            .add_systems(FixedUpdate, drive_from_bindings);

        let vessel = app
            .world_mut()
            .spawn((
                lunco_core::GlobalEntityId::from_raw(0xCAFE),
                lunco_core::InputPorts::new(&["throttle", "steer", "brake"]),
                ControlBinding::from_intent_entries(&[
                    ("forward".into(), "throttle".into(), 1.0),
                    ("backward".into(), "throttle".into(), -1.0),
                ])
                .expect("authored rover binding"),
            ))
            .id();
        let avatar = app
            .world_mut()
            .spawn((
                ControllerLink {
                    vessel_entity: vessel,
                },
                ActionState::<UserIntent>::default(),
                InputBindingsSettings::default().input_map().unwrap(),
            ))
            .id();

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyW);
        app.update();
        app.world_mut().run_schedule(FixedUpdate);

        let observed = &app.world().resource::<VesselControlObserved>().0;
        assert_eq!(
            observed.last(),
            Some(&(vessel, vec![("throttle".into(), 1.0)])),
            "KeyW on the possessed avatar must reach the vessel's named control surface"
        );
        assert!(app
            .world()
            .entity(avatar)
            .get::<ActionState<UserIntent>>()
            .expect("avatar action state")
            .pressed(&UserIntent::MoveForward));
    }

    /// Shift is a movement modifier, not a second input path.  It must remain
    /// active when Q/E transitions while the modifier is held, regardless of
    /// which physical Shift key was pressed first.
    #[test]
    fn shift_composes_with_both_vertical_movement_intents() {
        use bevy::input::InputPlugin;
        use leafwing_input_manager::prelude::InputManagerPlugin;

        let mut app = App::new();
        app.add_plugins((
            bevy::time::TimePlugin,
            InputPlugin,
            InputManagerPlugin::<UserIntent>::default(),
        ));
        let entity = app
            .world_mut()
            .spawn((
                ActionState::<UserIntent>::default(),
                InputBindingsSettings::default().input_map().unwrap(),
            ))
            .id();

        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.press(KeyCode::KeyQ);
        app.update();
        let state = app
            .world()
            .entity(entity)
            .get::<ActionState<UserIntent>>()
            .expect("input manager action state");
        assert!(state.pressed(&UserIntent::MoveDown));
        assert!(!state.pressed(&UserIntent::SpeedBoost));

        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.press(KeyCode::ShiftLeft);
        app.update();
        let state = app
            .world()
            .entity(entity)
            .get::<ActionState<UserIntent>>()
            .expect("action state after left Shift transition");
        assert!(state.pressed(&UserIntent::MoveDown));
        assert!(state.pressed(&UserIntent::SpeedBoost));

        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.release(KeyCode::KeyQ);
        keys.press(KeyCode::KeyE);
        app.update();
        let state = app
            .world()
            .entity(entity)
            .get::<ActionState<UserIntent>>()
            .expect("action state after Q-to-E transition");
        assert!(!state.pressed(&UserIntent::MoveDown));
        assert!(state.pressed(&UserIntent::MoveUp));
        assert!(state.pressed(&UserIntent::SpeedBoost));

        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.release(KeyCode::ShiftLeft);
        keys.press(KeyCode::ShiftRight);
        app.update();
        let state = app
            .world()
            .entity(entity)
            .get::<ActionState<UserIntent>>()
            .expect("action state after right Shift handoff");
        assert!(state.pressed(&UserIntent::MoveUp));
        assert!(state.pressed(&UserIntent::SpeedBoost));
    }

    /// The self-driver and the free-avatar movement consumer share the
    /// interaction schedule.  Both components of a diagonal must be visible
    /// to the consumer in the same step; a one-step-late deferred `SetPorts`
    /// application is not sufficient for movement.
    #[test]
    fn interaction_control_flushes_diagonal_ports_before_the_consumer() {
        use lunco_time::InteractionSchedule;

        let mut app = App::new();
        app.add_plugins((
            bevy::time::TimePlugin,
            lunco_time::TimePlugin,
            lunco_cosim::CoSimPlugin,
        ));
        app.configure_sets(InteractionSchedule, InteractionControlSet);
        app.add_systems(
            InteractionSchedule,
            drive_self_drivers.in_set(InteractionControlSet),
        );
        app.add_systems(
            InteractionSchedule,
            observe_interaction_ports.after(InteractionControlSet),
        );
        app.init_resource::<InteractionObserved>();

        let mut state = ActionState::<UserIntent>::default();
        state.press(&UserIntent::MoveDown);
        state.press(&UserIntent::MoveForward);
        state.press(&UserIntent::SpeedBoost);
        app.world_mut().spawn((
            state,
            lunco_core::InputPorts::new(&["forward", "up", "speed_boost"]),
            ControlBinding {
                binds: vec![
                    (UserIntent::MoveForward, "forward".into(), 1.0),
                    (UserIntent::MoveDown, "up".into(), -1.0),
                    (UserIntent::SpeedBoost, "speed_boost".into(), 1.0),
                ],
            },
        ));

        app.world_mut().run_schedule(InteractionSchedule);

        assert_eq!(
            app.world().resource::<InteractionObserved>().0,
            Some((1.0, -1.0, 1.0)),
            "the consumer must see Q, W, and Shift in the same interaction step"
        );
    }

    #[derive(Resource, Default)]
    struct LanderControlObserved(Option<(f64, f64, f64, f64)>);

    fn observe_lander_control_ports(
        q: Query<&lunco_core::InputPorts>,
        mut observed: ResMut<LanderControlObserved>,
    ) {
        let inputs = q.single().expect("the lander input surface");
        observed.0 = Some((
            inputs.cmd("external_throttle"),
            inputs.cmd("pitch"),
            inputs.cmd("roll"),
            inputs.cmd("yaw"),
        ));
    }

    /// The lander control contract is a real producer-to-consumer path:
    /// semantic intents resolve through the authored-shaped binding and arrive
    /// on the Modelica input surface in one interaction step. `Action` is not a
    /// throttle synonym, so F cannot both toggle autopilot and fire the engine.
    #[test]
    fn lander_controls_route_thrust_and_attitude_without_autopilot_action() {
        use lunco_time::InteractionSchedule;

        let mut app = App::new();
        app.add_plugins((
            bevy::time::TimePlugin,
            lunco_time::TimePlugin,
            lunco_cosim::CoSimPlugin,
        ));
        app.configure_sets(InteractionSchedule, InteractionControlSet);
        app.add_systems(
            InteractionSchedule,
            drive_self_drivers.in_set(InteractionControlSet),
        );
        app.add_systems(
            InteractionSchedule,
            observe_lander_control_ports.after(InteractionControlSet),
        );
        app.init_resource::<LanderControlObserved>();

        let mut state = ActionState::<UserIntent>::default();
        state.press(&UserIntent::Thrust);
        state.press(&UserIntent::MoveForward);
        state.press(&UserIntent::MoveLeft);
        state.press(&UserIntent::MoveDown);
        let binding = ControlBinding::from_intent_entries(&[
            ("thrust".into(), "external_throttle".into(), 1.0),
            ("forward".into(), "pitch".into(), -1.0),
            ("left".into(), "roll".into(), 1.0),
            ("yaw_left".into(), "yaw".into(), 1.0),
        ])
        .expect("lander controls must have an authored binding");
        assert!(!binding.has_intent(UserIntent::Action));

        app.world_mut().spawn((
            state,
            lunco_core::InputPorts::new(&["external_throttle", "pitch", "roll", "yaw"]),
            binding,
        ));
        app.world_mut().run_schedule(InteractionSchedule);

        assert_eq!(
            app.world().resource::<LanderControlObserved>().0,
            Some((1.0, -1.0, 1.0, 1.0)),
            "thrust and W/A/Q must reach the lander's command surface together"
        );
    }

    /// Pausing the SIM must not paralyse the USER: the free avatar's self-drive rides
    /// the interaction cadence, so its `SetPorts` keep flowing with `Time<Virtual>`
    /// paused — while `FixedUpdate` (the sim tick, and `drive_from_bindings` with it)
    /// is genuinely frozen. Both halves are asserted: without the frozen-tick control
    /// this test would pass on an app that simply never paused.
    #[test]
    fn a_paused_sim_still_drives_the_free_avatar() {
        use lunco_time::{InteractionSchedule, InteractionStep, TimeTransport, TransportMode};

        let mut app = App::new();
        // Bevy's time plugin drives `Time<Real>` from the wall clock (and the fixed
        // loop from `Time<Virtual>`); ours adds the transport + the interaction cadence.
        app.add_plugins((bevy::time::TimePlugin, lunco_time::TimePlugin));
        app.add_systems(InteractionSchedule, drive_self_drivers);

        // Pause through the REAL path: the transport, which `advance_world_clock`
        // projects onto `Time<Virtual>`'s paused flag.
        app.world_mut().resource_mut::<TimeTransport>().mode = TransportMode::Paused;

        // Control: did the sim tick really stop this update?
        #[derive(Resource, Default)]
        struct Ticks(u32);
        app.init_resource::<Ticks>();
        app.add_systems(FixedUpdate, |mut t: ResMut<Ticks>| t.0 += 1);

        // Collect the port writes the self-drive emits.
        #[derive(Resource, Default)]
        struct Writes(Vec<(String, f64)>);
        app.init_resource::<Writes>();
        app.add_observer(
            |trigger: On<lunco_cosim::SetPorts>, mut w: ResMut<Writes>| {
                w.0.extend(trigger.event().writes.iter().cloned());
            },
        );

        // A free avatar: its own input + its own binding, no `ControllerLink`.
        let mut state = ActionState::<UserIntent>::default();
        state.press(&UserIntent::MoveForward);
        app.world_mut().spawn((
            state,
            ControlBinding {
                binds: vec![(UserIntent::MoveForward, "forward".into(), 1.0)],
            },
        ));

        // Frame 1 seeds the clocks (and runs Startup); only frame 2 is measured.
        app.update();
        app.world_mut().resource_mut::<Ticks>().0 = 0;
        app.world_mut().resource_mut::<Writes>().0.clear();

        // Let real time pass — the interaction step drains the WALL clock, so this is
        // the only thing that must move for the avatar to keep driving.
        let step = app.world().resource::<InteractionStep>().step_secs;
        std::thread::sleep(std::time::Duration::from_secs_f64(step * 1.5));
        app.update();

        assert_eq!(
            app.world().resource::<Ticks>().0,
            0,
            "control: a paused sim must not run FixedUpdate — otherwise this test \
             proves nothing about the interaction cadence"
        );
        let writes = &app.world().resource::<Writes>().0;
        assert!(
            !writes.is_empty(),
            "the free avatar's setpoints must keep flowing while the sim is paused"
        );
        assert!(
            writes.iter().all(|w| w == &("forward".to_string(), 1.0)),
            "every emitted write is the bound setpoint (got {writes:?})"
        );
    }
}
