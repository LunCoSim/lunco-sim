//! End-to-end test: SimComponent ↔ AvianSim wire propagation.
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
        avian3d::prelude::RigidBody::Kinematic,
        systems::apply_forces::BalloonVelocity(Vec3::ZERO),
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

    // Create wires (exactly as balloon_setup does)
    app.world_mut().spawn(SimConnection {
        start_element: balloon, start_connector: "netForce".into(),
        end_element: balloon, end_connector: "force_y".into(), scale: 1.0,
    });
    app.world_mut().spawn(SimConnection {
        start_element: balloon, start_connector: "height".into(),
        end_element: balloon, end_connector: "height".into(), scale: 1.0,
    });

    // Run wire propagation
    app.world_mut().run_system_cached(
        lunco_cosim::systems::propagate::propagate_connections,
    ).unwrap();

    // Verify: SimComponent inputs should have been updated with Avian state
    let comp = app.world().get::<SimComponent>(balloon).unwrap();
    assert_eq!(comp.inputs["height"], 5.0,
        "height input should be 5.0 from AvianSim output");
}

#[test]
fn test_balloon_wire_accumulation() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(CoSimPlugin);
    
    // Add Time<Fixed> resource
    app.insert_resource(Time::<Fixed>::from_seconds(0.016));

    let balloon = app.world_mut().spawn((
        Name::new("Test Balloon"),
        Transform::from_xyz(0.0, 0.0, 0.0),
        avian3d::prelude::Position::from_xyz(0.0, 0.0, 0.0),
        avian3d::prelude::RigidBody::Kinematic,
        systems::apply_forces::BalloonVelocity(Vec3::ZERO),
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

    // Two wires to same target — forces should accumulate (50 + 100 = 150)
    app.world_mut().spawn(SimConnection {
        start_element: balloon, start_connector: "netForce".into(),
        end_element: balloon, end_connector: "force_y".into(), scale: 1.0,
    });
    app.world_mut().spawn(SimConnection {
        start_element: balloon, start_connector: "buoyancy".into(),
        end_element: balloon, end_connector: "force_y".into(), scale: 1.0,
    });

    // Run propagation (copies outputs to AvianSim inputs)
    app.world_mut().run_system_cached(
        lunco_cosim::systems::propagate::propagate_connections,
    ).unwrap();

    // Verify AvianSim inputs
    {
        let avian = app.world().get::<AvianSim>(balloon).unwrap();
        assert!(avian.inputs.contains_key("force_y"), "AvianSim should have force_y input");
        assert_eq!(avian.inputs["force_y"], 150.0);
    }

    // Advance time so delta_secs_f64() is non-zero
    {
        let mut time = app.world_mut().resource_mut::<Time<Fixed>>();
        let period = time.timestep();
        time.advance_by(period);
    }

    // Run force application (integrates F/m to velocity; position integration
    // is handled by Avian's integrate_positions from LinearVelocity).
    app.world_mut().run_system_cached(
        lunco_cosim::systems::apply_forces::apply_sim_forces,
    ).unwrap();

    // Verify velocity integrated correctly
    // F = 150, m = 2.0, a = 75.0, dt = 0.016
    // dv = 75 * 0.016 = 1.2
    let lin_vel = app.world().get::<avian3d::prelude::LinearVelocity>(balloon).unwrap();
    assert!((lin_vel.0.y - 1.2).abs() < 1e-6,
        "Expected LinearVelocity.y approx 1.2, got {}", lin_vel.0.y);

    // BalloonVelocity should mirror LinearVelocity
    let bv = app.world().get::<systems::apply_forces::BalloonVelocity>(balloon).unwrap();
    assert!((bv.0.y as f64 - 1.2).abs() < 1e-4,
        "Expected BalloonVelocity.y approx 1.2, got {}", bv.0.y);
}
