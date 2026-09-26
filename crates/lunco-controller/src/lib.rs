//! Input mapping and controller translation for simulation vessels.
//!
//! This crate translates user input into the ONE generic vessel control command,
//! [`lunco_cosim_core::commands::SetPorts`] — a batch of named input-port writes — through a
//! **two-stage, fully data-driven** mapping that reuses the existing
//! [`lunco_control_core::UserIntent`] input-abstraction (leafwing) rather than reading
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
//!   paralyse the user. A control producer adds a [`ControlLink`], which moves that
//!   entity to the first system by query, not by a flag.
//!
//! Both share stage 1 ([`intent_held`]) and stage 2 ([`ControlBinding::resolve`]).
//! Because control is keyed by *intent*, anything internal (rhai, mission logic, AI)
//! can drive a vessel by naming intents — the same consistent vocabulary. All writes
//! land through the same [`lunco_port_core::ports::PortRegistry`].

use bevy::input::{
    ButtonState,
    keyboard::{Key, KeyCode, KeyboardInput, NativeKey},
    mouse::{MouseButton, MouseButtonInput, MouseScrollUnit, MouseWheel},
    touch::TouchPhase,
};
use bevy::prelude::*;
use bevy::window::{CursorMoved, PrimaryWindow, WindowEvent};
use leafwing_input_manager::prelude::ActionState;
use lunco_command_contracts::{Ack, OpId};
use lunco_control_core::ControlLink;
use lunco_control_core::{
    ControlBinding, InteractionControlSet, UserIntent, ensure_control_plugin,
};
use lunco_core::{Command, on_command, register_commands};
use lunco_hooks::HookValue;
use lunco_input_core::InputBindingsSettings;
use serde::{Deserialize, Serialize};

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

/// A physical keyboard or pointer event delivered through Bevy's native input
/// message fan-out. The command is intentionally device-level: Rhai composes
/// these events into clicks, drags, chords, and workflows while picking, egui,
/// input bindings, and application tools continue to consume their normal
/// event streams.
#[derive(Reflect, Serialize, Deserialize, Clone, PartialEq, Debug)]
#[reflect(Clone, PartialEq)]
pub enum WindowInputEvent {
    /// A physical key transition. `key` accepts a Bevy `KeyCode` name such as
    /// `KeyW` or `AltLeft`, plus the active keymap's compact label such as `W`.
    Key {
        key: String,
        state: WindowInputState,
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        repeat: bool,
    },
    /// Move the primary pointer to logical window coordinates.
    PointerMove { x: f32, y: f32 },
    /// Move the primary pointer and transition a mouse button at that position.
    PointerButton {
        button: WindowPointerButton,
        state: WindowInputState,
        x: f32,
        y: f32,
    },
    /// Emit a native mouse-wheel event in line units.
    Scroll { x: f32, y: f32 },
}

/// Press/release state used by [`WindowInputEvent`].
#[derive(Reflect, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[reflect(Clone, PartialEq)]
pub enum WindowInputState {
    Pressed,
    Released,
}

impl WindowInputState {
    fn button_state(self) -> ButtonState {
        match self {
            Self::Pressed => ButtonState::Pressed,
            Self::Released => ButtonState::Released,
        }
    }
}

/// Pointer buttons supported by Bevy's picking and egui mouse paths.
#[derive(Reflect, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[reflect(Clone, PartialEq)]
pub enum WindowPointerButton {
    #[serde(alias = "left")]
    Primary,
    #[serde(alias = "right")]
    Secondary,
    Middle,
}

impl WindowPointerButton {
    fn mouse_button(self) -> MouseButton {
        match self {
            Self::Primary => MouseButton::Left,
            Self::Secondary => MouseButton::Right,
            Self::Middle => MouseButton::Middle,
        }
    }
}

/// Inject one native-style input event into the local application window.
///
/// This is the generic automation boundary for Rhai, API clients, playback,
/// and accessibility tooling. It does not invoke a scene tool or semantic
/// command directly. Instead, the next input phase receives the same Bevy
/// `WindowEvent` plus typed keyboard/mouse messages that the winit backend
/// normally emits, so every existing consumer follows its ordinary path.
#[Command]
pub struct InjectWindowInput {
    pub event: WindowInputEvent,
}

#[derive(Message)]
struct PendingWindowInput(WindowInputEvent);

/// The window cursor is temporarily projected for the frame that consumes an
/// injected pointer event. Restoring the authored/native value before Bevy's
/// winit synchronization prevents automation from asking Wayland/X11 to move
/// the operating-system pointer while still letting systems that read
/// `Window::cursor_position()` follow the injected gesture.
#[derive(Resource, Default)]
struct InjectedCursorRestore(Option<(Entity, Option<Vec2>)>);

fn finite_position(x: f32, y: f32) -> Result<Vec2, String> {
    if !x.is_finite() || !y.is_finite() {
        return Err("window input position must be finite".to_string());
    }
    Ok(Vec2::new(x, y))
}

fn finite_scroll(x: f32, y: f32) -> Result<(), String> {
    if x.is_finite() && y.is_finite() {
        Ok(())
    } else {
        Err("window input scroll values must be finite".to_string())
    }
}

fn resolve_window_key(settings: &InputBindingsSettings, label: &str) -> Result<KeyCode, String> {
    let label = label.trim();
    if label.is_empty() {
        return Err("window input key must not be empty".to_string());
    }
    if let Some(key) = settings.key_code(label)? {
        return Ok(key);
    }
    serde_json::from_value(serde_json::Value::String(label.to_string()))
        .map_err(|_| format!("unknown Bevy key code '{label}'"))
}

fn validate_window_input(
    settings: &InputBindingsSettings,
    event: &WindowInputEvent,
) -> Result<(), String> {
    match event {
        WindowInputEvent::Key { key, .. } => {
            resolve_window_key(settings, key)?;
        }
        WindowInputEvent::PointerMove { x, y } | WindowInputEvent::PointerButton { x, y, .. } => {
            finite_position(*x, *y)?;
        }
        WindowInputEvent::Scroll { x, y } => finite_scroll(*x, *y)?,
    }
    Ok(())
}

fn logical_key(key_code: KeyCode, text: Option<&str>) -> Key {
    if let Some(text) = text.filter(|text| !text.is_empty()) {
        return Key::Character(text.into());
    }
    match key_code {
        KeyCode::AltLeft | KeyCode::AltRight => Key::Alt,
        KeyCode::ControlLeft | KeyCode::ControlRight => Key::Control,
        KeyCode::ShiftLeft | KeyCode::ShiftRight => Key::Shift,
        KeyCode::SuperLeft | KeyCode::SuperRight => Key::Super,
        _ => Key::Unidentified(NativeKey::Unidentified),
    }
}

