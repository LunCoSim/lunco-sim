# Implementation Plan: Basic Driveable Rover

## Technology Stack
- **Engine:** Bevy (Rust)
- **Physics:** `avian3d` (Native Rust physics for Bevy)

## Architecture (Universal Vessel Pattern)

### 1. Avatar & Input (Local Client)
- **Avatar Entity**: A marker component on the player's camera. Stores the ID of the currently possessed vessel.
- **Input Adapter System**: Reads Bevy `ButtonInput<KeyCode>` and writes to the `ActionState` of the possessed vessel.
    - `W/S` -> `ActionState.Throttle`
    - `A/D` -> `ActionState.Steering`

### 2. Vessel Root (The Rover)
- **Vessel Component**: A marker component for the physical assembly.
- **ActionState Component**: Stores the semantic intent (Throttle/Steering).
- **ControlAuthority Component**: Stores the `PlayerId` allowed to control this vessel.
- **CommandModule Component**: Defines the "Forward" axis for the rover.

### 3. Actuators (The Wheels)
- **WheelActuator Component**: Attached to each wheel entity.
- **WheelSystem**: 
    - Queries for `WheelActuator` on entities that are children of a `Vessel`.
    - Reads `ActionState` from the parent `Vessel`.
    - Applies torque/angle using `avian3d` (physics).
