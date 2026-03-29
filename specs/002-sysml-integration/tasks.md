# Tasks: SysML v2 Integration


## Phase 1: Parsing SysML
- [ ] 1.1 Write failing tests parsing a mock `.sysml` file representing a 1.5kg rover chassis.
- [ ] 1.2 Implement the Rust parser/deserializer to pass the parsing tests.

## Phase 2: Bevy ECS Integration
- [ ] 2.1 Write failing unit tests asserting that reading a valid SysML struct correctly generates Bevy `MassPropertiesBundle` and `Collider` components.
- [ ] 2.2 Refactor the `spawn_rover` logic from feature `001` to loop through the parsed SysML structure instead of using hardcoded values, passing the tests.
- [ ] 2.3 Write an integration test to verify that loading a modified `.sysml` config correctly alters the rover's properties in the Bevy World.