#[on_command(InjectWindowInput)]
fn on_inject_window_input(
    trigger: On<InjectWindowInput>,
    settings: Res<InputBindingsSettings>,
    primary_window: Query<Entity, With<PrimaryWindow>>,
    mut pending: MessageWriter<PendingWindowInput>,
) -> Result<Ack, String> {
    let cmd = trigger.event();
    validate_window_input(&settings, &cmd.event)?;
    primary_window
        .single()
        .map_err(|_| "window input requires exactly one primary window".to_string())?;
    pending.write(PendingWindowInput(cmd.event.clone()));
    Ok(Ack::with_data(
        OpId::new(),
        HookValue::map([("queued", HookValue::Bool(true))]),
    ))
}

/// Fan out pending input through the same typed messages that `bevy_winit`
/// emits after receiving an operating-system event. Keeping the aggregate
/// `WindowEvent` and typed messages together is important: picking/egui read
/// the aggregate stream, while input state and keyboard focus read the typed
/// streams.
fn emit_pending_window_input(
    mut pending: MessageReader<PendingWindowInput>,
    mut windows: Query<(Entity, &mut Window), With<PrimaryWindow>>,
    settings: Res<InputBindingsSettings>,
    mut cursor_restore: ResMut<InjectedCursorRestore>,
    mut window_events: MessageWriter<WindowEvent>,
    mut cursor_moved: MessageWriter<CursorMoved>,
    mut keyboard_input: MessageWriter<KeyboardInput>,
    mut mouse_button_input: MessageWriter<MouseButtonInput>,
    mut mouse_wheel: MessageWriter<MouseWheel>,
) {
    let Ok((window, mut window_state)) = windows.single_mut() else {
        return;
    };
    for PendingWindowInput(event) in pending.read() {
        match event {
            WindowInputEvent::Key {
                key,
                state,
                text,
                repeat,
            } => {
                let Ok(key_code) = resolve_window_key(&settings, key) else {
                    continue;
                };
                let input = KeyboardInput {
                    key_code,
                    logical_key: logical_key(key_code, text.as_deref()),
                    state: state.button_state(),
                    text: text
                        .as_deref()
                        .filter(|value| !value.is_empty())
                        .map(Into::into),
                    repeat: *repeat,
                    window,
                };
                window_events.write(WindowEvent::KeyboardInput(input.clone()));
                keyboard_input.write(input);
            }
            WindowInputEvent::PointerMove { x, y } => {
                let position = Vec2::new(*x, *y);
                if cursor_restore.0.is_none() {
                    cursor_restore.0 = Some((window, window_state.cursor_position()));
                }
                window_state.set_cursor_position(Some(position));
                let moved = CursorMoved {
                    window,
                    position,
                    delta: None,
                };
                window_events.write(WindowEvent::CursorMoved(moved.clone()));
                cursor_moved.write(moved);
            }
            WindowInputEvent::PointerButton {
                button,
                state,
                x,
                y,
            } => {
                let position = Vec2::new(*x, *y);
                if cursor_restore.0.is_none() {
                    cursor_restore.0 = Some((window, window_state.cursor_position()));
                }
                window_state.set_cursor_position(Some(position));
                let moved = CursorMoved {
                    window,
                    position,
                    delta: None,
                };
                window_events.write(WindowEvent::CursorMoved(moved.clone()));
                cursor_moved.write(moved);
                let input = MouseButtonInput {
                    button: button.mouse_button(),
                    state: state.button_state(),
                    window,
                };
                window_events.write(WindowEvent::MouseButtonInput(input));
                mouse_button_input.write(input);
            }
            WindowInputEvent::Scroll { x, y } => {
                let input = MouseWheel {
                    unit: MouseScrollUnit::Line,
                    x: *x,
                    y: *y,
                    window,
                    phase: TouchPhase::Moved,
                };
                window_events.write(WindowEvent::MouseWheel(input));
                mouse_wheel.write(input);
            }
        }
    }
}

fn restore_injected_cursor(
    mut cursor_restore: ResMut<InjectedCursorRestore>,
    mut windows: Query<&mut Window>,
) {
    let Some((window, position)) = cursor_restore.0.take() else {
        return;
    };
    if let Ok(mut window_state) = windows.get_mut(window) {
        window_state.set_cursor_position(position);
    }
}

/// Force an intent held or released, as if a key were pressed — the headless way to
/// drive a possessed vessel over the API or from rhai.
///
/// `held = true` is "stuck" (the key is down and stays down); `held = false` is
/// "unstuck" (released). This command remains the level-triggered surface for
/// driving a held control value. Use [`SimulateIntentEdge`] for an atomic
/// momentary press/release or pulse. The named intent is the USD control
/// vocabulary (`forward`, `action`, `yaw_left`, …), parsed by
/// [`lunco_control_core::parse_user_intent`], so it matches whatever a vessel's
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

/// Deliver one atomic target-scoped semantic edge without requiring callers to
/// emulate a pulse with ordered `held: true` / `held: false` commands.
///
/// This is the API/Rhai/network entry point. The handler validates the shared
/// intent vocabulary and emits [`lunco_control_core::SemanticIntentEdge`]; it does not
/// decide which port or mechanism the consuming Twin should actuate.
#[Command]
pub struct SimulateIntentEdge {
    /// The entity whose semantic control surface receives the edge.
    #[authz_target]
    pub target: Entity,
    /// Intent name (`action`, `release`, `forward`, …).
    pub intent: String,
    /// `pressed`, `released`, or `pulse`.
    pub edge: String,
}

impl Default for SimulateIntentEdge {
    fn default() -> Self {
        Self {
            target: Entity::PLACEHOLDER,
            intent: String::new(),
            edge: String::new(),
        }
    }
}

fn parse_intent_edge(name: &str) -> Option<lunco_control_core::SemanticIntentEdgeKind> {
    match name.trim().to_ascii_lowercase().as_str() {
        "pressed" | "press" => Some(lunco_control_core::SemanticIntentEdgeKind::Pressed),
        "released" | "release" => Some(lunco_control_core::SemanticIntentEdgeKind::Released),
        "pulse" => Some(lunco_control_core::SemanticIntentEdgeKind::Pulse),
        _ => None,
    }
}

#[on_command(SimulateIntentEdge)]
fn on_simulate_intent_edge(
    _trigger: On<SimulateIntentEdge>,
    active_command: Res<lunco_core::ActiveCommandId>,
    mut commands: Commands,
) -> Result<Ack, String> {
    let Some(intent) = lunco_control_core::parse_user_intent(&cmd.intent) else {
        return Err(format!("unknown semantic intent '{}'", cmd.intent));
    };
    if cmd.target == Entity::PLACEHOLDER {
        return Err("semantic intent edge requires a target entity".to_string());
    }
    let Some(kind) = parse_intent_edge(&cmd.edge) else {
        return Err(format!(
            "unknown semantic edge '{}'; expected pressed, released, or pulse",
            cmd.edge
        ));
    };
    commands.trigger(lunco_control_core::SemanticIntentEdge {
        target: cmd.target,
        intent,
        kind,
        correlation_id: active_command.get().unwrap_or_else(|| OpId::new().0),
    });
    Ok(Ack::with_data(
        OpId::new(),
        HookValue::map([
            ("target", HookValue::str(format!("{:?}", cmd.target))),
            ("intent", HookValue::str(cmd.intent.clone())),
            ("edge", HookValue::str(kind.as_str())),
        ]),
    ))
}

