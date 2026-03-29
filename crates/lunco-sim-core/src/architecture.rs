use bevy::prelude::*;

/// Level 2: Digital Port (OBC Emulation)
/// Uses i16 (-32768 to 32767) to emulate hardware bit-depth
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DigitalPort {
    pub raw_value: i16,
}

/// Level 1: Physical Port (Plant Actuators/Sensors)
/// Uses f32 for physical units (Nm, rad/s)
#[derive(Component, Debug, Clone, Copy, PartialEq, Default)]
pub struct PhysicalPort {
    pub value: f32,
}

/// Link between Digital and Physical domains
#[derive(Component, Debug, Clone, Copy)]
pub struct Wire {
    pub source: Entity,
    pub target: Entity,
    /// Signal gain / scaling factor
    pub scale: f32,
}

/// Level 3-5: The universal "Instruction" packet 
#[derive(Event, Debug, Clone)]
pub struct CommandMessage {
    pub target: Entity,
    pub name: String,
    pub args: Vec<f32>,
    pub source: Entity,
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
            source: Entity::from_raw(0),
            target: Entity::from_raw(1),
            scale: 2.5,
        };

        assert_eq!(wire.scale, 2.5);
    }
}
