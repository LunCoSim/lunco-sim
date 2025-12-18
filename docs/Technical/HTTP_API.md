# HTTP API Reference

LunCoSim exposes an HTTP API for external tools and automation.

**Base URL**: `http://localhost:8082/api`

## Endpoints

### Entities & Metadata

#### `GET /entities`
List all discovered entities in the simulation.
-   **Response**: JSON array of entity objects (ID, Name, Type).

#### `GET /dictionary`
Returns the OpenMCT dictionary definition for all current telemetry points.

### Telemetry

#### `GET /telemetry/{entity_id}`
Get the latest telemetry snapshot for a specific entity.

#### `GET /telemetry/{entity_id}/history?start={ts}&end={ts}`
Get historical data samples.
-   `start`, `end`: Unix timestamps (ms).

### Events

#### `GET /events`
Get global simulation events (User join/leave, Spawn, etc.).

#### `GET /events/{entity_id}`
Get events specific to an entity.

### Commands

#### `GET /command`
List all available command definitions (supported targets and arguments).

#### `POST /command`
Execute a command on an entity.
-   **Body** (JSON):
    ```json
    {
        "target": "Rovers/Rover1",
        "command": "set_motor",
        "args": { "value": 1.0 }
    }
    ```
-   **Response**: Execution result or error.

## Example: Python Control script
```python
import requests

API_URL = "http://localhost:8082/api"

# 1. Find Rover
entities = requests.get(f"{API_URL}/entities").json()['entities']
rover_id = next(e['entity_id'] for e in entities if e['entity_type'] == "Rover")

# 2. Drive Forward
cmd = {
    "target": rover_id,
    "command": "set_motor",
    "args": {"value": 1.0}
}
requests.post(f"{API_URL}/command", json=cmd)
```
