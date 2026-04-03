pub mod architecture;
pub mod mocks;
pub mod telemetry;
pub mod coords;
pub mod log;

pub use architecture::*;
pub use mocks::*;
pub use telemetry::*;
pub use log::*;

use bevy::prelude::*;

pub struct LunCoCorePlugin;

#[derive(Component)]
pub struct Avatar;

#[derive(Component, Reflect, Default)]
#[reflect(Component)]
pub struct Spacecraft {
    pub name: String,
    pub ephemeris_id: i32,
    pub reference_id: i32,
    pub start_epoch_jd: Option<f64>,
    pub end_epoch_jd: Option<f64>,
    pub hit_radius_m: f32,
    pub user_visible: bool,
}
 
#[derive(Component)]
pub struct Vessel;
 
#[derive(Component)]
pub struct RoverVessel;

/// Physical properties of a vessel or celestial body.
#[derive(Component, Debug, Clone, Reflect, Default)]
#[reflect(Component)]
pub struct PhysicalProperties {
    pub radius_m: f64,
    pub mass_kg: f64,
}

#[derive(Component, Debug, Clone, Reflect, Default)]
#[reflect(Component)]
pub struct CelestialBody {
    pub name: String,
    pub ephemeris_id: i32,
    pub radius_m: f64,
}
#[derive(Component, Reflect, Clone, Debug)]
#[reflect(Component)]
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

#[derive(Resource, Debug, Clone, Copy, Reflect)]
#[reflect(Resource)]
pub struct CelestialClock {
    pub epoch: f64,            // Julian Date (TDB)
    pub speed_multiplier: f64, // 1.0 = real-time
    pub paused: bool,
}

impl Default for CelestialClock {
    fn default() -> Self {
        Self {
            epoch: 2451545.0, // J2000.0
            speed_multiplier: 1.0,
            paused: false,
        }
    }
}

impl Plugin for LunCoCorePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(LunCoLogPlugin);
        app.register_type::<Severity>()
           .register_type::<TelemetryValue>()
           .register_type::<TelemetryEvent>()
           .register_type::<Parameter>()
           .register_type::<SampledParameter>()
           .register_type::<CelestialClock>()
           .register_type::<OrbitState>()
           .register_type::<PhysicalPort>()
           .register_type::<DigitalPort>()
           .register_type::<Wire>()
           .register_type::<PhysicalProperties>()
           .register_type::<CelestialBody>()
           .register_type::<Spacecraft>()
           .register_type::<ActiveAction>()
           .register_type::<ActionStatus>()
           .register_type::<DifferentialDrive>()
           .register_type::<AckermannSteer>()
           .add_systems(Update, wire_system);
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
