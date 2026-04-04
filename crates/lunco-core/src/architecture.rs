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

use bevy::prelude::*;
use smallvec::SmallVec;
use leafwing_input_manager::prelude::*;

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

/// Level 3-5: The universal "Instruction" packet.
/// 
/// **Why**: [CommandMessage]s are the primary backbone for decentralized 
/// communication. 
/// 
/// **Performance Optimization**: Uses [SmallVec] with an inline buffer of 4 `f64` 
/// values. This ensures that 95% of commands (which typically pass coordinates 
/// or magnitudes) trigger **zero heap allocations**, significantly improving 
/// 60Hz+ simulation stability.
#[derive(Event, Debug, Clone)]
pub struct CommandMessage {
    /// Unique command ID for tracking and telemetry correlation.
    pub id: u64,
    /// The entity intended to receive/process this command.
    pub target: Entity,
    /// Semantic name of the command (e.g., "DRIVE_ROVER").
    pub name: String,
    /// High-precision arguments. Inline 4 `f64` values for hotspot performance.
    pub args: SmallVec<[f64; 4]>,
    /// The entity that originated the command.
    pub source: Entity,
}

/// Status of a command in the simulation lifecycle.
#[derive(Debug, Clone, PartialEq, Reflect)]
pub enum CommandStatus {
    /// Command received and accepted for processing.
    Ack,
    /// Command rejected (e.g., invalid parameters or state).
    Nack,
    /// Command is currently being executed.
    Processing,
    /// Command finished successfully.
    Completed,
    /// Command failed during execution with a reason.
    Failed(String),
}

/// Feedback event for a previously sent [CommandMessage].
///
/// Allows the asynchronous tracking of command completion, essential 
/// for flight scripts and complex autonomous behaviors.
#[derive(Event, Debug, Clone, Reflect)]
pub struct CommandResponse {
    /// Links back to the original [CommandMessage]::id.
    pub command_id: u64,
    /// Current status of the requested operation.
    pub status: CommandStatus,
}

/// Allows components to describe their capabilities for discovery systems.
pub trait CommandRegistry {
    /// Returns a list of semantic command names this component can handle.
    fn discover_commands(&self) -> Vec<&'static str>;
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
