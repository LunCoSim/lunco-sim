# Tasks: Basic Driveable Rover

## Phase 1: Engine Initialization
- [ ] 1.1 `cargo init` the Rust project.
- [ ] 1.2 Add `bevy` and `avian3d` to `Cargo.toml`.
- [ ] 1.3 Create the basic Bevy `App::new()` with `DefaultPlugins` and `PhysicsPlugins::default()`.

## Phase 2: Environment Setup
- [ ] 2.1 Add a startup system to spawn a `Camera3dBundle`.
- [ ] 2.2 Spawn a `DirectionalLightBundle` for illumination.
- [ ] 2.3 Spawn a ground plane (`PbrBundle` + `Collider::cuboid`).

## Phase 3: Rover Implementation
- [ ] 3.1 Define a `Rover` marker component.
- [ ] 3.2 Create a `spawn_rover` startup system to create a 3D box with a `RigidBody::Dynamic` and mass of 1.5kg.
- [ ] 3.3 Create a `rover_movement` system in `FixedUpdate` that reads `ButtonInput<KeyCode>` (WASD) and applies forces to the rover's `ExternalForce` component.

## Phase 4: Polish
- [ ] 4.1 Tune the physics (friction, mass, forces) so the movement feels responsive and grounded.
