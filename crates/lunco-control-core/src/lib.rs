//! Generic semantic-control contracts shared by input producers, authored control
//! bindings, and engine consumers.
//!
//! This package owns the semantic input boundary that used to be mixed into
//! `lunco-core`: the leafwing action vocabulary, authored intent-to-port
//! bindings, egui input gate, and bounded semantic-edge trace. The generic
//! engine/port substrate remains in `lunco-core`, so changes to control policy
//! do not rebuild that high-fanout crate.

use bevy::prelude::*;
use leafwing_input_manager::prelude::*;
use lunco_core::{Avatar, GlobalEntityId, LocalAvatar};
use std::collections::VecDeque;

// ── User Intent (Input Abstraction) ───────────────────────────────────────────

/// High-level semantic actions intended by the user.
///
/// These actions are mapped from raw input (keyboard, controller) to
/// abstract simulation intents. This allows the simulation logic to remain
/// agnostic of the input hardware.
#[derive(Actionlike, PartialEq, Eq, Hash, Clone, Copy, Debug, Reflect)]
pub enum UserIntent {
    /// Forward longitudinal movement.
    MoveForward,
    /// Backward longitudinal movement.
    MoveBackward,
    /// Lateral movement to the left.
    MoveLeft,
    /// Lateral movement to the right.
    MoveRight,
    /// Upward vertical movement.
    MoveUp,
    /// Downward vertical movement.
    MoveDown,

    /// Multiplies free-flight translation speed while held.
    ///
    /// This is a presentation-control intent, not a vessel port. It remains
    /// in the shared input vocabulary so the avatar reads the same configured
    /// binding as the help overlay and input simulation tools.
    SpeedBoost,

    /// Camera look/orientation adjustment.
    #[actionlike(DualAxis)]
    Look,
    /// Camera focal length or distance adjustment.
    #[actionlike(Axis)]
    Zoom,

    /// Context-sensitive primary interaction.
    Action,
    /// Normalized propulsion command for a powered vehicle.
    ///
    /// This is deliberately separate from [`UserIntent::Action`]: Action is
    /// the autopilot/editor shortcut, while a powered vehicle may consume
    /// Thrust as its engine command.
    Thrust,
    /// Vehicle-specific braking or hold command.
    ///
    /// This is deliberately separate from [`UserIntent::Action`] and
    /// [`UserIntent::Thrust`]: the former is the autopilot shortcut, while the
    /// latter is a powered-vehicle engine command.
    Brake,
    /// Release/detach a dock or coupling (e.g. a lander→rover fixed joint). Routed
    /// through the normal intent→port machinery to a `release` command port.
    Release,
    /// Toggles between different control or view modes.
    SwitchMode,
    /// Pauses or unpauses the simulation state.
    Pause,
    /// Cancel / back out: release possession or plain follow, back to free flight.
    /// A discrete key intent (default `Backspace`) — see `avatar_escape_possession`.
    /// While an egui field is focused egui consumes the key, so the guard suppresses
    /// this intent that frame and it acts only once the field is defocused.
    Cancel,
    /// Delete the current editor selection. This is an editor intent so the
    /// shortcut is rebindable and panels do not inspect raw keyboard state.
    DeleteSelection,
}

impl std::fmt::Display for UserIntent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Self::MoveForward => "Move forward",
            Self::MoveBackward => "Move backward",
            Self::MoveLeft => "Move left",
            Self::MoveRight => "Move right",
            Self::MoveUp => "Move up",
            Self::MoveDown => "Move down",
            Self::SpeedBoost => "Speed boost",
            Self::Look => "Look",
            Self::Zoom => "Zoom",
            Self::Action => "Primary action",
            Self::Thrust => "Vehicle thrust",
            Self::Brake => "Vehicle brake",
            Self::Release => "Release coupling",
            Self::SwitchMode => "Switch camera mode",
            Self::Pause => "Pause simulation",
            Self::Cancel => "Cancel current tool",
            Self::DeleteSelection => "Delete selection",
        };
        f.write_str(label)
    }
}

impl UserIntent {
    /// Canonical lower-case name used by authored bindings and event payloads.
    pub const fn canonical_name(self) -> &'static str {
        match self {
            Self::MoveForward => "forward",
            Self::MoveBackward => "backward",
            Self::MoveLeft => "left",
            Self::MoveRight => "right",
            Self::MoveUp => "yaw_right",
            Self::MoveDown => "yaw_left",
            Self::SpeedBoost => "speed_boost",
            Self::Look => "look",
            Self::Zoom => "zoom",
            Self::Action => "action",
            Self::Thrust => "thrust",
            Self::Brake => "brake",
            Self::Release => "release",
            Self::SwitchMode => "switch_mode",
            Self::Pause => "pause",
            Self::Cancel => "cancel",
            Self::DeleteSelection => "delete_selection",
        }
    }
}

