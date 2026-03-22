---
name: lunco-sim-control
description: Commands and simulation control for the LunCo simulation environment. Use when interacting with the simulation API to spawn entities, control avatars, or manage simulation state.
---

# LunCo Simulation Control

Use this skill to interact with the LunCo simulation environment via its HTTP API.

## Simulation API Endpoints

The simulation API is available at `http://localhost:8082/api/command`.

### Common Commands

#### Simulation
*   `SPAWN(type: string, position: array)`: Spawn an entity.
    *   Types include: `Spacecraft`, `Operator`, `Gobot`, `Astronaut`, `Rover`, `LunarLander`, `MarsRover`, `RoverRigid`.
*   `DELETE(entity_id: string)`: Delete an entity.
*   `LIST_ENTITIES()`: List available entity types.

#### Avatar
*   `TAKE_CONTROL(target: string)`: Control an entity (e.g., "RoverRigid").
*   `STOP_CONTROL()`: Release current control.
*   `KEY_DOWN(key: enum)`: Send key down event. (Keys: w, s, a, d, q, e, space, shift, v, f)
*   `KEY_UP(key: enum)`: Send key up event.
*   `KEY_PRESS(key: enum)`: Send short key press.

## Usage Pattern

Use `curl` to interact with the API:

```bash
curl -s -X POST -H "Content-Type: application/json" \
  -d '{"name": "COMMAND", "target_path": "TARGET", "arguments": {"arg": "val"}}' \
  http://localhost:8082/api/command
```
