//! Typed scene-transition intents and lifecycle edges.
//!
//! This module is the dependency-light contract between scene consumers and
//! the USD scene owner. Consumers request a transition without knowing which
//! command handler mounts the stage; the owner publishes lifecycle edges from
//! the same command boundary that performs teardown and mounting.

use bevy::prelude::*;

/// The complete identity of a scene transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SceneTransition {
    /// Mount a stage at the requested root. An empty root means the stage's
    /// authored default root, resolved by the USD owner.
    Load { path: String, root_prim: String },
    /// Remove the active scene and leave the viewport empty.
    Clear,
    /// Re-read the currently mounted stage from its authoritative source.
    Restart {
        path: String,
        root_prim: String,
        reset_document: bool,
    },
}

impl SceneTransition {
    /// Construct a stage-load intent.
    pub fn load(path: impl Into<String>, root_prim: impl Into<String>) -> Self {
        Self::Load {
            path: path.into(),
            root_prim: root_prim.into(),
        }
    }

    /// Construct a clear intent.
    pub const fn clear() -> Self {
        Self::Clear
    }
}

/// A typed request for the authoritative scene owner to perform a transition.
///
/// This is deliberately separate from the public API command envelope. A
/// tutorial or another in-process domain can request a scene without encoding
/// a command name and JSON parameters, while the USD command owner remains the
/// only code that resolves paths and performs the transition.
#[derive(Event, Debug, Clone, PartialEq, Eq)]
pub struct SceneTransitionIntent {
    pub transition: SceneTransition,
}

impl SceneTransitionIntent {
    pub fn load(path: impl Into<String>, root_prim: impl Into<String>) -> Self {
        Self {
            transition: SceneTransition::load(path, root_prim),
        }
    }

    pub const fn clear() -> Self {
        Self {
            transition: SceneTransition::Clear,
        }
    }
}

/// Published immediately before an accepted scene transition tears down the
/// outgoing scene. All consumers use this edge to wind down their own state.
#[derive(Event, Debug, Clone, PartialEq, Eq)]
pub struct SceneTransitionStarted {
    pub transition: SceneTransition,
}

/// Published after a transition has reached its authoritative completion edge.
#[derive(Event, Debug, Clone, PartialEq, Eq)]
pub struct SceneTransitionCompleted {
    pub transition: SceneTransition,
}

/// Published when a requested stage cannot reach its completion edge.
#[derive(Event, Debug, Clone, PartialEq, Eq)]
pub struct SceneTransitionFailed {
    pub transition: SceneTransition,
    pub error: String,
}