/// Project the typed edge onto the existing script/telemetry event bus. Rhai
/// scenarios can consume `intent.edge` in `on_event` without importing this
/// crate or creating a second event transport.
fn project_intent_edge(
    trigger: On<lunco_control_core::SemanticIntentEdge>,
    q_gid: Query<&lunco_core::GlobalEntityId>,
    mut causal_trace: ResMut<lunco_control_core::CausalTrace>,
    mut commands: Commands,
) {
    use lunco_telemetry_core::TelemetryValue;
    use std::collections::BTreeMap;

    let edge = trigger.event();
    let target_gid = q_gid.get(edge.target).ok().copied();
    causal_trace.record(edge, target_gid);
    let mut data = BTreeMap::new();
    data.insert(
        "intent".to_string(),
        TelemetryValue::String(edge.intent.canonical_name().to_string()),
    );
    data.insert(
        "edge".to_string(),
        TelemetryValue::String(edge.kind.as_str().to_string()),
    );
    data.insert(
        "target_gid".to_string(),
        TelemetryValue::I64(target_gid.map_or(0, |gid| gid.get() as i64)),
    );
    data.insert(
        "correlation_id".to_string(),
        TelemetryValue::I64(edge.correlation_id as i64),
    );
    commands.trigger(lunco_telemetry_core::TelemetryEvent {
        name: "intent.edge".to_string(),
        source: target_gid.map_or(0, |gid| gid.get()),
        severity: lunco_telemetry_core::Severity::Info,
        data: TelemetryValue::Map(data),
        timestamp: 0.0,
        sim_secs: 0.0,
        sim_tick: 0,
    });
}

