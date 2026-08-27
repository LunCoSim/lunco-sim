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

- **No hardcoded commands**: Any registered `#[Command]` type is automatically discoverable via `AppTypeRegistry` reflection; arbitrary internal reflected events are excluded.
- **Transport-independent**: HTTP is one optional transport. The core types know nothing about HTTP.
- **Headless-compatible**: Runs without GPU/graphics. Perfect for server deployments.

## Commands

Commands are discovered automatically. The API scans `AppTypeRegistry` for reflected events carrying the marker emitted by `#[Command]`. A command must still be registered by its owning plugin so its observer and reflected type exist in the running host.

### HTTP Endpoint

```
POST /api/commands
Content-Type: application/json

{
  "type": "ExecuteCommand",
  "command": "SetPorts",
  "params": {
    "target": 42,
    "writes": [["throttle", 0.8], ["steer", 0.0], ["brake", 0.0]],
    "seq": 0,
    "tick": 0
  }
}
```

### Response

```json
{
  "data": {
    "accepted": true
  }
}
```

The response envelope contains the command handler's result data when the
command returns one. A command with no result data returns the `accepted`
object above. For example, a result-returning command may respond with
`{"data":{"queued":true,"operations":1}}`. Long-running commands keep the
same envelope and send their completed result when their owner finishes.

### Schema Discovery

```
GET /api/commands/schema
```

Returns all available commands with their field types:

```json
{
  "commands": [
    {
      "name": "LoadScene",
      "fields": [
        { "name": "path", "type_name": "alloc::string::String" },
        { "name": "root_prim", "type_name": "alloc::string::String" }
      ]
    }
  ],
  "queries": ["GetBrokenConnections", "GetReadiness", "ListPorts", "Nearest", "ReadExposures", "ReadPorts"]
}
```

Data-returning queries use the same `POST /api/commands` envelope as commands.
`ReadExposures` reads the generic runtime capability registry used by HTML/CSS
surfaces and other clients:

```json
{
  "type": "ExecuteCommand",
  "command": "ReadExposures",
  "params": { "surface": "hud" }
}
```

The response contains the current `revision` and typed surface properties.
Clients can avoid rebuilding a view while that revision is unchanged.

## Domain Observer Integration

Commands triggered via API arrive as `ApiCommandEvent`. Domain observers can handle them two ways:

**Option 1: Observe `ApiCommandEvent` directly**
```rust
fn on_set_ports_api(
    trigger: On<ApiCommandEvent>,
    mut q_inputs: Query<&mut InputPorts>,
) {
    if trigger.event().command != "SetPorts" { return; }
    let params = &trigger.event().params;
    let writes = &params["writes"];
    // ... handle command
}
```

**Option 2: Use the typed command + API event**
```rust
// Internal trigger
fn on_set_ports_internal(trigger: On<SetPorts>, ...) { ... }

// API trigger
fn on_set_ports_api(trigger: On<ApiCommandEvent>, ...) {
    if trigger.event().command == "SetPorts" {
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
