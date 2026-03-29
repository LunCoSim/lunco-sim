# Tasks: Modelica Subsystem Simulation


## Phase 1: Rumoca Modelica Setup
- [ ] 1.1 Write a failing test `test_rumoca_parses_battery_model` verifying that rumoca can load a basic solar-battery Modelica definition.
- [ ] 1.2 Create the Modelica model file and add the `rumoca` dependency to `Cargo.toml` to pass the parsing test.

## Phase 2: Bevy FixedUpdate Subsystem
- [ ] 2.1 Write a failing Bevy integration test `test_modelica_step_updates_battery` verifying that moving the rover forwards mathematically depletes the battery.
- [ ] 2.2 Create a Bevy system in `FixedUpdate` that initializes the Modelica model via rumoca.
- [ ] 2.3 Create the step-logic system that reads the Bevy `LinearVelocity` state, feeds it into rumoca, and updates the Bevy `BatteryLevel` component to pass the test.
