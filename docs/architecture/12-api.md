# LunCoSim API

Transport-agnostic API layer for LunCoSim. Exposes simulation state and typed commands via HTTP.

## Quick Start

### 1. Start the simulator with API enabled

```bash
# Default port (4101)
cargo run --bin sandbox -- --api

# Custom port
cargo run --bin sandbox -- --api 8080
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
curl http://127.0.0.1:4101/api/health

# Discover all available commands
curl http://127.0.0.1:4101/api/commands/schema | jq .

# List all entities
curl http://127.0.0.1:4101/api/entities | jq .

# Query a specific entity
curl http://127.0.0.1:4101/api/entities/<entity-ulid> | jq .
```

## Endpoints

| Method | Endpoint | Description |
|---|---|---|
| `GET` | `/api/health` | Server health check |
| `GET` | `/api/commands/schema` | Discover all available commands with field types |
| `POST` | `/api/commands` | Execute a command or query |
| `GET` | `/api/entities` | List all entities |
| `GET` | `/api/entities/{id}` | Query entity details |

## API Queries (Data Retrieval)

Queries return structured data from the simulation. They use the same `POST /api/commands` endpoint but do not cause side effects.

### Query Catalog

| Query | Parameters | Description |
|---|---|---|
| `ListBundled` | `{}` | List embedded example models (`bundled://`). |
| `ListOpenDocuments` | `{}` | List all documents currently open in the workspace. |
| `ListRecentFiles` | `{}` | List recently opened files and Twins from `recents.json`. |
| `ListTwin` | `{"offset": u64, "limit": u64}` | List files in the currently active Twin folder. |
| `ListMsl` | `{"cursor": string, "limit": u64, "filter": {...}}` | Search and list the Modelica Standard Library (MSL). |
| `ListCompileCandidates` | `{"doc": u64}` | List all non-package classes in a document that can be compiled. |
| `QueryExperimentBounds` | `{"doc": u64, "class": string?}` | Resolve simulation bounds (start, end, dt) for a class. |
| `CompileStatus` | `{"doc": u64}` | Get the current compilation and run state of a document. |
| `RunStatus` | `{"experiment_id": string}` | Get the status of a specific simulation run. |
| `ListRuns` | `{"doc": u64?}` | List all simulation runs, optionally filtered by document. |
| `GetExperimentResult` | `{"experiment_id": string, "max_points": u64?}` | Retrieve trajectory data (timeseries) for a completed run. |
| `GetDocumentSource` | `{"doc": u64}` | Get the raw source code of a document (Modelica only). |
| `DescribeModel` | `{"doc": u64, "class": string?}` | Get structural info (components, pins, parameters) of a class. |
| `SnapshotVariables` | `{"doc": u64, "names": string[]?}` | Get the current values of simulation variables/inputs. |
| `FindModel` | `{"query": string, "limit": u64?}` | Fuzzy search across bundled, twin, MSL, and open docs. |
| `SetModelInput` | `{"doc": u64, "name": string, "value": f64}` | Set a model input value (returns success/error payload). |
| `CopyShareLink` | `{"doc": u64?}` | Generate a sharing URL for the document source. |
| `CosimStatus` | `{}` | List all USD-driven cosim entities with live telemetry. |

---

## Command Execution (Side Effects)

Commands are typed — each domain crate defines its own command structs. The API discovers them automatically via reflection. Use `GET /api/commands/schema` to see the full list of available commands and their parameters.

### Common Commands

