//! Schedule contracts shared by avatar systems and scene projection.

use bevy::prelude::*;

/// Update-schedule boundary for the camera handoff from USD projection.
///
/// USD projection publishes authored avatar and site components first; the
/// avatar subsystem then captures the authored pose and commits the BigSpace
/// migration as one ordered transaction.
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone)]
pub struct AvatarSceneHandoffSet;
