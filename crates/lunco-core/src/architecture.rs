use bevy::prelude::*;
use smallvec::SmallVec;

/// Level 2: Digital Port (OBC Emulation)
/// Uses i16 (-32768 to 32767) to emulate hardware bit-depth
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Default, Reflect)]
#[reflect(Component)]
pub struct DigitalPort {
    pub raw_value: i16,
}

/// Level 1: Physical Port (Plant Actuators/Sensors)
/// Uses f32 for physical units (Nm, rad/s)
#[derive(Component, Debug, Clone, Copy, PartialEq, Default, Reflect)]
#[reflect(Component)]
pub struct PhysicalPort {
    pub value: f32,
}

/// Link between Digital and Physical domains
#[derive(Component, Debug, Clone, Copy, Reflect)]
#[reflect(Component)]
pub struct Wire {
    pub source: Entity,
    pub target: Entity,
    /// Signal gain / scaling factor
    pub scale: f32,
}

/// Status of a long-running simulation action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Reflect, Default)]
pub enum ActionStatus {
    #[default]
    Running,
    Completed,
    Preempted,
    Failed,
}

/// Component attached to entities currently performing a long-running action.
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component)]
pub struct ActiveAction {
    pub name: String,
    pub status: ActionStatus,
    pub progress: f32, // 0.0 to 1.0
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

/// Level 3-5: The universal "Instruction" packet 
#[derive(Event, Debug, Clone)]
pub struct CommandMessage {
    /// Unique command ID for tracking and telemetry correlation
    pub id: u64,
    /// The entity intended to receive/process this command
    pub target: Entity,
    /// Semantic name of the command (e.g., "DRIVE_ROVER")
    pub name: String,
    /// High-precision arguments. Inline 4 f64 values (32 bytes) for zero-allocation hotspots.
    pub args: SmallVec<[f64; 4]>,
    /// The entity that originated the command
    pub source: Entity,
}

/// Status of a command in the simulation lifecycle
#[derive(Debug, Clone, PartialEq, Reflect)]
pub enum CommandStatus {
    /// Command received and accepted for processing
    Ack,
    /// Command rejected (e.g., invalid parameters or state)
    Nack,
    /// Command is currently being executed
    Processing,
    /// Command finished successfully
    Completed,
    /// Command failed during execution
    Failed(String),
}

/// Feedback event for a previously sent CommandMessage
#[derive(Event, Debug, Clone, Reflect)]
pub struct CommandResponse {
    pub command_id: u64,
    pub status: CommandStatus,
}

/// Allows components to describe their capabilities for AI/MCP discovery
pub trait CommandRegistry {
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
