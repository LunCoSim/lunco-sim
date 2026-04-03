pub mod assembler;
pub mod rover;

use bevy::prelude::*;

pub struct LunCoRoboticsPlugin;

impl Plugin for LunCoRoboticsPlugin {
    fn build(&self, _app: &mut App) {
        // High-level assembly logic usually doesn't need per-frame systems,
        // it just provides spawning helpers.
    }
}