/// Alias for the leafwing ActionState using our [UserIntent] enum.
pub type IntentState = ActionState<UserIntent>;

/// The discrete transition of a target-scoped semantic intent.
///
/// A pressed/released edge is distinct from a held port value: it is delivered
/// once and does not latch an actuator. `Pulse` is one atomic one-shot event;
/// the consuming Twin or domain decides whether that means a latch, release,
/// toggle, or another policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Reflect)]
pub enum SemanticIntentEdgeKind {
    /// The semantic intent became active.
    Pressed,
    /// The semantic intent became inactive.
    Released,
    /// A one-shot semantic action with no preceding held state required.
    Pulse,
}

impl SemanticIntentEdgeKind {
    /// Canonical wire/script spelling for this edge kind.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pressed => "pressed",
            Self::Released => "released",
            Self::Pulse => "pulse",
        }
    }
}

impl std::fmt::Display for SemanticIntentEdgeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A target-scoped semantic edge emitted by the controller contract.
///
/// This event carries intent identity and target identity only. It does not
/// choose a vehicle port or mutate a domain; authored policy consumes it and
/// may issue the existing `SetPorts` command if that is the intended effect.
#[derive(Event, Clone, Copy, Debug, PartialEq, Eq, Reflect)]
pub struct SemanticIntentEdge {
    /// The entity whose authored semantic control surface receives the edge.
    pub target: Entity,
    /// The shared semantic intent that changed or was pulsed.
    pub intent: UserIntent,
    /// The discrete transition delivered to the target.
    pub kind: SemanticIntentEdgeKind,
    /// The command/operation id that identifies this edge for read-only causal
    /// inspection. Physical input edges mint an id locally; API/Rhai dispatch
    /// reuses the active command id.
    pub correlation_id: u64,
}

/// One bounded semantic edge retained for causal inspection.
///
/// This is deliberately only the immutable input-side record. Downstream
/// observations (port owner, connection, admission, and measurements) belong
/// to their existing owners and are composed by the `CausalTrace` API query;
/// keeping them out of this ledger avoids a second routing or telemetry store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CausalTraceRecord {
    /// The command/operation id that identifies this action.
    pub correlation_id: u64,
    /// The session-local entity that received the edge.
    pub target: Entity,
    /// Stable target identity captured when the edge was emitted.
    pub target_gid: Option<GlobalEntityId>,
    /// The shared semantic intent.
    pub intent: UserIntent,
    /// The delivered edge kind.
    pub kind: SemanticIntentEdgeKind,
}

/// Bounded, scene-scoped semantic edge ledger used by the causal trace query.
///
/// The ledger is not a second command path and does not retain per-frame
/// control traffic. It keeps only the most recent discrete actions so an
/// operator can correlate one semantic edge with the live topology and
/// measurements exposed by the owning domains.
#[derive(Resource, Debug, Default)]
pub struct CausalTrace {
    records: VecDeque<CausalTraceRecord>,
}

impl CausalTrace {
    /// Maximum number of discrete semantic edges retained for inspection.
    pub const MAX_RECORDS: usize = 256;

    /// Retain one semantic edge, evicting the oldest record at the documented
    /// bound. Zero is accepted for direct in-process event producers.
    pub fn record(&mut self, edge: &SemanticIntentEdge, target_gid: Option<GlobalEntityId>) {
        self.records.push_back(CausalTraceRecord {
            correlation_id: edge.correlation_id,
            target: edge.target,
            target_gid,
            intent: edge.intent,
            kind: edge.kind,
        });
        while self.records.len() > Self::MAX_RECORDS {
            self.records.pop_front();
        }
    }

    /// Find one action for a stable target and correlation id.
    pub fn find(
        &self,
        target_gid: GlobalEntityId,
        correlation_id: u64,
    ) -> Option<&CausalTraceRecord> {
        self.records.iter().rev().find(|record| {
            record.target_gid == Some(target_gid) && record.correlation_id == correlation_id
        })
    }