| Domain | Command | Description |
|---|---|---|
| **Mobility** | `DriveRover` | Set forward/steer inputs for a rover. |
| | `BrakeRover` | Apply brakes to a rover. |
| **Avatar** | `PossessVessel` | Attach camera and control to a vessel. |
| | `FollowTarget` | Chase-camera a target. |
| | `FocusTarget` | Orbit-camera a target. |
| | `CaptureScreenshot` | Trigger an in-sim screenshot. |
| **USD** | `LoadScene` | Reload a USD stage from disk. |
| | `ApplyUsdOp` | Mutate a USD document via an atomic Op. |
| **Modelica** | `CompileModel` | Compile a specific class in a document. |
| | `RunActiveModel` | Start/Resume simulation of the active model. |
| | `PauseActiveModel` | Pause simulation. |
| | `ResetActiveModel` | Reset simulation to `t=0`. |
| **Workspace** | `OpenFile` | Open a file (USD, Modelica, etc.) into a new tab. |
| | `SaveAll` | Save all dirty documents to disk. |
| | `NewDocument` | Create a new untitled document. |
| | `AddTwin` | Create a new Twin folder and manifest. |
| **System** | `SetTheme` | Switch between Dark and Light modes. |
| | `TogglePerfHud` | Show/hide the performance overlay. |
| | `RunPython` | Execute a Python script snippet. |

### Example: Drive a Rover

```bash
curl -X POST http://127.0.0.1:4101/api/commands \
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
curl -X POST http://127.0.0.1:4101/api/commands \
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
curl -X POST http://127.0.0.1:4101/api/commands \
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

### Example: Reload USD scene at runtime

`LoadScene` despawns every entity carrying `UsdPrimPath` plus every
`SimConnection`, force-reads the asset from disk, and spawns a fresh
root parented to the first `Grid`. Use after editing a `.usda` file to
pick up changes without restarting.

```bash
curl -X POST http://127.0.0.1:4101/api/commands \
  -H "Content-Type: application/json" \
  -d '{
    "command": "LoadScene",
    "params": {
      "path": "scenes/sandbox/sandbox_scene.usda",
      "root_prim": ""
    }
  }'
```

`root_prim` empty auto-derives `/PascalCaseFromFilename`
(`sandbox_scene.usda` → `/SandboxScene`).

### Example: Possess / Follow / Focus

Three avatar-camera commands, all share `{avatar, target}`:

```bash
# Take direct control (rover, spacecraft)
curl -X POST http://127.0.0.1:4101/api/commands \
  -d '{"command":"PossessVessel","params":{"avatar":"01ARZ...","target":"01ARZ..."}}'

# Chase camera only — any SelectableRoot (balloons, props)
curl -X POST http://127.0.0.1:4101/api/commands \
  -d '{"command":"FollowTarget","params":{"avatar":"01ARZ...","target":"01ARZ..."}}'

# Orbit a celestial body
curl -X POST http://127.0.0.1:4101/api/commands \
  -d '{"command":"FocusTarget","params":{"avatar":"01ARZ...","target":"01ARZ..."}}'
```

### Example: Live cosim status

`CosimStatus` returns one row per USD-driven cosim entity
(`UsdSourcedCosim`) with position, velocity, Modelica timing, and
propagated wire values:

```bash
curl -X POST http://127.0.0.1:4101/api/commands \
  -H "Content-Type: application/json" \
  -d '{"command":"CosimStatus","params":{}}' | jq
```

```json
{
  "data": {
    "entities": [
      {
        "name": "/SandboxScene/RedBalloon",
        "y": 17.27,
        "vy": 3.04,
        "has_simcomponent": true,
        "modelica_var_count": 7,
        "modelica_current_time": 9.62,
        "netForce": 44.16,
        "force_y_input": 44.16,
        "buoyancy": 71.55
      }
    ]
  }
}
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
            return Some(4101);
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

1. **Reflect Event commands** — side effects. `OpenFile`,
   `MoveComponent`, `DriveRover`, etc. The executor reflects on the
   type, deserialises params, and triggers the matching `Event` for
   domain observers to handle. Returns `command_accepted` (with a
   request id) immediately; commands that report a result also record
   a pollable outcome — see *Command results* below.

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

### Command results — `QueryCommandResult`

Every accepted command returns a `command_id` (the request id). Most
commands are fire-and-forget and record nothing. A command whose
observer returns `Result<Ack, String>` (see AGENTS.md § 4.2) records a
**terminal outcome** under that id, which the caller polls:

