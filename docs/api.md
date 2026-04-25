# LunCoSim API

Transport-agnostic API layer for LunCoSim. Exposes simulation state and typed commands via HTTP.

## Quick Start

### 1. Start the simulator with API enabled

```bash
# Default port (3000)
cargo run --bin rover_sandbox_usd -- --api

# Custom port
cargo run --bin rover_sandbox_usd -- --api 8080
```

The `--api` flag enables the HTTP server. Without it, the sim runs normally with no network exposure.

### 2. Test the API

```bash
# Run all tests
./scripts/api/test_api.sh

# Run rover drive demo
./scripts/api/demo_drive_rover.sh
```

### 3. Manual curl commands

```bash
# Health check
curl http://127.0.0.1:3000/api/health

# Discover all available commands
curl http://127.0.0.1:3000/api/commands/schema | jq .

# List all entities
curl http://127.0.0.1:3000/api/entities | jq .

# Query a specific entity
curl http://127.0.0.1:3000/api/entities/<entity-ulid> | jq .
```

## Endpoints

| Method | Endpoint | Description |
|---|---|---|
| `GET` | `/api/health` | Server health check |
| `GET` | `/api/commands/schema` | Discover all available commands with field types |
| `POST` | `/api/commands` | Execute a command |
| `GET` | `/api/entities` | List all entities |
| `GET` | `/api/entities/{id}` | Query entity details |

## Command Execution

Commands are typed — each domain crate defines its own command structs. The API discovers them automatically via reflection.

### Example: Drive a Rover

```bash
curl -X POST http://127.0.0.1:3000/api/commands \
  -H "Content-Type: application/json" \
  -d '{
    "command": "DriveRover",
    "params": {
      "target": "01ARZ7NDEKTSV4M9",
      "forward": 0.8,
      "steer": 0.0
    }
  }'
```

### Example: Brake a Rover

```bash
curl -X POST http://127.0.0.1:3000/api/commands \
  -H "Content-Type: application/json" \
  -d '{
    "command": "BrakeRover",
    "params": {
      "target": "01ARZ7NDEKTSV4M9",
      "intensity": 1.0
    }
  }'
```

### Example: Spawn an Entity

```bash
curl -X POST http://127.0.0.1:3000/api/commands \
  -H "Content-Type: application/json" \
  -d '{
    "command": "SpawnEntity",
    "params": {
      "target": "01ARZ7NDEKTSV4M9",
      "entry_id": "ball_dynamic",
      "position": { "x": 0.0, "y": 2.0, "z": 0.0 }
    }
  }'
```

## Response Format

### Success

```json
{
  "data": {
    "command_id": 42
  }
}
```

Data responses include a `data` envelope. For example, `GET /api/entities` returns:
```json
{
  "data": {
    "entities": [{"api_id": "...", "entity_index": "..."}],
    "count": 192
  }
}
```

### Error

```json
{
  "error": "Command 'UnknownCommand' not found"
}
```

## Entity IDs

The API uses ULID-based stable IDs (`ApiEntityId`). Bevy `Entity` IDs are process-local and recycled; ULIDs survive across sessions.

Entity fields in command params accept ULID strings:
```json
{ "target": "01ARZ7NDEKTSV4M9" }
```

## Adding the API to a New Binary

1. Add dependency to `Cargo.toml`:
```toml
lunco-api = { path = "../lunco-api" }
```

2. Add `--api` CLI parsing:
```rust
fn parse_api_port() -> Option<u16> {
    let args: Vec<String> = std::env::args().collect();
    for i in 0..args.len() {
        if args[i] == "--api" {
            if i + 1 < args.len() {
                if let Ok(port) = args[i + 1].parse::<u16>() {
                    return Some(port);
                }
            }
            return Some(3000);
        }
    }
    None
}
```

3. Add the plugin conditionally:
```rust
let mut app = App::new();
// ... your plugins ...

if let Some(port) = parse_api_port() {
    app.add_plugins(lunco_api::LunCoApiPlugin::new(lunco_api::LunCoApiConfig {
        http_config: Some(lunco_api::transports::HttpServerConfig { port }),
    }));
    eprintln!("🌐 API server enabled on http://0.0.0.0:{}", port);
}

app.run();
```

## Architecture

There are **two response shapes** behind `POST /api/commands`:

1. **Reflect Event commands** — fire-and-forget side effects.
   `OpenFile`, `MoveComponent`, `DriveRover`, etc. The executor
   reflects on the type, deserialises params, and triggers the
   matching `Event` for domain observers to handle. Returns
   `command_accepted` immediately.

