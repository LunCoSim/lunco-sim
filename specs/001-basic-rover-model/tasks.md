# Tasks: Basic Driveable Rover


## Phase 1: Engine Initialization
- [ ] 1.1 `cargo init` the Rust project.
- [ ] 1.2 Add `bevy` and `avian3d` (or `bevy_xpbd_3d` successor) to `Cargo.toml`.
- [ ] 1.3 Write a failing test `test_app_initializes_with_plugins` to verify the ECS initializes safely.
- [ ] 1.4 Create the basic Bevy `App::new()` with `DefaultPlugins` and `PhysicsPlugins::default()` to pass the test.

## Phase 2: Environment Setup
- [ ] 2.1 Write failing tests: `test_camera_spawns`, `test_light_spawns`, `test_ground_plane_spawns`.
- [ ] 2.2 Implement startup systems to spawn `Camera3dBundle`, `DirectionalLightBundle`, and the physics ground plane to pass tests.

## Phase 3: Rover Implementation
- [ ] 3.1 Define a `Rover` marker component.
- [ ] 3.2 Write a failing test `test_rover_spawns_with_1_5kg_mass`.
- [ ] 3.3 Create a `spawn_rover` startup system to pass the physics initialization test.
- [ ] 3.4 Write a failing test `test_rover_movement_applies_forces` that simulates WASD input and checks `ExternalForce` or `LinearVelocity` within `FixedUpdate`.
- [ ] 3.5 Create a `rover_movement` system to pass the deterministic movement test.

## Phase 4: Polish
- [ ] 4.1 Tune the physics (friction, mass, forces) so the movement feels responsive and grounded.