    /// Return the newest recorded action for a stable target.
    pub fn latest_for_target(&self, target_gid: GlobalEntityId) -> Option<&CausalTraceRecord> {
        self.records
            .iter()
            .rev()
            .find(|record| record.target_gid == Some(target_gid))
    }

    /// Number of retained semantic edge records.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether no semantic edge records are retained.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Remove all scene-scoped records at the scene teardown boundary.
    pub fn clear(&mut self) {
        self.records.clear();
    }
}

/// A component that stores the current high-resolution analog values of user intents.
///
/// **Why**: While [UserIntent] tracks 'binary' state for mapping, complex
/// systems (like throttle control or gimbal steering) require the raw
/// floating-point deflection of the input device.
#[derive(Component, EntityEvent, Debug, Clone, Reflect)]
#[reflect(Component, Default)]
pub struct IntentAnalogState {
    /// The entity this intent state belongs to.
    pub entity: Entity,
    /// Normalized forward/backward value (-1.0 to 1.0).
    pub forward: f32,
    /// Normalized left/right value (-1.0 to 1.0).
    pub side: f32,
    /// Normalized up/down value (-1.0 to 1.0).
    pub elevation: f32,
    /// Pointer look delta in **screen space** — never radians.
    ///
    /// `+x` is pointer-right, `+y` is pointer-down (the raw device convention),
    /// in device units scaled by the capture gain: the producer
    /// (`lunco_avatar::capture_avatar_intent`) writes
    /// `ActionState::axis_pair(Look) * 10.0`, i.e. mouse motion, not an angle.
    ///
    /// Consumers turn it into an angle themselves — the only one today is
    /// `lunco_avatar::avatar_behavior_input_system`, which applies
    /// `-look_delta * sensitivity * 0.01` to get yaw/pitch radians (note the sign
    /// flip: screen-down must become pitch-up). Steering does **not** read this
    /// field; vessel control flows through the port path
    /// (`ControlBinding` → `SetPorts`), so there is exactly one interpretation.
    ///
    /// Anything new that consumes it owns the same screen-space → radians
    /// conversion; do not write pre-converted angles here.
    pub look_delta: Vec2,
    /// Simulation time when this state was captured.
    pub timestamp: f64,
}

impl Default for IntentAnalogState {
    fn default() -> Self {
        Self {
            entity: Entity::PLACEHOLDER,
            forward: 0.0,
            side: 0.0,
            elevation: 0.0,
            look_delta: Vec2::ZERO,
            timestamp: 0.0,
        }
    }
}

/// Parse a canonical control-intent name (case-insensitive) into a
/// [`UserIntent`]. Used by USD authoring ([`ControlBinding::from_intent_entries`])
/// and the API; authored bindings use one vocabulary everywhere.
pub fn parse_user_intent(name: &str) -> Option<UserIntent> {
    match name.trim().to_ascii_lowercase().as_str() {
        "forward" => Some(UserIntent::MoveForward),
        "backward" => Some(UserIntent::MoveBackward),
        "left" => Some(UserIntent::MoveLeft),
        "right" => Some(UserIntent::MoveRight),
        "yaw_right" => Some(UserIntent::MoveUp),
        "yaw_left" => Some(UserIntent::MoveDown),
        "speed_boost" => Some(UserIntent::SpeedBoost),
        "action" => Some(UserIntent::Action),
        "thrust" => Some(UserIntent::Thrust),
        "brake" => Some(UserIntent::Brake),
        "release" => Some(UserIntent::Release),
        "switch_mode" => Some(UserIntent::SwitchMode),
        "pause" => Some(UserIntent::Pause),
        "cancel" => Some(UserIntent::Cancel),
        "delete_selection" => Some(UserIntent::DeleteSelection),
        _ => None,
    }
}

