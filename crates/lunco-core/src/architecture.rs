//! # Simulation Control & Communication Fabric
//!
//! This module defines the "Nervous System" of the LunCoSim architecture.
//! It implements a multi-tier hierarchy that separates high-level user
//! intent from low-level physical actuation.
//!
//! ## The "Why": Fidelity-Driven Emulation
//! Signals move between subsystems through **[Port]**: one `f64` value — a
//! command, an actuator setpoint, a sensor reading, or a value exchanged with a
//! Modelica co-simulation. A directed link between two ports is a
//! `lunco_cosim::SimConnection` (the SSP connection: element + named connector,
//! with factor and offset), which is where a unit conversion belongs when two
//! ports are authored in different units.
//!
//! ## Typed Commands
//!
//! All simulation commands are **typed structs** that derive `#[derive(Command)]`.
//! This replaces the old string-based `CommandMessage` system.
//!
//! ```ignore
//! #[derive(Command)]
//! pub struct DriveRover {
//!     pub chassis: Entity,
//!     pub forward: f64,
//!     pub steer: f64,
//! }
//! ```
//!
//! Domain crates define their own commands and register them with one line:
//! ```ignore
//! app.register_command::<DriveRover>(on_drive_rover);
//! ```
//!
//! The API layer discovers all registered commands via `AppTypeRegistry`
//! reflection — zero hardcoding.

use bevy::prelude::*;
use leafwing_input_manager::prelude::*;

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

    /// Camera look/orientation adjustment.
    #[actionlike(DualAxis)]
    Look,
    /// Camera focal length or distance adjustment.
    #[actionlike(Axis)]
    Zoom,

    /// Context-sensitive primary interaction.
    Action,
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
}

/// Alias for the leafwing ActionState using our [UserIntent] enum.
pub type IntentState = ActionState<UserIntent>;

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
    /// Screen-space or angular delta for rotation.
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

/// Parse a control-intent name (case-insensitive, `Move` prefix optional, with
/// vessel-friendly aliases) into a [`UserIntent`]. Used by USD authoring
/// ([`ControlBinding::from_intent_entries`]) so a scene can name intents in plain
/// words (`"forward"`, `"brake"`, `"yaw_left"`).
pub fn parse_user_intent(name: &str) -> Option<UserIntent> {
    match name.trim().to_ascii_lowercase().as_str() {
        "forward" | "moveforward" | "pitch_down" => Some(UserIntent::MoveForward),
        // NOTE: `"back"` is NOT an alias here — it belongs to `Cancel` below
        // (menu/unpossess "go back"). Listing it in both arms made this arm win
        // and silently drove the vessel backward on an unpossess binding.
        "backward" | "movebackward" | "pitch_up" => Some(UserIntent::MoveBackward),
        "left" | "moveleft" | "roll_left" => Some(UserIntent::MoveLeft),
        "right" | "moveright" | "roll_right" => Some(UserIntent::MoveRight),
        "up" | "moveup" | "yaw_right" => Some(UserIntent::MoveUp),
        "down" | "movedown" | "yaw_left" => Some(UserIntent::MoveDown),
        "action" | "brake" | "arm" | "fire" => Some(UserIntent::Action),
        "release" | "detach" | "eject" | "decouple" => Some(UserIntent::Release),
        "switchmode" | "switch_mode" => Some(UserIntent::SwitchMode),
        "pause" => Some(UserIntent::Pause),
        "cancel" | "back" | "unpossess" => Some(UserIntent::Cancel),
        _ => None,
    }
}

/// How the possession/follow camera treats a vessel's **attitude** — the
/// authored answer to "should the camera rotate with the body?". It is a
/// property of how the vehicle MOVES, so it is authored on the vessel's control
/// profile (its `Controls` scope, `uniform token lunco:cameraFollow`) right
/// beside the intent→port binding, and read into this component during USD
/// projection.
///
/// The distinction matters because "follow the heading" is right for a surface
/// vehicle — a stable up and a meaningful forward — but wrong for a 6-DOF flyer:
/// extracting a yaw-heading from a body that is pitching and rolling swings the
/// camera wildly (it chases the tumble). A flyer wants a STABLE external frame
/// it rotates INSIDE of (`Orbit`), or — for a pilot who wants the body frame —
/// the FULL orientation (`Chase`). Absent an authored value a vessel defaults to
/// `Heading`, the historical surface-vehicle behavior.
#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[reflect(Component)]
pub enum CameraFollow {
    /// Track the body's position; follow its YAW heading only, up = surface
    /// normal. Ground vehicles (rovers): the camera turns as the vehicle steers.
    #[default]
    Heading,
    /// Track the body's position with a STABLE world/gravity up; do NOT rotate
    /// with the body. A 6-DOF flyer (lander) tumbles inside a steady view.
    Orbit,
    /// Follow the body's FULL orientation (yaw+pitch+roll) — a cockpit/chase
    /// frame that rolls with the craft. Opt-in for pilots who want it.
    Chase,
}

