# Feature Specification: Basic Rover Physical Plant

## Problem Statement
To bootstrap the LunCoSim Bevy engine, we need to implement our first **Physical Plant**: a 1.5kg rover. This entity must expose **Actuators** (for movement) and **Sensors** (for state) to be driven by a local "Manual Signal Source" (keyboard).

## User Scenarios

### Story 1: Physical Plant Initialization
As a developer, I want to spawn a rover entity that is physically "dumb"—it has wheels and motors but no internal driving logic, only **Actuator Ports** that respond to torque signals.

**Acceptance Criteria:**
- Rover is spawned with a 1.5kg Rigidbody.
- `WheelActuator` components are present on all wheels.
- Sending a manual signal to the `WheelActuator` results in physical movement.

### Story 2: Manual Signal Bridge (WASD)
As a user, I want to drive the rover using WASD, where the keyboard input acts as a temporary "Manual Controller" sending signals to the rover's actuator ports.

**Acceptance Criteria:**
- Pressing 'W' sends a positive torque signal to the `WheelActuators`.
- The system uses a basic `CommandMux` to route these manual signals to the physical motors.

## Out of Scope
- External SIL/HIL (Reserved for Feature 002).
- Complex autonomous navigation.
