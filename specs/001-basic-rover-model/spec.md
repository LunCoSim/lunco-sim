# Feature Specification: Basic Rover Physical Plant

## Problem Statement
To bootstrap the LunCoSim Bevy engine, we need to implement our first **Physical Plant**: a 1.5kg rover. This entity must expose **Actuators** (for movement) and **Sensors** (for state) to be driven by a local "Manual Signal Source" (keyboard).

## User Scenarios

### Story 1: Physical Plant Initialization
As a developer, I want to spawn a rover entity that is physically "dumb"—it has wheels and motors but no internal driving logic, only **Actuator Ports** that respond to torque signals.

**Acceptance Criteria:**
- Rover is spawned with a 1.5kg Rigidbody (`avian` solver standard instead of Rapier for ECS compatibility).
- `WheelActuator` components are present on all wheels.
- Sending a manual signal to the `WheelActuator` results in physical movement.

### Story 2: Manual Signal Bridge (WASD)
As a user, I want to drive the rover using WASD, where the keyboard input acts as a temporary "Manual Controller" sending signals to the rover's actuator ports.

**Acceptance Criteria:**
- Pressing 'W' sends a positive torque signal to the `WheelActuators`.
- The system uses a basic `CommandMux` to route these manual signals to the physical motors.

### Story 3: Double-Precision Mathematical Foundation (Large World Coordinates)
As a core engine architect, I want the simulation to handle vast Earth-Moon distances without vibrating physics, so that lunar orbiters and surface rovers can exist in the same mathematical space.

**Acceptance Criteria:**
- Physics calculations (transforms and forces) are executed natively in `f64` (double precision) either natively via customized components or Avian overrides.
- **Camera-Relative Rendering**: To prevent `f32` precision jitter in Bevy's GPU pipeline, the engine binds the active Camera near the origin `(0,0,0)`. Visual `Transforms` (`f32`) of entities are dynamically updated relative to the camera's true `f64` position every frame (mirroring the *Kitten Space Agency* methodology).

### Story 4: Plugin-First Extensibility
As a third-party developer, I want to add my own custom sensors, environments, or vehicle mechanics without modifying the core LunCoSim source code.

**Acceptance Criteria:**
- **Everything is a Plugin**: The architecture leverages Bevy's native `Plugin` system to the absolute maximum. The core engine executable is merely a lightweight shell that strings together modular plugins (e.g., `app.add_plugins(LuncoCorePlugin).add_plugins(RoverPhysicsPlugin)`).
- Adding a new capability is as simple as adding a standard Rust crate to the workspace and registering its Plugin. No tangled monolithic dependencies.

## Out of Scope
- External SIL/HIL (Reserved for Feature 002).
- Complex autonomous navigation.