/// Parse a `lunco:cameraFollow` token into a [`CameraFollow`]. Unknown/empty →
/// `None`, so the caller keeps the default (`Heading`).
pub fn parse_camera_follow(s: &str) -> Option<CameraFollow> {
    match s.trim().to_ascii_lowercase().as_str() {
        "heading" | "springarm" | "yaw" => Some(CameraFollow::Heading),
        "orbit" | "stable" | "external" => Some(CameraFollow::Orbit),
        "chase" | "cockpit" | "attitude" | "full" => Some(CameraFollow::Chase),
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
/// `lunco:port`+`lunco:scale`, built via
/// [`from_intent_entries`](ControlBinding::from_intent_entries)) — there is NO
/// hardcoded Rust default: a vessel is controllable iff it carries a `Controls`
/// scope. It is delivered as a child `references` arc to a shared profile in
/// `control_profiles.usda` (the same arc kind wheels use), so it composes through
/// a spawn/reference; a runtime-built entity becomes drivable by authoring that
/// one child prim. The consuming system (`lunco_controller::drive_from_bindings`)
/// reads it off the vessel via the controller link; a vessel without one is
/// simply not driven.
#[derive(Component, Debug, Clone)]
pub struct ControlBinding {
    /// `(intent, port_name, scale)` — each active intent adds its scale to the
    /// port; contributions to one port are summed then clamped to [-1, 1].
    pub binds: Vec<(UserIntent, String, f64)>,
}

impl ControlBinding {
    /// Build from `(intent_name, port, scale)` triples the USD reader collects by
    /// walking a vessel's `Controls` scope — each child prim's NAME is the intent
    /// (`parse_user_intent`), with `string lunco:port` + `double lunco:scale`.
    /// Unknown intents are skipped with a warning; returns `None` when nothing
    /// valid parsed, so the caller can fall back to a topology default.
    pub fn from_intent_entries(entries: &[(String, String, f64)]) -> Option<ControlBinding> {
        let mut binds = Vec::new();
        for (intent, port, scale) in entries {
            match parse_user_intent(intent) {
                Some(i) => binds.push((i, port.clone(), *scale)),
                None => warn!("[ControlBinding] unknown control intent '{intent}' (skipped)"),
            }
        }
        (!binds.is_empty()).then_some(ControlBinding { binds })
    }

    /// The distinct port names this binding targets — i.e. the vessel's declared
    /// command surface (from USD). A controllable seeds exactly these into its FSW
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

    /// Resolve active intents into summed, clamped port writes. Every port named
    /// by the binding is present (0.0 when its intents are idle) so a released
    /// input writes 0 and clears the setpoint. `active(intent)` is the sole input
    /// — shared by the keyboard path and any internal (rhai/mission/AI) driver.
    pub fn resolve(&self, active: impl Fn(UserIntent) -> bool) -> Vec<(String, f64)> {
        let mut values: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
        for (_intent, port, _s) in &self.binds {
            values.entry(port.clone()).or_insert(0.0);
        }
        for (intent, port, s) in &self.binds {
            if active(*intent) {
                *values.get_mut(port).unwrap() += *s;
            }
        }
        values
            .into_iter()
            .map(|(name, v)| (name, v.clamp(-1.0, 1.0)))
            .collect()
    }
}

// ── Ports ─────────────────────────────────────────────────────────────────────

/// A named signal value exchanged between subsystems.
///
/// One port type carries every signal in the simulation — commands from the
/// control surface, actuator setpoints consumed by the physics solvers, sensor
/// readings, and the values a Modelica co-simulation exchanges. Values are `f64`
/// in whatever unit the signal is authored in; a `lunco_cosim::SimConnection`
/// applies factor/offset when two ports are expressed in different units.
#[derive(Component, Debug, Clone, Copy, PartialEq, Default, Reflect)]
#[reflect(Component)]
pub struct Port {
    /// The signal value.
    pub value: f64,
}

// ── Control surface ───────────────────────────────────────────────────────────

/// A controllable's **command surface**: the logical input ports it accepts, with
/// their current commanded values.
///
/// The command *vocabulary is data* — the keys present here declare exactly which
/// command ports this vehicle accepts, so the port backend stays strict (an
/// undeclared name is rejected → still reported as a dangling wire). A rover seeds
/// `throttle`/`steer`/`brake`, an avatar `forward`/`side`/`up`, a lander
/// `throttle`/`pitch`/`roll`/`yaw`. The keys are seeded from the vessel's
/// [`ControlBinding`] (i.e. from its authored `Controls` scope), never from a Rust
/// literal.
///
/// Written through the shared port substrate (`SetPorts` → the command backend)
/// and consumed by the vehicle's actuator (`apply_drive_mix`, `apply_fly`, a
/// Modelica bridge, …).
///
/// NOTE: the command port named `"brake"` here is NOT the actuator port named
/// `"brake"` in [`ActuatorPorts`]. They carry different values — an analog command
/// in `[-1,1]` here, a discretized `1.0`/`0.0` gate there — and are deliberately
/// kept in two components so the two `"brake"`s can never be conflated.
#[derive(Component, Debug, Clone, Default)]
pub struct CommandInputs {
    /// Commanded value per accepted command-port name. Only seeded keys are
    /// writable; see the type docs.
    pub values: std::collections::HashMap<String, f64>,
    /// Derived brake state, cached from `values["brake"] > 0.5` by the actuator so
    /// the per-tick physics systems read a bool without a map lookup.
    pub brake_active: bool,
}

impl CommandInputs {
    /// Build with a seeded command vocabulary: the input-port names this vehicle
    /// accepts, each initialised to `0.0`. The seeded keys ARE the command surface.
    pub fn new(command_ports: &[&str]) -> Self {
        Self {
            values: command_ports.iter().map(|n| (n.to_string(), 0.0)).collect(),
            brake_active: false,
        }
    }

    /// Current value of command input `name` (`0.0` if this vehicle doesn't accept
    /// it). The read side of the command surface for actuators.
    #[inline]
    pub fn cmd(&self, name: &str) -> f64 {
        self.values.get(name).copied().unwrap_or(0.0)
    }
}

/// A vessel's index from **actuator** name to the [`Port`] entity carrying that
/// actuator's setpoint.
///
/// This is the hardware/output half of a vessel's control surface, and is a
/// different thing from [`CommandInputs`]: those are the logical commands a human
/// or script issues, these are the per-actuator registers a drive kernel allocates
/// them onto (`drive_left`, `drive_right`, `steering`, `brake`, plus whatever the
/// vessel declares as `outputs:` attributes).
///
/// The port entities are spawned as children of the vessel so the recursive
/// scene-clear reclaims them with it.
#[derive(Component, Debug, Clone, Default)]
pub struct ActuatorPorts {
    /// Maps actuator mnemonics (e.g. `"drive_left"`) to their `Port` entity.
    pub ports: std::collections::HashMap<String, Entity>,
}

impl ActuatorPorts {
    /// Build from a prebuilt actuator-name → `Port` entity index.
    pub fn new(ports: std::collections::HashMap<String, Entity>) -> Self {
        Self { ports }
    }

    /// The `Port` entity for actuator `name`, if this vessel has one.
    #[inline]
    pub fn get(&self, name: &str) -> Option<Entity> {
        self.ports.get(name).copied()
    }
}

// ── Action Status ─────────────────────────────────────────────────────────────

/// Status of a long-running simulation action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Reflect, Default)]
pub enum ActionStatus {
    /// The action is still in progress.
    #[default]
    Running,
    /// The action finished as planned.
    Completed,
    /// The action was interrupted by another task or user input.
    Preempted,
    /// The action encountered an error and stopped.
    Failed,
}

/// Component attached to entities currently performing a long-running action.
///
/// **Why**: Essential for task sequencers and UI to track non-instantaneous
/// operations like waypoint navigation to prevent task overlapping.
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component)]
pub struct ActiveAction {
    /// Unique identifier for the type of action.
    pub name: String,
    /// Current execution state.
    pub status: ActionStatus,
    /// Normalized progress value (0.0 to 1.0).
    pub progress: f32,
}

impl Default for ActiveAction {
    fn default() -> Self {
        Self {
            name: "Unknown".to_string(),
            status: ActionStatus::Running,
            progress: 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_port_defaults() {
        assert_eq!(
            Port::default().value,
            0.0,
            "A port should initialize to zero"
        );
    }

    /// `"back"` used to appear in BOTH the `MoveBackward` and the `Cancel` arm;
    /// the first arm won, so a scene binding `"back"` to unpossess drove the
    /// vessel backward instead. `"back"` belongs to `Cancel` only.
    #[test]
    fn back_parses_as_cancel_not_move_backward() {
        assert_eq!(parse_user_intent("back"), Some(UserIntent::Cancel));
        assert_eq!(parse_user_intent("Back"), Some(UserIntent::Cancel));
        assert_eq!(
            parse_user_intent("backward"),
            Some(UserIntent::MoveBackward)
        );
        assert_eq!(
            parse_user_intent("movebackward"),
            Some(UserIntent::MoveBackward)
        );
        assert_eq!(parse_user_intent("cancel"), Some(UserIntent::Cancel));
        assert_eq!(parse_user_intent("unpossess"), Some(UserIntent::Cancel));
    }
}
