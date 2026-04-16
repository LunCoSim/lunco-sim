//! End-to-end test: SimComponent ↔ AvianSim connection propagation.
//!
//! Tests the balloon co-simulation pipeline without needing a full app update.

use bevy::prelude::*;
use lunco_cosim::*;

#[test]
fn test_balloon_force_propagation() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(CoSimPlugin);

    // Spawn balloon entity with AvianSim + SimComponent
    let balloon = app.world_mut().spawn((
        Name::new("Test Balloon"),
        Transform::from_xyz(0.0, 5.0, 0.0),
        avian3d::prelude::Position::from_xyz(0.0, 5.0, 0.0),
        avian3d::prelude::RigidBody::Dynamic,
        AvianSim::default(),
        SimComponent {
            model_name: "Balloon".into(),
            outputs: [
                ("netForce".into(), 49.05),
                ("buoyancy".into(), 98.1),
                ("volume".into(), 100.0),
            ].into_iter().collect(),
            inputs: [
                ("height".into(), 0.0),
                ("velocity".into(), 0.0),
                ("g".into(), 0.0),
            ].into_iter().collect(),
            ..default()
        },
    )).id();

    // Set AvianSim outputs (simulating Avian reading position)
    {
        let mut avian = app.world_mut().get_mut::<AvianSim>(balloon).unwrap();
        avian.outputs.insert("height".into(), 5.0);
        avian.outputs.insert("velocity_y".into(), 0.0);
    }

    // Create connections (exactly as balloon_setup does)
    app.world_mut().spawn(SimConnection {
        start_element: balloon, start_connector: "netForce".into(),
        end_element: balloon, end_connector: "force_y".into(), scale: 1.0,
    });
    app.world_mut().spawn(SimConnection {
        start_element: balloon, start_connector: "height".into(),
        end_element: balloon, end_connector: "height".into(), scale: 1.0,
    });

    // Run propagation
    app.world_mut().run_system_cached(
        lunco_cosim::systems::propagate::propagate_connections,
    ).unwrap();

    // Verify: SimComponent inputs populated with Avian state
    let comp = app.world().get::<SimComponent>(balloon).unwrap();
    assert_eq!(comp.inputs["height"], 5.0,
        "height input should be 5.0 from AvianSim output");

    // Verify: AvianSim inputs populated with SimComponent output (the force)
    let avian = app.world().get::<AvianSim>(balloon).unwrap();
    assert_eq!(avian.inputs["force_y"], 49.05,
        "force_y should carry the netForce value into Avian");
}

#[test]
fn test_balloon_connection_accumulation() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(CoSimPlugin);

    let balloon = app.world_mut().spawn((
        Name::new("Test Balloon"),
        Transform::from_xyz(0.0, 0.0, 0.0),
        avian3d::prelude::Position::from_xyz(0.0, 0.0, 0.0),
        avian3d::prelude::RigidBody::Dynamic,
        AvianSim::default(),
        SimComponent {
            model_name: "Balloon".into(),
            parameters: [("mass".into(), 2.0)].into_iter().collect(),
            outputs: [
                ("netForce".into(), 50.0),
                ("buoyancy".into(), 100.0),
            ].into_iter().collect(),
            ..default()
        },
    )).id();

    // Two connections to same target — forces should accumulate (50 + 100 = 150)
    app.world_mut().spawn(SimConnection {
        start_element: balloon, start_connector: "netForce".into(),
        end_element: balloon, end_connector: "force_y".into(), scale: 1.0,
    });
    app.world_mut().spawn(SimConnection {
        start_element: balloon, start_connector: "buoyancy".into(),
        end_element: balloon, end_connector: "force_y".into(), scale: 1.0,
    });

    // Run propagation — accumulates into AvianSim.inputs
    app.world_mut().run_system_cached(
        lunco_cosim::systems::propagate::propagate_connections,
    ).unwrap();

    let avian = app.world().get::<AvianSim>(balloon).unwrap();
    assert_eq!(avian.inputs["force_y"], 150.0,
        "Two connections should accumulate into one input");
}
