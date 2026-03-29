# Feature Specification: Basic Driveable Rover

## Problem Statement
To bootstrap the LunCoSim Bevy engine, we need a simple "Hello World" 3D environment. We need to spawn a basic physical representation of a rover (1.5kg) on a flat lunar surface and be able to drive it around using keyboard controls.

## User Scenarios

### Story 1: Basic 3D Scene
As a developer
I want to launch the Bevy app and see a 3D scene with lighting, a camera, and a ground plane
So that I have a foundation to spawn the rover.

**Acceptance Criteria:**
- Window launches successfully.
- Ground plane is visible and acts as a static physics collider.

### Story 2: Driveable Rover Entity
As a user
I want to spawn a rover and drive it around with keyboard controls (WASD/Arrows)
So that I can verify the Bevy physics engine (`bevy_rapier3d`) and input handling are working.

**Acceptance Criteria:**
- Rover is spawned with a visual representation (e.g., a simple box or primitive shapes).
- Rover has a 1.5kg Rigidbody and realistic gravity.
- Pressing movement keys applies forces/impulses to move the rover.

## Out of Scope
Anything beyond a local desktop simulation is **strictly out of scope** for this initial feature, including:
- Multiplayer / Networking
- WebAssembly (WASM) browser compilation
- OpenMCT Telemetry
- SysML / Modelica integration
- Complex terrain generation
