# Implementation Plan: Basic Driveable Rover

## Technology Stack
- **Engine:** Bevy (Rust)
- **Physics:** `avian3d` (Native Rust physics for Bevy)

## Architecture

### Component Design

#### 1. Environment Setup
- **Responsibility:** Spawns a 3D Camera, a directional light, and a static ground plane.

#### 2. Rover Component (`Rover`)
- **Responsibility:** A marker component for the rover entity to easily query it in systems.

#### 3. Rover Spawner System
- **Responsibility:** Spawns a `PbrBundle` (visuals) alongside Avian physics components (`RigidBody::Dynamic`, `Collider`, `MassPropertiesBundle`).

#### 4. Movement System
- **Responsibility:** Runs in Bevy's `FixedUpdate` schedule to ensure determinism. Queries for the `Rover` component and `Res<ButtonInput<KeyCode>>`. Applies physical forces or impulses to the rover's `ExternalForce` or `LinearVelocity` component.