/// Per-vessel **intent → port** binding: while a [`UserIntent`] is active it
/// contributes `scale` to the named input port. Multiple entries may share an
/// intent, or a port (e.g. `MoveForward`/`MoveBackward` summing into `throttle`
/// with +1/-1).
///
/// This is the SECOND, per-vessel stage of control. The first (key → intent) is
/// the shared leafwing [`UserIntent`] input map; this component decides only what
/// each intent *actuates* on this vessel, so a rover and a lander share the
/// intent vocabulary while binding different ports. It is authored purely from
/// USD as a `Controls` child scope (intent-named `def` prims with
/// `lunco:port`+`lunco:factor`, built via
/// [`from_intent_entries`](ControlBinding::from_intent_entries)) — there is NO
/// hardcoded Rust default. It is delivered as a child `references` arc to a
/// shared profile in `control_profiles.usda` (the same arc kind wheels use), so
/// it composes through a spawn/reference. The consuming system
/// (`lunco_controller::drive_from_bindings`) reads it off the controlled endpoint
/// via the controller link.
///
/// This is an **adapter**, not the input surface or authority predicate. An
/// endpoint exposes and accepts commands through [`InputPorts`]; it can be
/// remotely controlled with `SetPorts` without an avatar-keyboard binding.
/// Adding a `Controls` scope only makes shared `UserIntent`s (keyboard, gamepad,
/// simulated intent) translate into those exposed ports.
#[derive(Component, Debug, Clone)]
pub struct ControlBinding {
    /// `(intent, port_name, scale)` — each active intent adds its scale to the
    /// port; contributions to one port are summed then clamped to [-1, 1].
    pub binds: Vec<(UserIntent, String, f64)>,
}

impl ControlBinding {
    /// Build from `(intent_name, port, scale)` triples the USD reader collects by
    /// walking a vessel's `Controls` scope — each child prim's NAME is the intent
    /// (`parse_user_intent`), with `string lunco:port` + `double lunco:factor`.
    /// Unknown intents are skipped with a warning; returns `None` when nothing
    /// valid parsed, so the endpoint has no keyboard adapter. There is no
    /// topology fallback: an omitted or invalid authored binding remains
    /// unavailable rather than silently inventing a control surface.
    pub fn from_intent_entries(entries: &[(String, String, f64)]) -> Option<ControlBinding> {
        let mut binds = Vec::new();
        for (intent, port, scale) in entries {
            let Some(i) = parse_user_intent(intent) else {
                warn!("[ControlBinding] unknown control intent '{intent}' (skipped)");
                continue;
            };
            if port.trim().is_empty() {
                warn!("[ControlBinding] empty input port for intent '{intent}' (skipped)");
                continue;
            }
            if !scale.is_finite() {
                warn!(
                    "[ControlBinding] non-finite factor for intent '{intent}' and port '{port}' (skipped)"
                );
                continue;
            }
            binds.push((i, port.clone(), *scale));
        }
        (!binds.is_empty()).then_some(ControlBinding { binds })
    }

    /// The distinct port names this binding targets — i.e. the vessel's declared
    /// input surface (from USD). An endpoint seeds exactly these into its FSW
    /// `inputs` so the strict command backend accepts writes to them and no others.
    pub fn ports(&self) -> impl Iterator<Item = &str> {
        // `binds` is small (a handful of intents); a linear "seen" scan beats a
        // HashSet here and keeps the return borrow-clean.
        let mut seen: Vec<&str> = Vec::new();
        for (_i, port, _s) in &self.binds {
            if !seen.contains(&port.as_str()) {
                seen.push(port.as_str());
            }
        }
        seen.into_iter()
    }

    /// Whether this binding routes the given semantic intent to any port.
    /// Consumers use this to install intent-specific domain actuators only when
    /// the authored control profile actually exposes that intent.
    pub fn has_intent(&self, intent: UserIntent) -> bool {
        self.binds.iter().any(|(bound, _, _)| *bound == intent)
    }

    /// Resolve active intents into summed, clamped port writes. Every port named
    /// by the binding is present (0.0 when its intents are idle) so a released
    /// input writes 0 and clears the setpoint. `active(intent)` is the sole input
    /// — shared by the keyboard path and any internal (rhai/mission/AI) driver.
    pub fn resolve(&self, active: impl Fn(UserIntent) -> bool) -> Vec<(String, f64)> {
        // Keep the first authored occurrence order. HashMap iteration would make
        // the serialized SetPorts order vary between processes.
        let mut values: Vec<(String, f64)> = Vec::new();
        for (_intent, port, _s) in &self.binds {
            if !values.iter().any(|(name, _)| name == port) {
                values.push((port.clone(), 0.0));
            }
        }
        for (intent, port, s) in &self.binds {
            if active(*intent) {
                values
                    .iter_mut()
                    .find(|(name, _)| name == port)
                    .expect("resolve seeds every binding port")
                    .1 += *s;
            }
        }
        values
            .into_iter()
            .map(|(name, value)| (name, value.clamp(-1.0, 1.0)))
            .collect()
    }
}

