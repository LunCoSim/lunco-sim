# lunco-api

Transport-agnostic API layer for LunCoSim. Exposes simulation state and typed commands via HTTP, with support for future transports (ROS2, IPC, DDS, WebSocket).

## Architecture

```
┌────────────────────────────────────────────────────────────┐
│  Transports                                                │
│  HTTP (axum) │ ROS2 │ IPC │ DDS │ WebSocket                │
└────────────────────┬───────────────────────────────────────┘
                     │
                     ▼
┌────────────────────────────────────────────────────────────┐
│  lunco-api-core                                            │
│  ApiEntityRegistry  — GlobalEntityId (u64) ↔ Bevy Entity   │
│  ApiExecutor        — ApiRequest → ECS                    │
│  ApiDiscovery       — schema introspection via reflection  │
│  ApiTelemetry       — telemetry subscription + broadcast   │
└────────────────────┬───────────────────────────────────────┘
                     │
                     ▼
┌────────────────────────────────────────────────────────────┐
│  ECS World                                                 │
│  #[Command] types · Resources · ApiCommandEvent            │
└────────────────────────────────────────────────────────────┘
```

## Key Design

- **No hardcoded commands**: Any `#[Command]` type is automatically discoverable via `AppTypeRegistry` reflection.
- **Transport-independent**: HTTP is one optional transport. The core types know nothing about HTTP.
- **Headless-compatible**: Runs without GPU/graphics. Perfect for server deployments.

## Commands

Commands are discovered automatically. The API scans `AppTypeRegistry` for types that implement `Event + Reflect`. Every `#[Command]` struct in the workspace is available.

### HTTP Endpoint

```
POST /api/commands
Content-Type: application/json

{
  "command": "DriveRover",
  "params": {
    "target": 42,
    "forward": 0.8,
    "steer": 0.0
  }
}
```

### Response

```json
{
  "command_id": 42,
  "status": "queued"
}
```

### Schema Discovery

```
GET /api/commands/schema
```

Returns all available commands with their field types:

```json
{
  "commands": [
    {
      "name": "DriveRover",
      "fields": [
        { "name": "target", "type_name": "bevy::prelude::Entity" },
        { "name": "forward", "type_name": "f64" },
        { "name": "steer", "type_name": "f64" }
      ]
    }
  ]
}
```

## Domain Observer Integration

Commands triggered via API arrive as `ApiCommandEvent`. Domain observers can handle them two ways:

**Option 1: Observe `ApiCommandEvent` directly**
```rust
fn on_drive_rover_api(
    trigger: On<ApiCommandEvent>,
    mut q_inputs: Query<&mut CommandInputs>,
) {
    if trigger.event().command != "DriveRover" { return; }
    let params = &trigger.event().params;
    let forward = params["forward"].as_f64().unwrap_or(0.0);
    // ... handle command
}
```

**Option 2: Use the typed command + API event**
```rust
// Internal trigger
fn on_drive_rover_internal(trigger: On<DriveRover>, ...) { ... }

// API trigger
fn on_drive_rover_api(trigger: On<ApiCommandEvent>, ...) {
    if trigger.event().command == "DriveRover" {
        // Same logic, different source
    }
}
```

## Usage

```rust
use lunco_api::LunCoApiPlugin;

app.add_plugins(LunCoApiPlugin::default());
// HTTP server starts on port 4101
```

With custom config:

```rust
use lunco_api::{LunCoApiPlugin, LunCoApiConfig, transports::HttpServerConfig};

app.add_plugins(LunCoApiPlugin::new(LunCoApiConfig {
    http_config: Some(HttpServerConfig { port: 8080 }),
}));
```

## Features

| Feature | Description |
|---|---|
| `transport-http` | HTTP transport via axum (default) |

## Entity IDs

The API addresses entities by **numeric** `GlobalEntityId` (a `u64`, defined in
`lunco-core`), *not* a ULID string. The `ApiEntityRegistry` resource maintains a
bidirectional `GlobalEntityId ↔ Bevy Entity` map; `sync_api_registry` keeps it
in step as entities carrying a `GlobalEntityId` component are added/removed.
Entity fields in command params are plain JSON numbers:

```json
{ "target": 42 }
```

(`ListEntities` / discovery responses report the same numeric ids, so a client
reads an id from one call and passes it straight back as a command param.)
