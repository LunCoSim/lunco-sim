use bevy::prelude::*;
use lunco_core::architecture::{DigitalPort, PhysicalPort, Wire};

/// Connects a DigitalPort to a PhysicalPort via a Wire with a specific scale.
pub fn connect_ports(
    commands: &mut Commands,
    parent: Entity,
    source: Entity,
    target: Entity,
    scale: f32,
) -> Entity {
    let wire = commands.spawn(Wire { source, target, scale }).id();
    commands.entity(parent).add_child(wire);
    wire
}

/// Spawns a DigitalPort as a child of the parent.
pub fn spawn_digital_port(commands: &mut Commands, parent: Entity, name: &str) -> Entity {
    let port = commands.spawn((Name::new(name.to_string()), DigitalPort::default())).id();
    commands.entity(parent).add_child(port);
    port
}

/// Spawns a PhysicalPort as a child of the parent.
pub fn spawn_physical_port(commands: &mut Commands, parent: Entity, name: &str) -> Entity {
    let port = commands.spawn((Name::new(name.to_string()), PhysicalPort::default())).id();
    commands.entity(parent).add_child(port);
    port
}
