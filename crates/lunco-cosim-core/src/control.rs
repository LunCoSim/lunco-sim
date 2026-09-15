//! Backend-neutral control relationships.

use bevy::prelude::*;

/// Connect a local control producer to the entity whose control surface it drives.
///
/// The component is attached to the producer (for example, a local avatar or an
/// AI controller) and names the target entity. It is deliberately independent of
/// avatars, vessels, input devices, and session ownership: those are higher-level
/// policy and runtime concerns. A controller runtime may use this relationship to
/// project semantic intents onto the target's authored [`lunco_core::ControlBinding`].
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct ControlLink {
    /// Entity whose input/control surface receives this producer's commands.
    pub target: Entity,
}