#[on_command(SimulateIntent)]
fn on_simulate_intent(trigger: On<SimulateIntent>, mut sim: ResMut<SimulatedIntents>) {
    let cmd = trigger.event();
    let Some(intent) = lunco_control_core::parse_user_intent(&cmd.intent) else {
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
/// The generic verb behind [`lunco_core_session::ControlPathRegistry`]. A mission
/// script computes the DOMAIN fact and states the CONSEQUENCE here; an authored
/// policy ([`lunco_core_session::AUTHORIZE_HOOK`]) then decides what to refuse.
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
    mut paths: ResMut<lunco_core_session::ControlPathRegistry>,
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

register_commands!(
    on_simulate_intent,
    on_simulate_intent_edge,
    on_set_control_path,
    on_inject_window_input,
);

/// Plugin for managing vessel input and command translation.
pub struct LunCoControllerPlugin;

/// Clear controller state that names scene entities. A released intent or
/// control-path blackout must not be applied to a replacement scene that reuses
/// the same entity slot or global id.
fn reset_scene_control_state(
    mut intents: ResMut<SimulatedIntents>,
    mut paths: ResMut<lunco_core_session::ControlPathRegistry>,
) {
    intents.0.clear();
    paths.clear();
}

impl Plugin for LunCoControllerPlugin {
    fn build(&self, app: &mut App) {
        ensure_control_plugin(app);
        // NOTE: OwnedInputLog / AppliedInputSeq are always-on session substrate
        // owned by LunCoCoreSessionPlugin (lunco-core-session). The controller's
        // observers consume them unconditionally, but it does NOT init them here;
        // the session plugin is the single resource owner.
        //
        // Input → port writes are EMITTED once per fixed tick (so the
        // prediction replay is a clean 1:1 loop over `InputFrame`s).
        // Suppressed during a rollback replay: re-simulation feeds the RECORDED
        // input for each replayed tick, so regenerating input from the live keyboard
        // mid-replay would overwrite the very history we are replaying (and mint new
        // seqs for ticks that already happened).
        if !app.is_plugin_added::<lunco_input_core::InputBindingsPlugin>() {
            app.add_plugins(lunco_input_core::InputBindingsPlugin);
        }
        app.init_resource::<SimulatedIntents>()
            .init_resource::<InjectedCursorRestore>();
        app.add_message::<PendingWindowInput>();
        lunco_core::MarkClientLocalExt::mark_client_local::<InjectWindowInput>(app);
        app.init_resource::<lunco_core_session::CommandPolicyRegistry>();
        app.world_mut()
            .resource_mut::<lunco_core_session::CommandPolicyRegistry>()
            .register(
                "SimulateIntentEdge",
                lunco_core_session::CommandPolicy::OWNED_CONTROL,
            );
        app.add_observer(project_intent_edge);
        app.add_systems(lunco_core::SceneTeardown, reset_scene_control_state);
        // The blackout table the authorization gate reads. Empty by default, so an
        // app that never declares one is byte-for-byte unchanged.
        app.init_resource::<lunco_core_session::ControlPathRegistry>();
        register_all_commands(app);
        app.add_systems(
            FixedUpdate,
            // Ahead of wire propagation, so the `Port` writes this tick emits
            // reach their wired targets in the same tick. Unordered, propagation
            // may read the port before or after this system depending on the
            // schedule's parallel layout, and prediction diverges from the host
            // on that coin flip.
            drive_from_bindings
                .run_if(lunco_core_runtime::not_rolling_back)
                .run_if(lunco_time::simulation_is_running)
                .before(lunco_core_runtime::ControlDacSet),
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
        // a `ControlLink`, which excludes it from `q_self`, so its input keeps
        // riding `FixedUpdate` and a paused rover stays put.
        if !app.is_plugin_added::<lunco_time::TimePlugin>() {
            app.add_plugins(lunco_time::TimePlugin);
        }
        app.configure_sets(lunco_time::InteractionSchedule, InteractionControlSet);
        app.add_systems(
            lunco_time::InteractionSchedule,
            drive_self_drivers.in_set(InteractionControlSet),
        );
        // WindowEvent is consumed by Bevy Picking in First, while the typed
        // keyboard/mouse messages are consumed by InputSystems in PreUpdate.
        // This bridge is only valid for a windowed host: headless/offscreen
        // hosts have no native window-message resources to initialize.
        // Emit before Picking so one pending gesture reaches both consumers in
        // the same frame; the typed messages remain available for PreUpdate.
        app.add_systems(
            First,
            emit_pending_window_input
                .run_if(any_with_component::<PrimaryWindow>)
                .before(bevy::picking::PickingSystems::Input),
        );
        app.add_systems(PostUpdate, restore_injected_cursor);
        // The SINGLE input-bookkeeping chokepoint: every `SetPorts` — keyboard,
        // API, or wire-replayed — flows through this observer, so the client
        // prediction log and the host reconcile-ack no longer depend on how the
        // command was produced.
        app.add_observer(record_control_input);
    }
}

/// The per-vessel **intent → port** binding (stage 2) is
/// [`lunco_control_core::ControlBinding`]
/// — pure data, authored on the VESSEL from USD (`lunco:controlBindings`) or
/// defaulted by topology at possess time. The semantic contract and authored
/// binding implementation live in `lunco-control-core`; this crate only provides
/// the SYSTEM that consumes it
/// ([`drive_from_bindings`]).

/// Cap on the unacked input ring (~2 s at 60 Hz). The reconcile normally drains
/// it to the acked `seq` each snapshot; this only bounds a stalled/disconnected
/// client so the buffer can't grow without limit.
const MAX_INPUT_FRAMES: usize = 128;

/// Magnitude below which a control setpoint counts as "no input" for the
/// prediction-membership signal (`VesselInputLog::last_active_tick`). The
/// controller emits a `SetPorts` every fixed tick even when idle (all zeros), so
/// presence of writes is NOT an activity signal — the *value* is.
const INPUT_EPS: f64 = 1e-3;

#[derive(Default)]
struct VesselInputState {
    ports_active: std::collections::HashMap<Entity, bool>,
    takeover_requested: std::collections::HashSet<(Entity, Entity)>,
    order: Vec<ControllerOrder>,
}

/// One controller's deterministic key in the reusable fixed-step sort buffer.
#[derive(Clone, Copy)]
struct ControllerOrder {
    key: ((bool, u64), (bool, u64), u64),
    source: Entity,
}

/// Put admitted global identities before keys scoped to this World.
fn controller_entity_order(entity: Entity, global_id: Option<u64>) -> (bool, u64) {
    global_id.map_or((true, entity.to_bits()), |id| (false, id))
}

/// Fixed-tick input emission for prediction. Emits a [`lunco_cosim_core::commands::SetPorts`]
/// while a controller is active and once on the active→idle edge, from its
/// [`ControlBinding`] and held keys, stamped with a per-vessel `seq` + `SimTick`.
/// For a vessel this client owns + predicts ([`lunco_core_session::OwnedLocally`]) the
/// frame is buffered for replay by [`record_control_input`]; on host/standalone
/// the command uses `seq = 0`. Silent idle ticks leave the domain writer alone.
fn drive_from_bindings(
    role: Res<lunco_core_session::NetworkRole>,
    tick: Res<lunco_core_runtime::SimTick>,
    mut log: ResMut<lunco_core_session::OwnedInputLog>,
    // Control authority is generic session ownership. If another session owns a
    // target, an active operator intent requests the normal claim transition below;
    // the authored authority policy decides whether the handoff is allowed. Both
    // resources are optional so a controller-only app without the session
    // substrate retains its local input path.
    registry: Option<Res<lunco_core_session::SessionRegistry>>,
    local_session: Option<Res<lunco_core_session::LocalSession>>,
    // The authored authorization POLICY applies to the local keyboard too — see the
    // gate below. All `Option` so a controller-only test app without the session
    // substrate still runs ungated.
    rbac: Option<Res<lunco_core_session::SessionRbac>>,
    control_paths: Option<Res<lunco_core_session::ControlPathRegistry>>,
    q_ctrl: Query<(Entity, &ControlLink, &ActionState<UserIntent>)>,
    q_binding: Query<&ControlBinding>,
    q_vessel: Query<(
        &lunco_core::GlobalEntityId,
        Has<lunco_core_session::OwnedLocally>,
    )>,
    // egui keyboard capture (published by `lunco-workbench`). While a text field
    // is focused we treat every intent as released so a keypress typed into the UI
    // doesn't also drive the vessel — see the `held` closure below. `Option` so a
    // controller-only test app without the workbench still runs (no gate).
    egui_focus: Option<Res<lunco_control_core::EguiFocus>>,
    // Intents forced by `SimulateIntent` — the headless/API/rhai stand-in for keys.
    sim_intents: Option<Res<SimulatedIntents>>,
    // Port idle-yield and held-input authority-request state.
    mut input_state: Local<VesselInputState>,
    // Despawned vessels leave `ports_active` — pruned below so a recycled
    // Entity id can't inherit a stale flag and mistime the all-zero batch.
    mut removed_bindings: RemovedComponents<ControlBinding>,
    // Previous semantic state used to publish pressed/released transitions once.
    mut edge_state: Local<std::collections::HashMap<(Entity, UserIntent), bool>>,
    mut commands: Commands,
) {
    // Prune despawned/unbound vessels before reading edges: a recycled Entity
    // id must start from "idle", not the previous vessel's last state.
    for vessel in removed_bindings.read() {
        input_state.ports_active.remove(&vessel);
        edge_state.retain(|(entity, _), _| *entity != vessel);
        input_state
            .takeover_requested
            .retain(|(_, target)| *target != vessel);
    }
    input_state
        .takeover_requested
        .retain(|(source, _)| q_ctrl.contains(*source));

    let client = matches!(*role, lunco_core_session::NetworkRole::Client);

    // When egui holds the keyboard, no local key counts as pressed. `drive_from_
    // bindings` still runs and `resolve` still writes EVERY bound port — now all
    // 0 — so the vessel decelerates to a clean stop rather than latching its last
    // command (as it would if we simply skipped the system).
    let egui_keyboard = egui_focus.is_some_and(|f| f.wants_keyboard);
    let sim_intents = sim_intents.as_deref();
    let held = |vessel: Entity, intent, intents: &ActionState<UserIntent>| {
        intent_held(vessel, intent, intents, sim_intents, egui_keyboard)
    };

    // Query iteration follows ECS storage layout, which can change when
    // unrelated components move entities between archetypes. Reuse this local
    // buffer and order control effects by stable target identity before any
    // intent edge or SetPorts command is emitted.
    input_state.order.clear();
    for (source, link, _) in q_ctrl.iter() {
        let target_id = q_vessel.get(link.target).ok().map(|(id, _)| id.get());
        let source_id = q_vessel.get(source).ok().map(|(id, _)| id.get());
        input_state.order.push(ControllerOrder {
            key: (
                controller_entity_order(link.target, target_id),
                controller_entity_order(source, source_id),
                source.to_bits(),
            ),
            source,
        });
    }
    input_state.order.sort_unstable_by_key(|entry| entry.key);

    for index in 0..input_state.order.len() {
        let source = input_state.order[index].source;
        let Ok((_, link, intents)) = q_ctrl.get(source) else {
            continue;
        };
        // Stage 1 (key→intent) is the shared leafwing `InputMap<UserIntent>`;
        // stage 2 maps this vessel's active intents → summed, clamped port writes.
        // The binding is authored ON THE VESSEL as a USD `Controls` child scope
        // (referencing a shared profile) — skip a vessel that carries none.
        let Ok(binding) = q_binding.get(link.target) else {
            continue;
        };

        let operator_intent_active =
            has_active_control_intent(link.target, binding, intents, sim_intents, egui_keyboard);
        if !operator_intent_active {
            input_state
                .takeover_requested
                .retain(|(owner, _)| *owner != source);
        }

        // The vessel's id (gid + is-it-locally-owned) — used both by the ownership
        // handoff below and the client seq bookkeeping.
        let vessel_id = q_vessel.get(link.target).ok();
        let owns = registry
            .as_ref()
            .zip(local_session.as_ref())
            .zip(vessel_id)
            .is_some_and(|((reg, local), (gid, _))| reg.owns(local.0, gid.get()));

        // The authored authorization policy ([`AUTHORIZE_HOOK`]) gates the LOCAL
        // keyboard, not just the wire and script paths. Without this a policy like
        // "refuse tele-op while the control path is down" was true only for remote
        // and scripted commands, while the student at the keyboard drove straight
        // through it — `authorize()` sits on `sync.rs` and
        // `lunco-scripting-bridge-core`, and
        // this system triggers `SetPorts` directly.
        //
        // `authorize_policy`, NOT the full `authorize`: the role/ownership floor is a
        // wire concern. This path drives locally unowned targets as well as targets
        // already claimed by this session; applying the ownership-gated floor here
        // would break ordinary local play. The policy is what must bind everywhere;
        // the floor stays where it belongs.
        if let (Some(rbac), Some(paths), Some(local), Some((gid, _))) = (
            rbac.as_ref(),
            control_paths.as_ref(),
            local_session.as_ref(),
            vessel_id,
        ) {
            if lunco_core_session::authorize_policy(
                rbac,
                paths,
                local.0,
                "SetPorts",
                Some(gid.get()),
                owns,
            )
            .is_err()
            {
                input_state
                    .takeover_requested
                    .remove(&(source, link.target));
                continue;
            }
        }

        // A foreign session remains the only writer until the shared authority
        // transaction accepts a local claim. An active semantic control intent
        // requests that transaction once; the existing Rhai authority policy
        // decides whether this session may take the endpoint. The input frame is
        // applied on the next fixed tick, after ownership has changed.
        if let (Some(reg), Some(local), Some((gid, _))) =
            (registry.as_ref(), local_session.as_ref(), vessel_id)
        {
            let owner_is_other = reg
                .owner_of(gid.get())
                .is_some_and(|owner| owner != local.0);
            if owner_is_other {
                if operator_intent_active
                    && input_state.takeover_requested.insert((source, link.target))
                {
                    commands.trigger(lunco_core_session::commands::ClaimControl {
                        target: link.target,
                    });
                }
                continue;
            }
        }
        input_state
            .takeover_requested
            .remove(&(source, link.target));

        emit_intent_edges(
            link.target,
            binding,
            intents,
            sim_intents,
            egui_keyboard,
            &mut edge_state,
            &mut commands,
        );

        let writes = binding.resolve(|intent| held(link.target, intent, intents));

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
        // on the same vessel, so authored guidance could not drive a vessel the
        // player possessed. Go
        // SILENT in steady idle and emit exactly ONE all-zero batch on the
        // active→idle edge — ports latch, so a single zero write still gives
        // the clean stop the every-tick stream provided. A pressed key resumes
        // writing immediately: the human always preempts a script mid-drive.
        //
        let active = writes.iter().any(|(_, v)| v.abs() > f64::EPSILON);
        let prev = input_state
            .ports_active
            .insert(link.target, active)
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

        commands.trigger(lunco_cosim_core::commands::SetPorts {
            target: link.target,
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

/// Whether the user has an active semantic control intent for this target.
/// `Action` is included because authored programs may use it as a control
/// action even when the vessel has no matching command port.
fn has_active_control_intent(
    vessel: Entity,
    binding: &ControlBinding,
    intents: &ActionState<UserIntent>,
    sim_intents: Option<&SimulatedIntents>,
    egui_keyboard: bool,
) -> bool {
    intent_held(
        vessel,
        UserIntent::Action,
        intents,
        sim_intents,
        egui_keyboard,
    ) || binding
        .binds
        .iter()
        .any(|(intent, _, _)| intent_held(vessel, *intent, intents, sim_intents, egui_keyboard))
}

/// Publish each authored semantic intent transition exactly once for a target.
/// The state is local to the producer schedule: a possessed vessel and a free
/// avatar cannot inherit one another's edge history when control ownership or
/// the scene changes.
fn emit_intent_edges(
    target: Entity,
    binding: &ControlBinding,
    intents: &ActionState<UserIntent>,
    sim_intents: Option<&SimulatedIntents>,
    egui_keyboard: bool,
    previous: &mut std::collections::HashMap<(Entity, UserIntent), bool>,
    commands: &mut Commands,
) {
    // `Action` is a semantic interaction edge, not a port write. It must be
    // observed even when a control surface has no authored port named
    // `action`; scene programs consume the edge and decide what it means.
    let mut seen = vec![UserIntent::Action];
    let active = intent_held(
        target,
        UserIntent::Action,
        intents,
        sim_intents,
        egui_keyboard,
    );
    let prior = previous
        .insert((target, UserIntent::Action), active)
        .unwrap_or(false);
    if active != prior {
        commands.trigger(lunco_control_core::SemanticIntentEdge {
            target,
            intent: UserIntent::Action,
            kind: if active {
                lunco_control_core::SemanticIntentEdgeKind::Pressed
            } else {
                lunco_control_core::SemanticIntentEdgeKind::Released
            },
            correlation_id: OpId::new().0,
        });
    }
    for (intent, _, _) in &binding.binds {
        if seen.contains(intent) {
            continue;
        }
        seen.push(*intent);
        let active = intent_held(target, *intent, intents, sim_intents, egui_keyboard);
        let prior = previous.insert((target, *intent), active).unwrap_or(false);
        if active != prior {
            commands.trigger(lunco_control_core::SemanticIntentEdge {
                target,
                intent: *intent,
                kind: if active {
                    lunco_control_core::SemanticIntentEdgeKind::Pressed
                } else {
                    lunco_control_core::SemanticIntentEdgeKind::Released
                },
                correlation_id: OpId::new().0,
            });
        }
    }
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
/// Disjoint from [`drive_from_bindings`]'s query by `Without<ControlLink>`: an
/// avatar that possesses a vessel is no longer a self-driver, so its input goes back
/// on the sim tick and freezes with the sim, as pause is meant to.
fn drive_self_drivers(
    q_self: Query<(Entity, &ActionState<UserIntent>, &ControlBinding), Without<ControlLink>>,
    egui_focus: Option<Res<lunco_control_core::EguiFocus>>,
    sim_intents: Option<Res<SimulatedIntents>>,
    mut edge_state: Local<std::collections::HashMap<(Entity, UserIntent), bool>>,
    mut commands: Commands,
) {
    let egui_keyboard = egui_focus.is_some_and(|f| f.wants_keyboard);
    let sim_intents = sim_intents.as_deref();
    for (entity, intents, binding) in q_self.iter() {
        // A self-driver IS its own vessel, so it is its own intent subject.
        emit_intent_edges(
            entity,
            binding,
            intents,
            sim_intents,
            egui_keyboard,
            &mut edge_state,
            &mut commands,
        );
        let writes = binding
            .resolve(|intent| intent_held(entity, intent, intents, sim_intents, egui_keyboard));
        commands.trigger(lunco_cosim_core::commands::SetPorts {
            target: entity,
            writes,
            seq: 0,
            tick: 0,
        });
    }
}

/// The single chokepoint where a [`lunco_cosim_core::commands::SetPorts`] records its input
/// bookkeeping, regardless of origin (local keyboard via [`drive_from_bindings`],
/// the HTTP/MCP API, or a wire-replayed remote input). Unifying it here is what
/// keeps control and prediction on the same path: prediction logging (client) and
/// the reconcile ack (host) no longer depend on *how* the command was made.
fn record_control_input(
    trigger: On<lunco_cosim_core::commands::SetPorts>,
    role: Res<lunco_core_session::NetworkRole>,
    sim_tick: Res<lunco_core_runtime::SimTick>,
    virtual_time: Option<Res<Time<Virtual>>>,
    mut owned_log: ResMut<lunco_core_session::OwnedInputLog>,
    mut applied: ResMut<lunco_core_session::AppliedInputSeq>,
    // Latest local drive input per gid — the render-lead reads it to visually
    // anticipate the rover's motion (presentational only; see `LocalDriveInput`).
    mut drive_input: ResMut<lunco_core_session::LocalDriveInput>,
    // Host-side per-tick input buffer + ownership table: a forwarded client input
    // is queued by seq so `apply_buffered_client_inputs` steps EXACTLY ONE per
    // fixed tick — matching the client's one-input-per-tick prediction, so the two
    // deterministic sims stay in lockstep (no divergence → gentle reconcile).
    reg: Res<lunco_core_session::SessionRegistry>,
    local: Res<lunco_core_session::LocalSession>,
    mut buffered: ResMut<lunco_core_session::BufferedClientInputs>,
    q: Query<(
        &lunco_core::GlobalEntityId,
        Has<lunco_core_session::OwnedLocally>,
    )>,
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
        // Buffer the complete port frame keyed by seq so rollback can re-simulate
        // it and reconciliation can prune it after the host acknowledges it.
        let entry = owned_log.0.entry(g).or_default();
        if entry.frames.back().is_none_or(|f| f.seq != cmd.seq) {
            // Capture the full port actuation for deterministic rollback.
            // `drive_from_bindings` resolves every bound port each tick; API
            // and other partial writes retain the prior value for omitted ports.
            let mut writes = entry
                .frames
                .back()
                .map(|frame| frame.writes.clone())
                .unwrap_or_default();
            for (name, value) in &cmd.writes {
                if let Some((_, latched_value)) = writes
                    .iter_mut()
                    .find(|(latched_name, _)| latched_name == name)
                {
                    *latched_value = *value;
                } else {
                    writes.push((name.clone(), *value));
                }
            }
            entry.frames.push_back(lunco_core_session::InputFrame {
                seq: cmd.seq,
                tick: cmd.tick,
                writes,
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

#[cfg(test)]
mod input_ack_tests {
    use super::*;
    use lunco_command_contracts::SessionId;
    use lunco_core::GlobalEntityId;
    use lunco_core_runtime::SimTick;
    use lunco_core_session::{
        AppliedInputSeq, BufferedClientInputs, LocalDriveInput, LocalSession, NetworkRole,
        OwnedInputLog, SessionRegistry,
    };

    const HOST: SessionId = SessionId(0);
    const CLIENT_A: SessionId = SessionId(11);

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

    fn predicted_client_app(gid: u64) -> (App, Entity) {
        let mut app = App::new();
        app.insert_resource(NetworkRole::Client)
            .insert_resource(LocalSession(CLIENT_A))
            .init_resource::<SimTick>()
            .init_resource::<OwnedInputLog>()
            .init_resource::<AppliedInputSeq>()
            .init_resource::<LocalDriveInput>()
            .init_resource::<BufferedClientInputs>()
            .init_resource::<SessionRegistry>();
        app.add_observer(record_control_input);
        let e = app
            .world_mut()
            .spawn((
                GlobalEntityId::from_raw(gid),
                lunco_core_session::OwnedLocally,
            ))
            .id();
        (app, e)
    }

    fn drive(app: &mut App, target: Entity, seq: u32, steer: f64) {
        app.world_mut()
            .trigger(lunco_cosim_core::commands::SetPorts {
                target,
                writes: vec![("steer".to_string(), steer)],
                seq,
                tick: seq as u64,
            });
        app.update();
    }

    /// The host's fixed-tick consumer, in miniature: exactly what
    /// `apply_buffered_client_inputs` in the networking prediction pipeline does
    /// to the ack.
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

    #[test]
    fn rollback_frames_keep_all_control_ports_and_latch_partial_updates() {
        let gid = 0xBEEF_0004;
        let (mut app, entity) = predicted_client_app(gid);

        app.world_mut()
            .trigger(lunco_cosim_core::commands::SetPorts {
                target: entity,
                writes: vec![
                    ("steer".into(), 0.5),
                    ("arm".into(), 1.0),
                    ("throttle".into(), 0.7),
                ],
                seq: 1,
                tick: 7,
            });
        app.update();

        app.world_mut()
            .trigger(lunco_cosim_core::commands::SetPorts {
                target: entity,
                writes: vec![("steer".into(), -0.25)],
                seq: 2,
                tick: 8,
            });
        app.update();

        let frames = &app.world().resource::<OwnedInputLog>().0[&gid].frames;
        assert_eq!(frames[0].tick, 7);
        assert_eq!(
            frames[0].writes,
            vec![
                ("steer".into(), 0.5),
                ("arm".into(), 1.0),
                ("throttle".into(), 0.7),
            ]
        );
        assert_eq!(frames[1].tick, 8);
        assert_eq!(
            frames[1].writes,
            vec![
                ("steer".into(), -0.25),
                ("arm".into(), 1.0),
                ("throttle".into(), 0.7),
            ]
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_control_core::UserIntent;
    use lunco_core::GlobalEntityId;
    use lunco_input_core::resolved_input_label;

    fn test_bindings() -> InputBindingsSettings {
        InputBindingsSettings::from_json(
            r#"{
                "look_button": "Right",
                "forward": ["KeyW"],
                "backward": ["KeyS"],
                "left": ["KeyA"],
                "right": ["KeyD"],
                "yaw_right": ["KeyE"],
                "yaw_left": ["KeyQ"],
                "speed_boost": ["ShiftLeft", "ShiftRight"],
                "action": ["KeyF"],
                "thrust": ["Space"],
                "brake": ["Space"],
                "release": ["KeyG"],
                "switch_mode": ["KeyV"],
                "pause": ["KeyP"],
                "cancel": ["Backspace", "Escape"],
                "delete_selection": ["Delete"]
            }"#,
        )
        .expect("valid input test fixture")
    }

    #[derive(Resource, Default)]
    struct WindowInputObserved {
        aggregate: Vec<WindowEvent>,
        cursors: Vec<CursorMoved>,
        keys: Vec<KeyboardInput>,
        buttons: Vec<MouseButtonInput>,
        scroll: Vec<MouseWheel>,
    }

    fn collect_window_input(
        mut aggregate: MessageReader<WindowEvent>,
        mut cursors: MessageReader<CursorMoved>,
        mut keys: MessageReader<KeyboardInput>,
        mut buttons: MessageReader<MouseButtonInput>,
        mut scroll: MessageReader<MouseWheel>,
        mut observed: ResMut<WindowInputObserved>,
    ) {
        observed.aggregate.extend(aggregate.read().cloned());
        observed.cursors.extend(cursors.read().cloned());
        observed.keys.extend(keys.read().cloned());
        observed.buttons.extend(buttons.read().cloned());
        observed.scroll.extend(scroll.read().cloned());
    }

    #[test]
    fn injected_window_input_fanout_matches_native_messages() {
        let mut app = App::new();
        app.add_message::<PendingWindowInput>()
            .add_message::<WindowEvent>()
            .add_message::<CursorMoved>()
            .add_message::<KeyboardInput>()
            .add_message::<MouseButtonInput>()
            .add_message::<MouseWheel>()
            .init_resource::<InputBindingsSettings>()
            .init_resource::<InjectedCursorRestore>()
            .init_resource::<WindowInputObserved>()
            .add_systems(
                Update,
                (emit_pending_window_input, collect_window_input).chain(),
            )
            .add_systems(PostUpdate, restore_injected_cursor);
        app.world_mut().spawn((Window::default(), PrimaryWindow));

        app.world_mut()
            .resource_mut::<Messages<PendingWindowInput>>()
            .write(PendingWindowInput(WindowInputEvent::Key {
                key: "KeyA".into(),
                state: WindowInputState::Pressed,
                text: None,
                repeat: false,
            }));
        app.world_mut()
            .resource_mut::<Messages<PendingWindowInput>>()
            .write(PendingWindowInput(WindowInputEvent::PointerButton {
                button: WindowPointerButton::Primary,
                state: WindowInputState::Pressed,
                x: 12.0,
                y: 34.0,
            }));
        app.world_mut()
            .resource_mut::<Messages<PendingWindowInput>>()
            .write(PendingWindowInput(WindowInputEvent::Scroll {
                x: 0.0,
                y: 1.0,
            }));

        app.update();

        let observed = app.world().resource::<WindowInputObserved>();
        assert_eq!(observed.keys.len(), 1);
        assert_eq!(observed.keys[0].key_code, KeyCode::KeyA);
        assert_eq!(observed.buttons.len(), 1);
        assert_eq!(observed.buttons[0].button, MouseButton::Left);
        assert_eq!(observed.cursors.len(), 1);
        assert_eq!(observed.cursors[0].position, Vec2::new(12.0, 34.0));
        assert_eq!(observed.scroll.len(), 1);
        assert_eq!(observed.aggregate.len(), 4);
        assert!(matches!(
            observed.aggregate[0],
            WindowEvent::KeyboardInput(_)
        ));
        assert!(matches!(observed.aggregate[1], WindowEvent::CursorMoved(_)));
        assert!(matches!(
            observed.aggregate[2],
            WindowEvent::MouseButtonInput(_)
        ));
        assert!(matches!(observed.aggregate[3], WindowEvent::MouseWheel(_)));
    }

    #[test]
    fn injected_window_input_is_safe_without_window_event_stream() {
        let mut app = App::new();
        app.add_message::<PendingWindowInput>()
            .init_resource::<InputBindingsSettings>()
            .init_resource::<InjectedCursorRestore>()
            .add_systems(
                Update,
                emit_pending_window_input.run_if(any_with_component::<PrimaryWindow>),
            );

        app.world_mut()
            .resource_mut::<Messages<PendingWindowInput>>()
            .write(PendingWindowInput(WindowInputEvent::PointerMove {
                x: 12.0,
                y: 34.0,
            }));

        app.update();
    }

    #[derive(Resource, Default)]
    struct SemanticEdgeObserved {
        typed: Vec<lunco_control_core::SemanticIntentEdge>,
        telemetry: Vec<lunco_telemetry_core::TelemetryEvent>,
    }

    fn observe_semantic_edge(
        trigger: On<lunco_control_core::SemanticIntentEdge>,
        mut observed: ResMut<SemanticEdgeObserved>,
    ) {
        observed.typed.push(*trigger.event());
    }

    fn observe_edge_telemetry(
        trigger: On<lunco_telemetry_core::TelemetryEvent>,
        mut observed: ResMut<SemanticEdgeObserved>,
    ) {
        if trigger.event().name == "intent.edge" {
            observed.telemetry.push(trigger.event().clone());
        }
    }

    #[test]
    fn semantic_edge_is_atomic_target_scoped_and_script_visible() {
        let mut app = App::new();
        app.init_resource::<lunco_core::CommandResults>()
            .init_resource::<lunco_core::ActiveCommandId>()
            .init_resource::<lunco_control_core::CausalTrace>()
            .init_resource::<SimulatedIntents>()
            .init_resource::<lunco_core_session::CommandPolicyRegistry>()
            .add_observer(observe_semantic_edge)
            .add_observer(observe_edge_telemetry);
        app.world_mut()
            .resource_mut::<lunco_core_session::CommandPolicyRegistry>()
            .register(
                "SimulateIntentEdge",
                lunco_core_session::CommandPolicy::OWNED_CONTROL,
            );
        register_all_commands(&mut app);
        app.add_observer(project_intent_edge);
        app.init_resource::<SemanticEdgeObserved>();

        let target = app
            .world_mut()
            .spawn(lunco_core::GlobalEntityId::from_raw(0x11))
            .id();
        let other = app
            .world_mut()
            .spawn(lunco_core::GlobalEntityId::from_raw(0x22))
            .id();

        assert_eq!(
            app.world()
                .resource::<lunco_core_session::CommandPolicyRegistry>()
                .policy_for("SimulateIntentEdge"),
            lunco_core_session::CommandPolicy::OWNED_CONTROL
        );

        app.world_mut().trigger(SimulateIntentEdge {
            target,
            intent: "release".into(),
            edge: "pulse".into(),
        });
        app.update();

        let observed = app.world().resource::<SemanticEdgeObserved>();
        assert_eq!(observed.typed.len(), 1);
        assert_eq!(observed.typed[0].target, target);
        assert_eq!(observed.typed[0].intent, UserIntent::Release);
        assert_eq!(
            observed.typed[0].kind,
            lunco_control_core::SemanticIntentEdgeKind::Pulse
        );
        assert_ne!(observed.typed[0].correlation_id, 0);
        assert_eq!(observed.typed[0].target, target);
        assert_ne!(observed.typed[0].target, other);
        assert_eq!(observed.telemetry.len(), 1);
        assert_eq!(observed.telemetry[0].source, 0x11);
        let lunco_telemetry_core::TelemetryValue::Map(data) = &observed.telemetry[0].data else {
            panic!("semantic edge telemetry must be structured");
        };
        assert_eq!(
            data["intent"],
            lunco_telemetry_core::TelemetryValue::String("release".into())
        );
        assert_eq!(
            data["edge"],
            lunco_telemetry_core::TelemetryValue::String("pulse".into())
        );
        assert_eq!(
            data["target_gid"],
            lunco_telemetry_core::TelemetryValue::I64(0x11)
        );
        assert_eq!(
            data["correlation_id"],
            lunco_telemetry_core::TelemetryValue::I64(observed.typed[0].correlation_id as i64)
        );
        let trace = app.world().resource::<lunco_control_core::CausalTrace>();
        assert_eq!(trace.len(), 1);
        assert!(
            trace
                .find(
                    lunco_core::GlobalEntityId::from_raw(0x11),
                    observed.typed[0].correlation_id
                )
                .is_some()
        );

        app.world_mut().trigger(SimulateIntentEdge {
            target,
            intent: "release".into(),
            edge: "not-an-edge".into(),
        });
        app.update();
        assert_eq!(
            app.world().resource::<SemanticEdgeObserved>().typed.len(),
            1,
            "an invalid edge must not emit a typed event"
        );
    }

    #[derive(Resource, Default)]
    struct InteractionObserved(Option<(f64, f64, f64)>);

    fn observe_interaction_ports(
        q: Query<&lunco_port_core::InputPorts>,
        mut observed: ResMut<InteractionObserved>,
    ) {
        let inputs = q.single().expect("the free avatar input surface");
        observed.0 = Some((
            inputs.cmd("forward"),
            inputs.cmd("up"),
            inputs.cmd("speed_boost"),
        ));
    }

    #[test]
    fn lander_intent_labels_and_port_signs_are_data_driven() {
        let settings = test_bindings();
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
    fn autopilot_action_is_separate_from_contextual_space_controls() {
        let bindings = test_bindings().key_bindings().unwrap();
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

        let input_map = test_bindings().input_map().unwrap();
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
                test_bindings().input_map().unwrap(),
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
        trigger: On<lunco_cosim_core::commands::SetPorts>,
        mut observed: ResMut<VesselControlObserved>,
    ) {
        let event = trigger.event();
        observed.0.push((event.target, event.writes.clone()));
    }

    /// Possession redirects the shared keyboard action state to the authored
    /// vessel binding. This is the complete desktop control seam: a physical
    /// `KeyW` updates the producer's input state, `ControlLink` selects the
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
        app.insert_resource(lunco_core_session::NetworkRole::Host)
            .init_resource::<lunco_core_runtime::SimTick>()
            .init_resource::<lunco_core_session::OwnedInputLog>()
            .init_resource::<VesselControlObserved>()
            .add_observer(observe_vessel_control)
            .add_systems(FixedUpdate, drive_from_bindings);

        let vessel = app
            .world_mut()
            .spawn((
                lunco_core::GlobalEntityId::from_raw(0xCAFE),
                lunco_port_core::InputPorts::new(&["throttle", "steer", "brake"]),
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
                ControlLink { target: vessel },
                ActionState::<UserIntent>::default(),
                test_bindings().input_map().unwrap(),
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
        assert!(
            app.world()
                .entity(avatar)
                .get::<ActionState<UserIntent>>()
                .expect("avatar action state")
                .pressed(&UserIntent::MoveForward)
        );
    }

    #[derive(Resource, Default)]
    struct ControlTargetOrder(Vec<u64>);

    fn record_control_target_order(
        trigger: On<lunco_cosim_core::commands::SetPorts>,
        identities: Query<&GlobalEntityId>,
        mut order: ResMut<ControlTargetOrder>,
    ) {
        if let Ok(identity) = identities.get(trigger.event().target) {
            order.0.push(identity.get());
        }
    }

    #[test]
    fn fixed_step_control_commands_follow_target_global_identity_order() {
        use lunco_core_runtime::SimTick;
        use lunco_core_session::{NetworkRole, OwnedInputLog};

        let mut app = App::new();
        app.insert_resource(NetworkRole::Host)
            .init_resource::<SimTick>()
            .init_resource::<OwnedInputLog>()
            .init_resource::<ControlTargetOrder>()
            .add_observer(record_control_target_order)
            .add_systems(FixedUpdate, drive_from_bindings);

        // Spawn in descending identity order so ECS query order cannot accidentally
        // make this assertion pass.
        for gid in [200, 100] {
            let target = app
                .world_mut()
                .spawn((
                    GlobalEntityId::from_raw(gid),
                    ControlBinding {
                        binds: vec![(UserIntent::MoveForward, "throttle".into(), 1.0)],
                    },
                ))
                .id();
            let mut intents = ActionState::<UserIntent>::default();
            intents.press(&UserIntent::MoveForward);
            app.world_mut().spawn((ControlLink { target }, intents));
        }

        app.world_mut().run_schedule(FixedUpdate);

        assert_eq!(
            app.world().resource::<ControlTargetOrder>().0,
            vec![100, 200],
            "control effects entering the fixed-step command path must have stable target order"
        );
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
                test_bindings().input_map().unwrap(),
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
            lunco_port_core::InputPorts::new(&["forward", "up", "speed_boost"]),
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
        q: Query<&lunco_port_core::InputPorts>,
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
            lunco_port_core::InputPorts::new(&["external_throttle", "pitch", "roll", "yaw"]),
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

        // Pause through the REAL path: the transport, which `project_time_transport`
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
            |trigger: On<lunco_cosim_core::commands::SetPorts>, mut w: ResMut<Writes>| {
                w.0.extend(trigger.event().writes.iter().cloned());
            },
        );

        // A free producer: its own input + its own binding, no `ControlLink`.
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
