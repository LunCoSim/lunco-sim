# Implementation Plan: Basic Driveable Rover

## Technology Stack
- **Engine:** Bevy (Rust)
- **Physics:** `bevy_rapier3d`

## Architecture

### Component Design

#### 1. Environment Setup
- **Responsibility:** Spawns a 3D Camera, a directional light, and a static ground plane.

#### 2. Rover Component (`Rover`)
- **Responsibility:** A marker component for the rover entity to easily query it in systems.

#### 3. Rover Spawner System
- **Responsibility:** Spawns a `PbrBundle` (visuals) alongside Rapier physics components (`RigidBody::Dynamic`, `Collider`, `ColliderMassProperties::Mass(1.5)`).

#### 4. Movement System
- **Responsibility:** Queries for the `Rover` component and `Res<ButtonInput<KeyCode>>`. Applies kinematic movement or physical forces to the rover's `ExternalForce` or `Velocity` component.
