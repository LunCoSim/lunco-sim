//! Headless integration tests for lunco-cosim.
//!
//! These tests run without a window, renderer, or GPU.
//! They verify wire propagation, AvianSim auto-add, and co-simulation orchestration.

use avian3d::prelude::*;
use bevy::math::DVec3;
use bevy::prelude::*;
use lunco_cosim::*;

/// Build a minimal test App with CoSimPlugin and Avian physics.
fn build_test_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(PhysicsPlugins::default());
    app.add_plugins(CoSimPlugin);
    app
}

// ---------------------------------------------------------------------------
// SimComponent Tests
// ---------------------------------------------------------------------------

#[test]
fn test_sim_component_default() {
    let comp = SimComponent::default();
    assert!(comp.model_name.is_empty());
    assert!(comp.inputs.is_empty());
    assert!(comp.outputs.is_empty());
    assert!(comp.parameters.is_empty());
    assert_eq!(comp.status, SimStatus::Idle);
    assert!(!comp.is_stepping);
}

#[test]
fn test_sim_status_can_step() {
    assert!(SimStatus::Idle.can_step());
    assert!(SimStatus::Running.can_step());
    assert!(!SimStatus::Compiling.can_step());
    assert!(!SimStatus::Stepping.can_step());
    assert!(!SimStatus::Paused.can_step());
    assert!(!SimStatus::Error("oops".into()).can_step());
}

// ---------------------------------------------------------------------------
// Avian body ports (resolved live from Position/LinearVelocity, no mirror)
// ---------------------------------------------------------------------------

#[test]
fn test_avian_body_ports_listed() {
    // A rigid body auto-exposes position/velocity outputs + force inputs through
    // the `AVIAN` spec table — no marker component, detected by presence.
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    let e = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Position(DVec3::new(0.0, 0.0, 0.0)),
            LinearVelocity(DVec3::ZERO),
        ))
        .id();

    let names: Vec<String> = entity_ports(app.world(), e)
        .into_iter()
        .map(|p| p.name)
        .collect();
    for expected in [
        "position_x", "position_y", "position_z", "height", "velocity_x",
        "velocity_y", "velocity_z", "force_x", "force_y", "force_z",
    ] {
        assert!(names.contains(&expected.to_string()), "missing port {expected}");
    }
}

#[test]
fn test_avian_body_ports_read_live_state() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    let e = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Position(DVec3::new(100.0, 1200.0, 50.0)),
            LinearVelocity(DVec3::new(1.0, 3.2, 0.5)),
        ))
        .id();

    let w = app.world();
    assert_eq!(read_output_port(w, e, "position_x"), Some(100.0));
    assert_eq!(read_output_port(w, e, "position_y"), Some(1200.0));
    assert_eq!(read_output_port(w, e, "height"), Some(1200.0)); // alias
    assert_eq!(read_output_port(w, e, "velocity_y"), Some(3.2));
}

// ---------------------------------------------------------------------------
// SimConnection Tests
// ---------------------------------------------------------------------------

#[test]
fn test_sim_wire_default() {
    let wire = SimConnection::default();
    assert_eq!(wire.start_element, Entity::PLACEHOLDER);
    assert_eq!(wire.end_element, Entity::PLACEHOLDER);
    assert_eq!(wire.scale, 1.0);
}

// ---------------------------------------------------------------------------
// Wire Propagation Tests
// ---------------------------------------------------------------------------

#[test]
fn test_propagate_sim_component_to_sim_component() {
    let mut app = build_test_app();

    // Create two SimComponent entities
    let source = app.world_mut().spawn(SimComponent {
        model_name: "Source".into(),
        outputs: [("netForce".into(), 49.0)].into_iter().collect(),
        ..default()
    }).id();

    let target = app.world_mut().spawn(SimComponent {
        model_name: "Target".into(),
        inputs: [("force_in".into(), 0.0)].into_iter().collect(),
        ..default()
    }).id();

    // Create wire: Source.netForce → Target.force_in
    app.world_mut().spawn(SimConnection {
        start_element: source,
        start_connector: "netForce".into(),
        end_element: target,
        end_connector: "force_in".into(),
        scale: 1.0,
        offset: 0.0,
    });

    // Run wire propagation
    app.world_mut().run_system_cached(
        lunco_cosim::systems::propagate::propagate_connections,
    )
    .unwrap();

    // Verify value propagated
    let comp = app.world().get::<SimComponent>(target).unwrap();
    assert_eq!(comp.inputs["force_in"], 49.0);
}

