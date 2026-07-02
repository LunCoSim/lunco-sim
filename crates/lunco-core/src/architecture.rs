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
    /// Toggles between different control or view modes.
    SwitchMode,
    /// Pauses or unpauses the simulation state.
    Pause,
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
/// ([`ControlBinding::from_usd_spec`]) so a scene can name intents in plain
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
        "switchmode" | "switch_mode" => Some(UserIntent::SwitchMode),
        "pause" => Some(UserIntent::Pause),
        _ => None,
    }
}

/// Per-vessel **intent → port** binding: while a [`UserIntent`] is active it
/// contributes `scale` to the named input port. Multiple entries may share an
/// intent (e.g. `Action` arming both `manual` and `manual_throttle`) or a port
/// (e.g. `MoveForward`/`MoveBackward` summing into `throttle`).
///
/// This is the SECOND, per-vessel stage of control. The first (key → intent) is
/// the shared leafwing [`UserIntent`] input map; this component decides only what
/// each intent *actuates* on this vessel, so a rover and a lander share the
/// intent vocabulary while binding different ports. It's authorable from USD via
/// [`from_usd_spec`](ControlBinding::from_usd_spec) (`lunco:controlBindings`), and
/// the controller falls back to a topology default ([`rover_binding`] /
/// [`flight_binding`]) when a possessed vessel carries none. The consuming system
/// (`lunco_controller::drive_from_bindings`) reads it off the vessel via the
/// controller link.
#[derive(Component, Debug, Clone)]
pub struct ControlBinding {
    /// `(intent, port_name, scale)` — each active intent adds its scale to the
    /// port; contributions to one port are summed then clamped to [-1, 1].
    pub binds: Vec<(UserIntent, String, f64)>,
}

impl ControlBinding {
    /// Wheeled rover: forward/back → `throttle`, left/right → `steer`,
    /// `Action` (Space/F) → `brake`.
    pub fn rover_binding() -> ControlBinding {
        ControlBinding {
            binds: vec![
                (UserIntent::MoveForward, "throttle".into(), 1.0),
                (UserIntent::MoveBackward, "throttle".into(), -1.0),
                (UserIntent::MoveLeft, "steer".into(), -1.0),
                (UserIntent::MoveRight, "steer".into(), 1.0),
                (UserIntent::Action, "brake".into(), 1.0),
            ],
        }
    }

    /// Cosim-flown lander: forward/back → pitch, left/right → roll, up/down
    /// (E/Q) → yaw, `Action` (Space/F) arms manual mode AND fires full throttle
    /// (`manual` + `manual_throttle`, mirroring `Lander.mo`). Port names match the
    /// Modelica `SimComponent.inputs`.
    pub fn flight_binding() -> ControlBinding {
        ControlBinding {
            binds: vec![
                (UserIntent::MoveForward, "manual_pitch".into(), -1.0),
                (UserIntent::MoveBackward, "manual_pitch".into(), 1.0),
                (UserIntent::MoveLeft, "manual_roll".into(), 1.0),
                (UserIntent::MoveRight, "manual_roll".into(), -1.0),
                (UserIntent::MoveDown, "manual_yaw".into(), 1.0),
                (UserIntent::MoveUp, "manual_yaw".into(), -1.0),
                (UserIntent::Action, "manual".into(), 1.0),
                (UserIntent::Action, "manual_throttle".into(), 1.0),
            ],
        }
    }

    /// Parse a `lunco:controlBindings` USD attribute into a binding. The spec is
    /// a comma-separated list of `intent:port:scale` entries, e.g.
    /// `"forward:throttle:1, backward:throttle:-1, action:brake:1"`. Unparseable
    /// entries (unknown intent, missing port, bad scale) are skipped with a
    /// warning. Returns `None` when nothing valid parsed, so the caller can fall
    /// back to a topology default.
    pub fn from_usd_spec(spec: &str) -> Option<ControlBinding> {
        let mut binds = Vec::new();
        for entry in spec.split(',').map(str::trim).filter(|e| !e.is_empty()) {
            let mut parts = entry.split(':');
            let intent = parts.next().and_then(parse_user_intent);
            let port = parts.next().map(str::trim).filter(|p| !p.is_empty());
            let scale = parts.next().and_then(|s| s.trim().parse::<f64>().ok());
            match (intent, port, scale) {
                (Some(i), Some(p), Some(s)) => binds.push((i, p.to_string(), s)),
                _ => warn!("[ControlBinding] ignoring malformed entry '{entry}' (want intent:port:scale)"),
            }
        }
        (!binds.is_empty()).then_some(ControlBinding { binds })
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
