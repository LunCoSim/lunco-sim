use bevy::prelude::*;
use big_space::prelude::*;
use lunco_celestial::CelestialPlugin;
use lunco_celestial::CelestialBody;
use lunco_celestial::CelestialClock;
use lunco_materials::BlueprintMaterial;

#[test]
fn test_celestial_startup_and_movement() {
    let mut app = App::new();

    // Minimum plugins for headless simulation
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::input::InputPlugin::default());
    app.add_plugins(bevy::transform::TransformPlugin);
    app.add_plugins(bevy::asset::AssetPlugin::default());
    app.init_resource::<Assets<Mesh>>();
    app.init_resource::<Assets<StandardMaterial>>();
    app.init_resource::<Assets<bevy_shader::Shader>>();
    app.init_resource::<Assets<BlueprintMaterial>>();
    app.init_asset::<Image>();
    app.add_plugins(bevy::gizmos::GizmoPlugin);
    app.add_plugins(CelestialPlugin);
    
    // Ensure startup systems run
    app.update();
    
    let epoch_before = app.world().resource::<CelestialClock>().epoch;
    
    // 1. Verify Sun and Earth exist
    let mut query = app.world_mut().query::<(&lunco_celestial::EarthRoot, &CellCoord, &Transform)>();
    let earth = query.iter(app.world()).next().expect("No EarthRoot found");
    let earth_pos_1 = earth.2.translation;
    
    // 2. Advance clock by 10 days
    {
        let mut clock = app.world_mut().resource_mut::<CelestialClock>();
        clock.epoch += 10.0;
    }
    
    app.update();
    
    // 3. Verify Earth has moved
    let mut query = app.world_mut().query::<(&lunco_celestial::EarthRoot, &CellCoord, &Transform)>();
    let earth = query.iter(app.world()).next().expect("No EarthRoot found");
    let earth_pos_2 = earth.2.translation;
    
    assert_ne!(earth_pos_1, earth_pos_2, "Earth should have moved after 10 days");
}