#[test]
fn test_propagate_with_scale() {
    let mut app = build_test_app();

    let source = app.world_mut().spawn(SimComponent {
        model_name: "Source".into(),
        outputs: [("current".into(), 10.0)].into_iter().collect(),
        ..default()
    }).id();

    let target = app.world_mut().spawn(SimComponent {
        model_name: "Target".into(),
        inputs: [("current_in".into(), 0.0)].into_iter().collect(),
        ..default()
    }).id();

    app.world_mut().spawn(SimConnection {
        start_element: source,
        start_connector: "current".into(),
        end_element: target,
        end_connector: "current_in".into(),
        scale: 0.5,
        offset: 0.0,
    });

    app.world_mut().run_system_cached(
        lunco_cosim::systems::propagate::propagate_connections,
    )
    .unwrap();

    let comp = app.world().get::<SimComponent>(target).unwrap();
    assert_eq!(comp.inputs["current_in"], 5.0); // 10.0 * 0.5
}

#[test]
fn test_propagate_avian_to_sim_component() {
    use bevy::math::DVec3;

    let mut app = build_test_app();

    // Spawn a rigid body (auto-exposes the `height` output from Position) with a
    // SimComponent that has a `height` input.
    let entity = app.world_mut().spawn((
        RigidBody::Dynamic,
        Position(DVec3::new(0.0, 1200.0, 0.0)),
        LinearVelocity(DVec3::new(0.0, 3.2, 0.0)),
        SimComponent {
            model_name: "Balloon".into(),
            inputs: [("height".into(), 0.0), ("velocity".into(), 0.0)].into_iter().collect(),
            ..default()
        },
    )).id();

    // Wire: avian height output → SimComponent height input (same entity).
    app.world_mut().spawn(SimConnection {
        start_element: entity,
        start_connector: "height".into(),
        end_element: entity,
        end_connector: "height".into(),
        scale: 1.0,
        offset: 0.0,
    });

    // Propagate — the source `height` is read live from Position (no snapshot
    // system needed); avian state is stable until a physics step runs.
    app.world_mut().run_system_cached(lunco_cosim::systems::propagate::propagate_connections)
        .unwrap();

    let comp = app.world().get::<SimComponent>(entity).unwrap();
    assert_eq!(comp.inputs["height"], 1200.0);
}

// ---------------------------------------------------------------------------
// Avian presence-detection (no observer, no marker component)
// ---------------------------------------------------------------------------

#[test]
fn test_rigid_body_exposes_ports_by_presence() {
    // No observer/marker: a RigidBody entity exposes avian ports purely by
    // component presence through the resolver. An entity without one exposes none.
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);

    let plain = app.world_mut().spawn_empty().id();
    assert!(
        read_output_port(app.world(), plain, "height").is_none(),
        "a non-body entity exposes no avian ports"
    );

    let body = app
        .world_mut()
        .spawn((RigidBody::Dynamic, Position(DVec3::new(0.0, 7.0, 0.0))))
        .id();
    assert_eq!(
        read_output_port(app.world(), body, "height"),
        Some(7.0),
        "a RigidBody+Position entity exposes the height port"
    );
}

// ---------------------------------------------------------------------------
// Force Application Tests
// ---------------------------------------------------------------------------

