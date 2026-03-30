pub mod architecture;
pub mod mocks;

pub use architecture::*;
pub use mocks::*;

use bevy::prelude::*;

pub struct LunCoSimCorePlugin;

#[derive(Component)]
pub struct Vessel;

#[derive(Component)]
pub struct RoverVessel;

impl Plugin for LunCoSimCorePlugin {
    fn build(&self, _app: &mut App) {
    }
}
