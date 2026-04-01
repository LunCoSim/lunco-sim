use bevy::prelude::*;
use big_space::prelude::*;
use lunco_sim_celestial::CelestialPlugin;
use lunco_sim_celestial::registry::CelestialBody;
use lunco_sim_celestial::clock::CelestialClock;

#[test]
fn test_celestial_startup_and_movement() {
    let mut app = App::new();
    
    // Minimum plugins for headless simulation
    app.add_plugins(MinimalPlugins);
    app.init_resource::<Assets<Mesh>>();
    app.init_resource::<Assets<StandardMaterial>>();
    app.add_plugins(CelestialPlugin);
    
    // Ensure startup systems run
    app.update();
    
    let epoch_before = app.world().resource::<CelestialClock>().epoch;
    
    // 1. Verify Sun and Earth exist
    let mut query = app.world_mut().query::<(&CelestialBody, &CellCoord, &Transform)>();
    let bodies: Vec<_> = query.iter(app.world()).collect();
    
    assert!(bodies.len() >= 3, "Should have at least Sun, EMB and Earth");
    
    let earth = bodies.iter().find(|(b, _, _)| b.name == "Earth").expect("No Earth found");
    let earth_pos_1 = earth.2.translation;
    
    // 2. Advance clock by 10 days
    {
        let mut clock = app.world_mut().resource_mut::<CelestialClock>();
        clock.epoch += 10.0;
    }
    
    app.update();
    
    // 3. Verify Earth has moved
    let mut query = app.world_mut().query::<(&CelestialBody, &CellCoord, &Transform)>();
    let earth = query.iter(app.world()).find(|(b, _, _)| b.name == "Earth").expect("No Earth found");
    let earth_pos_2 = earth.2.translation;
    
    assert_ne!(earth_pos_1, earth_pos_2, "Earth should have moved after 10 days");
}
