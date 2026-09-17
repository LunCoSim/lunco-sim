//! Authored opt-in markers for generic celestial placement systems.

use bevy::prelude::{Component, Reflect, ReflectComponent};

/// Track a scene-local entity in the solar frame.
///
/// Entities with a geodetic anchor, orbit, or libration anchor are tracked
/// automatically. This marker is for local children such as a rover antenna
/// or an opaque link blocker whose placement follows its scene hierarchy.
#[derive(Component, Debug, Clone, Copy, Default, Reflect)]
#[reflect(Component)]
pub struct SolarTracked;
