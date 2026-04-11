//! Helpers — CommandBuilder for fluent CommandMessage construction.
//!
//! All UI interactions should flow through CommandMessage events.
//! This makes the UI AI-native: AI observes/emits the same command stream.

use bevy::prelude::*;
use lunco_core::architecture::CommandMessage;
use smallvec::SmallVec;

/// Fluent builder for constructing CommandMessage events.
///
/// # Example
/// ```ignore
/// ctx.trigger(
///     CommandBuilder::new("FOCUS")
///         .target(entity)
///         .source(camera_entity)
///         .build()
/// );
/// ```
pub struct CommandBuilder {
    name: String,
    target: Entity,
    source: Entity,
    args: SmallVec<[f64; 4]>,
}

impl CommandBuilder {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            target: Entity::PLACEHOLDER,
            source: Entity::PLACEHOLDER,
            args: SmallVec::new(),
        }
    }

    pub fn target(mut self, entity: Entity) -> Self {
        self.target = entity;
        self
    }

    pub fn source(mut self, entity: Entity) -> Self {
        self.source = entity;
        self
    }

    pub fn arg(mut self, value: f64) -> Self {
        self.args.push(value);
        self
    }

    pub fn build(self) -> CommandMessage {
        CommandMessage {
            id: 0, // TODO: use monotonically increasing counter
            target: self.target,
            name: self.name,
            args: self.args,
            source: self.source,
        }
    }
}
