pub mod architecture;
pub mod mocks;

pub use architecture::*;
pub use mocks::*;

use bevy::prelude::*;

pub struct LunCoSimCorePlugin;

impl Plugin for LunCoSimCorePlugin {
    fn build(&self, _app: &mut App) {
    }
}