2. **Query providers** — return structured data.
   `ListBundled`, `ListTwin`, `ListMsl`, `MslStatus`,
   `ListOpenDocuments` (and future entries from spec 033). Domain
   crates register implementations of `ApiQueryProvider` against the
   `ApiQueryRegistry`; the executor checks the registry first when
   handling an `ExecuteCommand` request, runs the provider deferred
   via `commands.queue` so it has `&mut World` access, and returns
   the resulting `ApiResponse::Ok { data }` to the transport.

The wire format is identical for both — `{"command": "...", "params": {...}}` —
so callers don't need to know which path their command takes. The
executor differentiates internally.

```
┌────────────────────────────────────────────────────────────┐
│  HTTP Client (curl, Python, MCP, Browser)                  │
└────────────────────┬───────────────────────────────────────┘
                     │ POST /api/commands
                     ▼
┌────────────────────────────────────────────────────────────┐
│  lunco-api                                                 │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────┐ │
│  │ HttpBridge   │→ │ ApiExecutor  │→ │ ApiQueryRegistry │ │  ← query?
│  │ (axum)       │  │              │  │ → provider.exec  │ │    yes → returns data
│  └──────────────┘  └─────┬────────┘  └──────────────────┘ │
│                          │ no                              │
│                          ▼                                 │
│                    ┌──────────────────┐                    │
│                    │ ApiCommandEvent  │  (Reflect dispatch)│
│                    └──────────────────┘                    │
└────────────────────┬───────────────────────────────────────┘
                     │ On<ApiCommandEvent>
                     ▼
┌────────────────────────────────────────────────────────────┐
│  Domain Observers (lunco-mobility, lunco-avatar, …)        │
│  Handle the command and mutate simulation state            │
└────────────────────────────────────────────────────────────┘
```

**Adding a new query endpoint** (data-returning): implement
`ApiQueryProvider` in the domain crate that owns the data, register
it in your plugin's `build` via
`app.world_mut().resource_mut::<ApiQueryRegistry>().register(...)`.
See `crates/lunco-modelica/src/api_queries.rs` for examples and
spec [`032-model-source-listing`](../specs/032-model-source-listing/spec.md)
for the design.

**Adding a new typed command** (side-effect): follow the existing
pattern in `skills/test-via-api/SKILL.md`.

### External API visibility

Some Reflect events exist inside the app but are **hidden from the
external API surface** by default. The mechanism is `ApiVisibility`
(see `crates/lunco-api/src/queries.rs`) — a `Resource` holding the
short type names of commands that should not appear in
`discover_schema` and that the executor rejects with
`CommandNotFound`. Domain crates push their hide-by-default events
into the set during plugin build.

Crucially, hidden ≠ unregistered. The events are still in the Bevy
type registry — the GUI dispatches them, observers fire on them, tests
use them. Only the external API surface (HTTP / MCP) is filtered. This
matches AGENTS.md §4.1 rule 3 ("UI never mutates state directly … all
UI interactions emit `CommandMessage` events"): a single Reflect event
input shape, with a separate visibility decision for who outside the
process can see it.

| Flag | Reveals |
|---|---|
| `--api-expose-edits` | Modelica document mutation commands (`SetDocumentSource`, `AddModelicaComponent`, `RemoveModelicaComponent`, `ConnectComponents`, `DisconnectComponents`). Without this flag the external API is effectively read-only on Modelica sources, while the workbench GUI itself remains fully editable. See `crates/lunco-modelica/src/api_edits.rs`. |

To add a new opt-in mutation surface in another domain: register the
events as usual, then in the plugin's `build` push their names into
`ApiVisibility` unless a corresponding flag was supplied.

## Binaries with API Support

| Binary | Flag | Default Port |
|---|---|---|
| `rover_sandbox_usd` | `--api [PORT]` | 3000 |
| `modelica_workbench` | `--api [PORT]` | 3000 |
| `model_viewer` | `--api [PORT]` | 3000 |

## Troubleshooting

| Issue | Solution |
|---|---|
| Connection refused | Make sure sim was started with `--api` flag |
| "Command not found" | Check `/api/commands/schema` for available commands |
| "Entity not found" | Check `/api/entities` for valid ULID strings |
| `lunco_api` not found in `Cargo.toml` | Add `lunco-api = { path = "../lunco-api" }` dependency |