/// Whether egui is currently consuming pointer / keyboard input.
///
/// egui is a second, immediate-mode input world layered on top of Bevy: it
/// reads its own copy of the winit events and never removes anything from
/// Bevy's `ButtonInput`. So a key pressed while an egui text field is focused
/// reaches BOTH egui and Bevy's `ButtonInput<KeyCode>` — and without this gate
/// it would also drive the avatar (typing `w`/`a`/`s`/`d` in the Inspector or a
/// REPL would move the vessel). Likewise a scroll/orbit over a panel would move
/// the camera.
///
/// This resource relays egui's `wants_keyboard_input()` / `wants_pointer_input()`
/// (from the primary egui context) into the ECS so scene-input systems can gate
/// on it without depending on `bevy_egui`. Populated once per frame by
/// `lunco-workbench` (the crate that owns the `PrimaryEguiContext`); a press in
/// the main scene explicitly surrenders stale editor focus before this resource
/// is published. On a headless server nothing writes it, so both flags stay
/// `false` and every gate is a no-op.
///
/// Discrete scene *picks* (click-to-select / click-to-spawn) do NOT need this —
/// they flow through `bevy_picking`, where egui occlusion is already handled by
/// the workbench's egui picking backend. This gate is for the *continuous / raw*
/// input systems: keyboard driving, camera orbit, scroll-zoom.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct EguiFocus {
    /// A focused egui widget (text field, drag-value, …) wants the keyboard.
    pub wants_keyboard: bool,
    /// The pointer is over an egui widget that wants pointer input.
    pub wants_pointer: bool,
}

/// Marks the app-level input surface that resolves the local user's semantic
/// intents when no avatar owns the keyboard. The workbench carries one such
/// surface so editor-only views can use the same rebindable intent vocabulary
/// as simulation control without inventing a raw-key path.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct LocalIntentSurface;

/// "The user asked to back out" — the [`UserIntent::Cancel`] intent.
///
/// Read this instead of sniffing `KeyCode::Escape`/`Backspace`: the bindings are DATA
/// (`assets/config/keybindings.json`), so a rebind works everywhere at once and every
/// mode agrees on what cancelling means. It reads both the local avatar and the
/// workbench's app-level intent surface, so editor-only previews do not require an
/// avatar. Suppressed while an egui field has keyboard focus, so Backspace typed into
/// a text box edits text rather than backing out.
#[derive(bevy::ecs::system::SystemParam)]
pub struct CancelIntent<'w, 's> {
    avatars: Query<'w, 's, &'static IntentState, (With<Avatar>, With<LocalAvatar>)>,
    global_surface: Query<'w, 's, &'static IntentState, With<LocalIntentSurface>>,
    egui_focus: Res<'w, EguiFocus>,
}

impl CancelIntent<'_, '_> {
    /// True on the frame the user pressed Cancel.
    pub fn just_pressed(&self) -> bool {
        if self.egui_focus.wants_keyboard {
            return false;
        }
        self.avatars
            .iter()
            .any(|i| i.just_pressed(&UserIntent::Cancel))
            || self
                .global_surface
                .iter()
                .any(|i| i.just_pressed(&UserIntent::Cancel))
    }
}

/// "Delete the current selection" — the rebindable
/// [`UserIntent::DeleteSelection`] editor intent.
///
/// Like [`CancelIntent`], it stands down while egui owns keyboard focus so a
/// focused text editor receives Delete normally.
#[derive(bevy::ecs::system::SystemParam)]
pub struct DeleteSelectionIntent<'w, 's> {
    avatars: Query<'w, 's, &'static IntentState, (With<Avatar>, With<LocalAvatar>)>,
    egui_focus: Res<'w, EguiFocus>,
}

impl DeleteSelectionIntent<'_, '_> {
    /// True on the frame the user requested deletion.
    pub fn just_pressed(&self) -> bool {
        !self.egui_focus.wants_keyboard
            && self
                .avatars
                .iter()
                .any(|intent| intent.just_pressed(&UserIntent::DeleteSelection))
    }
}

/// Install the semantic-control resources and the shared leafwing action state.
pub struct LunCoControlPlugin;

impl Plugin for LunCoControlPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(InputManagerPlugin::<UserIntent>::default())
            .init_resource::<EguiFocus>()
            .init_resource::<CausalTrace>()
            .register_type::<UserIntent>()
            .register_type::<SemanticIntentEdgeKind>()
            .register_type::<SemanticIntentEdge>()
            .register_type::<IntentAnalogState>();
    }
}

#[cfg(test)]
mod tests {
    use super::{ControlBinding, UserIntent, parse_user_intent};

