pub mod architecture;

pub use architecture::*;

use bevy::prelude::*;

pub struct LunCoSimCorePlugin;

impl Plugin for LunCoSimCorePlugin {
    fn build(&self, app: &mut App) {
        app.add_event::<CommandMessage>();
    }
}
