//! # Simulation Control & Communication Fabric
//!
//! This module defines the "Nervous System" of the LunCoSim architecture.
//! It implements a multi-tier hierarchy that separates high-level user
//! intent from low-level physical actuation.
//!
//! ## The "Why": Fidelity-Driven Emulation
//! To support Flight Software (FSW) development, the simulation must
//! emulate the constraints of real hardware:
//! 1. **Digital Domain ([DigitalPort])**: Real On-Board Computers (OBCs)
//!    often communicate using discrete integer registers. We use `i16`
//!    to simulate bit-depth limits and signal quantization.
//! 2. **Physical Domain ([PhysicalPort])**: The "Plant" (physics engine)
//!    requires continuous values (`f32`) for forces and velocities.
//! 3. **The Bridge ([Wire])**: Acts as an emulated DAC/ADC, handles gains
//!    and signal conversions between the digital logic and physical reality.
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
        "backward" | "back" | "movebackward" | "pitch_up" => Some(UserIntent::MoveBackward),
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

// ── Digital / Physical Ports ──────────────────────────────────────────────────

/// Level 2: Digital Port (OBC Register Emulation)
///
/// **Why**: Uses `i16` (-32768 to 32767) to emulate hardware bit-depth and
/// the data-saturated environments typical of 16-bit flight computers.
/// It forces the developer to handle quantization and range limits.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Default, Reflect)]
#[reflect(Component)]
pub struct DigitalPort {
    /// Raw integer representation of the signal.
    pub raw_value: i16,
}

/// Level 1: Physical Port (Plant Actuators/Sensors)
///
/// **Why**: Uses `f32` for physical units (Nm, rad/s) representing the "real-world"
/// state. This is the value actually consumed by physics solvers.
#[derive(Component, Debug, Clone, Copy, PartialEq, Default, Reflect)]
#[reflect(Component)]
pub struct PhysicalPort {
    /// The physical value being applied or sensed.
    pub value: f32,
}

/// Link between Digital and Physical domains.
///
/// **Why**: Bridges the gap between Flight Software (Digital) and the
/// Simulation Engine (Physical), acting as a virtual cable with gain.
#[derive(Component, Debug, Clone, Copy, Reflect)]
#[reflect(Component)]
pub struct Wire {
    /// The digital port source.
    pub source: Entity,
    /// The physical port target.
    pub target: Entity,
    /// Signal gain / scaling factor to convert `i16` to `f32` physical units.
    pub scale: f32,
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
        let physical = PhysicalPort::default();
        let digital = DigitalPort::default();

        assert_eq!(physical.value, 0.0, "Physical port should initialize to zero precision float");
        assert_eq!(digital.raw_value, 0, "Digital port should initialize to zero bitwise integer");
    }

    #[test]
    fn test_wire_scale_assignment() {
        let wire = Wire {
            source: Entity::PLACEHOLDER,
            target: Entity::PLACEHOLDER,
            scale: 2.5,
        };

        assert_eq!(wire.scale, 2.5);
    }
}
