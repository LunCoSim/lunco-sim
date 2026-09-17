//! Schedule contract shared by spatial scene projection and handoff systems.

use bevy::prelude::*;

/// Update-schedule boundary for the spatial handoff from USD projection.
///
/// USD projection publishes authored avatar and site components first; the
/// spatial adapter then captures the authored pose and commits the BigSpace
/// migration as one ordered transaction.
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone)]
pub struct SceneSpatialHandoffSet;