```bash
# 1. dispatch → get the request id
curl -s :4101/api/commands -d '{"command":"RunPython","params":{"code":"print(2+2)"}}'
# → {"command_id": 7}

# 2. poll the outcome by id
curl -s :4101/api/commands -d '{"type":"QueryCommandResult","id":"7"}'
# → {"data":{"id":7,"outcome":{"Succeeded":{"assigned":{"stdout":"4\n"}}}}}
```

`outcome` is one of `Succeeded(Ack)` (ran OK — `Ack.assigned` carries any
returned data), `Failed(msg)` (ran and errored — do **not** revert),
`Rejected(Reject)` (never ran — revert an optimistic edit), `Pending`
(accepted, async not yet done), or `null` (unknown id / fire-and-forget).

The store (`CommandResults` in `lunco-core`) is bounded (FIFO-evicted),
so poll reasonably soon after dispatch. The model is deliberately minimal
— one result + a few states (F′/MAVLink/behaviour-tree style), not XTCE's
multi-stage verifier pipeline. Long-running lifecycles (queued, progress,
cancel) live as per-domain state (e.g. experiments' `RunStatus`), not here.

### TBD: grouped self-submitting command registration

Today each `#[on_command]` generates an internal `__register_*` helper
and the owning plugin enumerates its observers in a
`register_commands!(...)` list (bare idents or `module::fn` paths). The
list is hand-maintained and can drift — a forgotten entry silently
omits the command from the API surface.

**Proposed (Option C):** each `#[on_command(Cmd, group = "x")]`
self-submits its registration thunk into an `inventory` collection
(already a transitive dep via `bevy_reflect`; verified to work on
`wasm32-unknown-unknown` under wasm-bindgen). A plugin then registers
its commands with one call — `register_group(app, "x")` — and **no
list is maintained anywhere**.

The `group` tag is load-bearing: a flat global `register_all` would
register every command in every *linked* crate regardless of which
plugins are added, which breaks feature-gating (a command would become
API-triggerable whenever its crate compiles, not when its plugin is
added) and reintroduces the missing-resource panic class. The group
namespaces registration to the owning plugin, preserving per-plugin
scoping while removing the list.

Scope of the change (orthogonal to dispatch/results — pure
registration polish):
- macro: parse `group =`, emit `inventory::submit!`;
- `lunco-core`: a `CommandReg { group, register: fn(&mut App) }` type +
  `inventory::collect!` + `register_group`, plus an idempotency guard
  (re-running a group must not double-`add_observer`);
- `inventory` becomes a direct dep of `lunco-core`;
- migration: add `group = "…"` to every `#[on_command]` and replace
  each `register_commands!` + `register_all_commands(app)` with one
  `register_group(app, "…")` (~all commands, ~10 crates).

Failure mode shifts from "forgot a list line" to "wrong/missing
`group`" — mitigable by making `group` a const/enum rather than a free
string. Decision pending; current state stays on the explicit-list
form.

### External API visibility (optional)

Domain crates can hide Reflect events from the external API surface
without un-registering them, via the [`ApiVisibility`] resource (see
`crates/lunco-api/src/queries.rs`). Names pushed into
`hidden_commands` are filtered out of `discover_schema` and rejected
by `execute_command` with `CommandNotFound`. The events remain in the
Bevy type registry — GUI panels, tests, and observers dispatch them
unaffected.

No domain currently uses this; it's available for future surfaces
that want a runtime-toggleable opt-out.

## Binaries with API Support

| Binary | Flag | Default Port |
|---|---|---|
| `sandbox` | `--api [PORT]` | 4101 |
| `lunica` | `--api [PORT]` | 4101 |
| `model_viewer` | `--api [PORT]` | 4101 |

## Troubleshooting

| Issue | Solution |
|---|---|
| Connection refused | Make sure sim was started with `--api` flag |
| "Command not found" | Check `/api/commands/schema` for available commands |
| "Entity not found" | Check `/api/entities` for valid ULID strings |
| `lunco_api` not found in `Cargo.toml` | Add `lunco-api = { path = "../lunco-api" }` dependency |