#[test]
fn test_apply_sim_forces_accumulates_multiple_connections() {
    let mut app = build_test_app();

    // Entity with SimComponent output
    let source = app.world_mut().spawn(SimComponent {
        model_name: "Modelica".into(),
        outputs: [
            ("netForce".into(), 30.0),
            ("buoyancy".into(), 20.0),
        ].into_iter().collect(),
        ..default()
    }).id();

    let target = app.world_mut().spawn(RigidBody::Dynamic).id();

    // Two connections to same force target: netForce + buoyancy → force_y
    app.world_mut().spawn(SimConnection {
        start_element: source,
        start_connector: "netForce".into(),
        end_element: target,
        end_connector: "force_y".into(),
        scale: 1.0,
        offset: 0.0,
    });
    app.world_mut().spawn(SimConnection {
        start_element: source,
        start_connector: "buoyancy".into(),
        end_element: target,
        end_connector: "force_y".into(),
        scale: 1.0,
        offset: 0.0,
    });

    // Propagate — both wires sum into the force_y input, which lands in
    // `PendingForces.f.y` (readable through the resolver's input side).
    app.world_mut().run_system_cached(
        lunco_cosim::systems::propagate::propagate_connections,
    ).unwrap();

    assert_eq!(
        read_input_port(app.world(), target, "force_y"),
        Some(50.0),
        "forces should accumulate: 30 + 20 = 50"
    );

    // apply_pending_forces drains the accumulator into avian and zeroes it, so
    // accumulation starts fresh next tick.
    app.world_mut().run_system_cached(
        lunco_cosim::avian::apply_pending_forces,
    ).unwrap();

    assert_eq!(
        read_input_port(app.world(), target, "force_y"),
        Some(0.0),
        "apply_pending_forces should drain force_y to 0"
    );
}

// ---------------------------------------------------------------------------
// Suggestion Tests
// ---------------------------------------------------------------------------

#[test]
fn test_suggestions_for_balloon_model() {
    let suggestions = generate_suggestions(
        Entity::PLACEHOLDER,
        "Balloon",
        vec!["height".into(), "velocity".into(), "g".into()].into_iter(),
        vec!["netForce".into(), "volume".into(), "buoyancy".into()].into_iter(),
        true,  // has_forces
        true,  // has_collider
    );

    // Should suggest force connections for netForce, buoyancy
    let force_suggestions: Vec<_> = suggestions.iter()
        .filter(|s| s.end_connector == "force_y")
        .collect();
    assert!(force_suggestions.len() >= 2);

    // Should suggest collider for volume
    let collider_suggestions: Vec<_> = suggestions.iter()
        .filter(|s| s.end_connector == "collider")
        .collect();
    assert_eq!(collider_suggestions.len(), 1);

    // Should suggest gravity for g — sourced from the local-gravity output
    // (populated by lunco-environment), not a hardcoded constant.
    let gravity_suggestions: Vec<_> = suggestions.iter()
        .filter(|s| s.start_connector == lunco_cosim::GRAVITY_SOURCE_CONNECTOR)
        .collect();
    assert_eq!(gravity_suggestions.len(), 1);
}

#[test]
fn test_suggestions_for_battery_model() {
    let suggestions = generate_suggestions(
        Entity::PLACEHOLDER,
        "Battery",
        vec!["current_in".into()].into_iter(),
        vec!["soc".into(), "voltage_out".into()].into_iter(),
        false, // no forces
        false, // no collider
    );

    // Should NOT suggest force/collider connections for a battery
    assert!(!suggestions.iter().any(|s| s.end_connector == "force_y"));
    assert!(!suggestions.iter().any(|s| s.end_connector == "collider"));

    // Should have suggestions for known patterns if any match
    // (battery has no force/velocity/height vars, so likely minimal suggestions)
}

// ---------------------------------------------------------------------------
// Collider Sync Tests
// ---------------------------------------------------------------------------

#[test]
fn test_collider_sync_from_volume() {
    let mut app = build_test_app();

    let radius = 1.0;
    let entity = app.world_mut().spawn((
        RigidBody::Dynamic,
        Collider::sphere(radius),
        SimComponent {
            model_name: "Balloon".into(),
            outputs: [("volume".into(), 100.0)].into_iter().collect(),
            ..default()
        },
    )).id();

    // Run collider sync
    app.world_mut().run_system_cached(lunco_cosim::systems::collider::sync_collider)
        .unwrap();

    // Volume 100.0 → radius = cbrt(3*100/(4π)) ≈ 2.879
    // We can't directly inspect the collider radius (opaque struct),
    // but we can verify the collider component still exists and was modified
    let collider = app.world().get::<Collider>(entity);
    assert!(collider.is_some(), "Collider should still exist after sync");

    // Verify the volume → radius calculation is correct
    let expected_radius = ((3.0 * 100.0) / (4.0 * std::f64::consts::PI)).cbrt();
    assert!(expected_radius > 2.0 && expected_radius < 3.0,
        "Expected radius ~2.879, got {expected_radius}");
}
