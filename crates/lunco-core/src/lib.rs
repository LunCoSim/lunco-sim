pub mod architecture;
pub mod mocks;

pub use architecture::*;
pub use mocks::*;

use bevy::prelude::*;

pub struct LunCoCorePlugin;

#[derive(Component)]
pub struct Vessel;
 
#[derive(Component)]
pub struct RoverVessel;

#[derive(Component)]
pub struct Avatar;

#[derive(Component)]
pub struct OrbitState {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub vertical_offset: f32,
}

impl Default for OrbitState {
    fn default() -> Self {
        Self {
            yaw: 0.0,
            pitch: -0.5,
            distance: 10.0,
            vertical_offset: 1.0,
        }
    }
}

#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct TimeWarpState {
    pub speed: f64,
    pub physics_enabled: bool,
}

impl Plugin for LunCoCorePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, wire_system);
    }
}

fn wire_system(
    q_wires: Query<&Wire>,
    q_digital: Query<&DigitalPort>,
    mut q_physical: Query<&mut PhysicalPort>,
) {
    for wire in q_wires.iter() {
        if let Ok(digital) = q_digital.get(wire.source) {
            if let Ok(mut physical) = q_physical.get_mut(wire.target) {
                // Normalize i16 (-32768..32767) to -1.0..1.0 approximately, then apply scale
                physical.value = (digital.raw_value as f32 / 32767.0) * wire.scale;
            }
        }
    }
}
