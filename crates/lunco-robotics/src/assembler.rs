// Ports are ECS-structural children, not `GridAnchor`s. The atomic-migration
// contract enforced by `clippy::disallowed_methods` doesn't apply here.
#![allow(clippy::disallowed_methods)]

//! Utilities for programmatically assembling robotic systems.
//!
//! This module provides helper functions to spawn ports, forming the control
//! backbone of complex robotic entities. Links between ports are
//! `lunco_cosim::SimConnection` entities, authored in USD as attribute
//! connections.

use bevy::prelude::*;
use lunco_core::architecture::Port;

/// Spawns a named [`Port`] as a child of the parent.
pub fn spawn_port(commands: &mut Commands, parent: Entity, name: &str) -> Entity {
    let port = commands
        .spawn((Name::new(name.to_string()), Port::default()))
        .id();
    commands.entity(parent).add_child(port);
    port
}
