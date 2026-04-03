//! Defines the communication and control architecture for simulation entities.
//!
//! This module implements a multi-level control hierarchy:
//! 1. **User Intents**: Semantic high-level actions (e.g., "Move Forward").
//! 2. **Commands**: Structured packets for inter-entity communication.
//! 3. **Digital/Physical Ports**: Hardware-level emulation using discrete (i16) 
//!    and continuous (f32) signal domains.

use bevy::prelude::*;
use smallvec::SmallVec;
use leafwing_input_manager::prelude::*;

/// High-level semantic actions intended by the user.
///
/// These actions are mapped from raw input (keyboard, controller) to 
/// abstract simulation intents that can be consumed by various subsystems.
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
    Look, 
    /// Camera focal length or distance adjustment.
    Zoom,
    
    /// Context-sensitive primary interaction.
    Action,
    /// Toggles between different control or view modes.
    SwitchMode,
    /// Pauses or unpauses the simulation state.
    Pause,
}

/// Alias for the leafwing ActionState using our UserIntent enum.
pub type IntentState = ActionState<UserIntent>;

/// A component that stores the current analog values of intents.
///
/// Used to capture the magnitude of raw inputs (e.g., joystick deflection)
/// for systems that require more than binary active/inactive states.
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

/// Level 2: Digital Port (OBC Emulation)
///
/// Uses i16 (-32768 to 32767) to emulate hardware bit-depth and 
/// data-saturated environments typical of flight software.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Default, Reflect)]
#[reflect(Component)]
pub struct DigitalPort {
    /// Raw integer representation of the signal.
    pub raw_value: i16,
}

/// Level 1: Physical Port (Plant Actuators/Sensors)
///
/// Uses f32 for physical units (Nm, rad/s) representing the real-world
/// state of a component after being driven by digital logic.
#[derive(Component, Debug, Clone, Copy, PartialEq, Default, Reflect)]
#[reflect(Component)]
pub struct PhysicalPort {
    /// The physical value being applied or sensed.
    pub value: f32,
}

/// Link between Digital and Physical domains.
///
/// Bridges the gap between Flight Software (Digital) and the 
/// Simulation Engine (Physical).
#[derive(Component, Debug, Clone, Copy, Reflect)]
#[reflect(Component)]
pub struct Wire {
    /// The digital port source.
    pub source: Entity,
    /// The physical port target.
    pub target: Entity,
    /// Signal gain / scaling factor to convert i16 to f32 units.
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
/// Used by task sequencers and UI to track the lifecycle of operations
/// like waypoint navigation, arm deployment, or camera transitions.
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
/// CommandMessages are the primary way subsystems communicate high-level
/// requests. They use SmallVec for f64 arguments to avoid heap allocations
/// in high-frequency simulation loops.
#[derive(Event, Debug, Clone)]
pub struct CommandMessage {
    /// Unique command ID for tracking and telemetry correlation.
    pub id: u64,
    /// The entity intended to receive/process this command.
    pub target: Entity,
    /// Semantic name of the command (e.g., "DRIVE_ROVER").
    pub name: String,
    /// High-precision arguments. Inline 4 f64 values (32 bytes) for zero-allocation hotspots.
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

/// Feedback event for a previously sent CommandMessage.
///
/// Allows the sender to track completion or handle errors from the receiver.
#[derive(Event, Debug, Clone, Reflect)]
pub struct CommandResponse {
    /// Links back to the original CommandMessage::id.
    pub command_id: u64,
    /// Current status of the requested operation.
    pub status: CommandStatus,
}

/// Allows components to describe their capabilities for AI/MCP discovery.
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
