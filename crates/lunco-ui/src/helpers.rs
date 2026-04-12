//! Helpers — CommandBuilder for fluent CommandMessage construction.
//!
//! All UI interactions should flow through CommandMessage events.
//! This makes the UI AI-native: AI observes/emits the same command stream.

use bevy::prelude::*;
use lunco_core::architecture::CommandMessage;
use smallvec::SmallVec;

/// Monotonically increasing counter for unique command IDs.
/// Each call to `CommandBuilder::build()` increments this counter.
static NEXT_COMMAND_ID: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(1);

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

    /// Build the command with a unique monotonically increasing ID.
    pub fn build(self) -> CommandMessage {
        let id = NEXT_COMMAND_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        CommandMessage {
            id,
            target: self.target,
            name: self.name,
            args: self.args,
            source: self.source,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_builder_unique_ids() {
        let cmd1 = CommandBuilder::new("FOCUS").build();
        let cmd2 = CommandBuilder::new("FOCUS").build();
        let cmd3 = CommandBuilder::new("RELEASE").build();
        assert_ne!(cmd1.id, cmd2.id);
        assert_ne!(cmd2.id, cmd3.id);
        assert_eq!(cmd1.name, cmd2.name); // same name, different IDs
    }

    #[test]
    fn test_command_builder_args() {
        let cmd = CommandBuilder::new("DRIVE")
            .arg(1.0)
            .arg(0.5)
            .build();
        assert_eq!(cmd.args.len(), 2);
        assert!((cmd.args[0] - 1.0).abs() < 1e-10);
        assert!((cmd.args[1] - 0.5).abs() < 1e-10);
    }
}
