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
│  ApiEntityRegistry  — ULID ↔ Bevy Entity mapping           │
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
    "target": "01ARZ7NDEKTSV4M9",
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
    mut q_fsw: Query<&mut FlightSoftware>,
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
// HTTP server starts on port 3000
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

The API uses ULID-based stable IDs (`ApiEntityId`). The `ApiEntityRegistry` maps ULIDs to Bevy `Entity` handles automatically. Entity fields in command params accept ULID strings:

```json
{ "target": "01ARZ7NDEKTSV4M9" }
```