    /// Intent parsing accepts exactly the authored control vocabulary and rejects
    /// former aliases instead of silently changing their meaning.
    #[test]
    fn parse_user_intent_accepts_only_canonical_names() {
        for (name, expected) in [
            ("forward", UserIntent::MoveForward),
            ("backward", UserIntent::MoveBackward),
            ("left", UserIntent::MoveLeft),
            ("right", UserIntent::MoveRight),
            ("yaw_right", UserIntent::MoveUp),
            ("yaw_left", UserIntent::MoveDown),
            ("speed_boost", UserIntent::SpeedBoost),
            ("action", UserIntent::Action),
            ("thrust", UserIntent::Thrust),
            ("brake", UserIntent::Brake),
            ("release", UserIntent::Release),
            ("switch_mode", UserIntent::SwitchMode),
            ("pause", UserIntent::Pause),
            ("cancel", UserIntent::Cancel),
            ("delete_selection", UserIntent::DeleteSelection),
        ] {
            assert_eq!(parse_user_intent(name), Some(expected), "{name}");
        }
        for old_spelling in [
            "moveforward",
            "movebackward",
            "moveleft",
            "moveright",
            "moveup",
            "movedown",
            "up",
            "down",
            "pitch_up",
            "pitch_down",
            "roll_left",
            "roll_right",
            "arm",
            "fire",
            "detach",
            "eject",
            "decouple",
            "switchmode",
            "back",
            "unpossess",
        ] {
            assert_eq!(parse_user_intent(old_spelling), None, "{old_spelling}");
        }
    }

    #[test]
    fn control_binding_reports_only_authored_intents() {
        let rover = ControlBinding::from_intent_entries(&[
            ("forward".into(), "throttle".into(), 1.0),
            ("backward".into(), "throttle".into(), -1.0),
            ("left".into(), "steer".into(), -1.0),
        ])
        .expect("rover profile has authored controls");
        assert!(!rover.has_intent(UserIntent::Release));
        assert!(rover.has_intent(UserIntent::MoveForward));

        let lander =
            ControlBinding::from_intent_entries(&[("release".into(), "release".into(), 1.0)])
                .expect("lander profile has an authored release control");
        assert!(lander.has_intent(UserIntent::Release));
    }

    #[test]
    fn control_binding_resolve_preserves_authored_port_order_and_sums() {
        let binding = ControlBinding::from_intent_entries(&[
            ("right".into(), "steer".into(), 0.25),
            ("forward".into(), "throttle".into(), 2.0),
            ("backward".into(), "throttle".into(), -1.0),
            ("left".into(), "steer".into(), 0.5),
        ])
        .expect("valid authored controls");

        assert_eq!(
            binding.resolve(|_| true),
            vec![("steer".into(), 0.75), ("throttle".into(), 1.0)]
        );
    }

    #[test]
    fn control_binding_rejects_malformed_authored_entries() {
        assert!(
            ControlBinding::from_intent_entries(&[
                ("forward".into(), " ".into(), 1.0),
                ("backward".into(), "throttle".into(), f64::NAN),
                ("not_an_intent".into(), "throttle".into(), 1.0),
            ])
            .is_none()
        );
    }
}
#[cfg(test)]
mod cancel_intent_tests {
    use super::{CancelIntent, EguiFocus, LocalIntentSurface, UserIntent};
    use bevy::prelude::*;
    use leafwing_input_manager::prelude::{ActionState, InputManagerPlugin, InputMap};

    #[derive(Resource, Default)]
    struct ObservedCancel(bool);

    fn observe_cancel(cancel: CancelIntent, mut observed: ResMut<ObservedCancel>) {
        observed.0 = cancel.just_pressed();
    }

    #[test]
    fn editor_cancel_works_without_a_local_avatar() {
        let mut app = App::new();
        app.add_plugins((
            bevy::time::TimePlugin,
            bevy::input::InputPlugin,
            InputManagerPlugin::<UserIntent>::default(),
        ))
        .init_resource::<EguiFocus>()
        .init_resource::<ObservedCancel>()
        .add_systems(Update, observe_cancel);

        let mut input_map = InputMap::default();
        input_map.insert(UserIntent::Cancel, KeyCode::Escape);
        app.world_mut().spawn((
            LocalIntentSurface,
            ActionState::<UserIntent>::default(),
            input_map,
        ));

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Escape);
        app.update();

        assert!(
            app.world().resource::<ObservedCancel>().0,
            "the shared cancel intent must read the app-level editor input surface"
        );
    }
}
