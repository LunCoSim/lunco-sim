# lunco-api

Bevy ECS runtime for typed commands, queries, discovery, and telemetry. The
lightweight in-process contracts are in `lunco-api-core`; `lunco-api-codec`
converts values only at JSON wire boundaries, and outward transports are in
`lunco-api-transport`.

## Architecture

```
┌────────────────────────────────────────────────────────────┐
│  lunco-api-transport                                     │
│  HTTP / browser adapters                                  │
└────────────────────┬───────────────────────────────────────┘
                     │ JSON at the wire boundary
                     ▼
┌────────────────────────────────────────────────────────────┐
│  lunco-api-codec                                           │
│  JSON ↔ typed lunco-api-core values                        │
└────────────────────┬───────────────────────────────────────┘
                     │ external representation
                     ▼
┌────────────────────────────────────────────────────────────┐
│  lunco-api-core                                             │
│  ApiRequest · ApiResponse · ApiValue · schemas              │
└────────────────────┬───────────────────────────────────────┘
                     │ typed requests and responses
                     ▼
┌────────────────────────────────────────────────────────────┐
│  lunco-api                                                 │
│  ApiEntityRegistry · executor · discovery · telemetry      │
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
- **Typed inside the app**: ECS callers exchange `ApiValue`; JSON is confined to `lunco-api-codec` at wire boundaries.
- **Small contracts**: Consumers that only need typed request/response/value contracts depend on `lunco-api-core`, not the Bevy runtime.
- **Headless-compatible**: Runs without GPU/graphics. Perfect for server deployments.

## Commands

Commands are discovered automatically. The API scans `AppTypeRegistry` for reflected events carrying the marker emitted by `#[Command]`. A command must still be registered by its owning plugin so its observer and reflected type exist in the running host.

### HTTP Endpoint

The endpoint is supplied by `lunco-api-transport`. Pure JSON wire envelopes
live in `lunco-api-contracts`; `lunco-api-codec` translates them to/from the
typed contracts in `lunco-api-core`. This package owns ECS execution and
contains no JSON value conversion.

```
POST /api/commands
Content-Type: application/json

{
  "type": "ExecuteCommand",
  "command": "SetPorts",
  "params": {
    "target": 42,
    "producer_id": 4812,
    "writes": [["throttle", 0.8], ["steer", 0.0], ["brake", 0.0]],
    "seq": 0,
    "tick": 0
  }
}
```

For coordinated mechanisms spanning multiple entities, `SetPortsBatch` accepts
per-target write groups and validates the full command before admitting it:

```json
{
  "type": "ExecuteCommand",
  "command": "SetPortsBatch",
  "params": {
    "producer_id": 4812,
    "batches": [
      {"target": 42, "writes": [["angle", 0.8]]},
      {"target": 43, "writes": [["angle", -0.8]]}
    ]
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

`ListEntities` and `QueryEntity` expose a human-readable `name` resolved from
authored `ui:displayName`, catalog identity, or the `Name` leaf. The stable
`api_id` remains the machine identity; `QueryEntity.usd_prim_path` is the full
USD address when a client needs canonical scene resolution.

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
  "queries": ["GetBrokenConnections", "GetReadiness", "ListPorts", "Nearest", "ReadActuatorStatus", "ReadExposures", "ReadModelicaStepSamples", "ReadPorts", "ReadPortsBatch"]
}
```

`ReadPorts` and `ReadPortsBatch` require explicit `port_names`. Use `ListPorts`
to discover names, then request only the values needed. Batch targets each carry
an `api_id` and `port_names`; the response preserves target order and returns
all selected rows with one shared `sim_tick`. A batch is limited to 16 entities
and 128 returned port rows. `ReadActuatorStatus` compares explicitly named writable input,
measured output, and measured rate ports. It requires compatible value/frame
metadata and a resolved rate unit equal to the measured value's unit per second.
Its `settled` predicate requires both position error and measured rate to be
within their caller-supplied tolerances. `motion_state` reports `moving` or
`stationary` from measured rate. The authored Rhai prelude exposes these as
`read_ports_batch()` and `actuator_status()` while keeping target selection and
waiting policy in Rhai.

`ReadModelicaStepSamples` returns selected inputs and outputs from one accepted
solver step, with its input and output simulation times and session/step
identity. A `sample: ()` response means that the participant has not accepted a
step in its current session. This provenance distinguishes newly connected
values from the exact outputs produced by an accepted input vector. Rhai exposes
the batch through `read_modelica_step_samples(targets)`.

Port rows report `value: f64` only when the owner has a live sample; `()` means
the port is declared but currently unavailable. Unit definitions, unit identity,
coordinate frame, bounds, owner, authority, and writability come from that
owner's metadata contract.
Port reads encode a unit as `()` when absent, or `{ id, definition }` when
present. A unit definition is `()` for an unresolved identity; otherwise it
contains the symbol, seven SI dimension exponents ordered length, mass, time,
current, temperature, amount, luminous intensity, the scale and offset to SI,
and scale exactness. `frame` is the stable frame identity string or `()`.
`GetBrokenConnections` includes the port owner's rejection in each `failure`
field, so a range or writability fault remains distinguishable from a missing
port without parsing a log message.

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
use lunco_api_transport::LunCoApiPlugin;

app.add_plugins(LunCoApiPlugin::default());
// HTTP server starts on port 4101
```

With custom config:

```rust
use lunco_api_transport::{LunCoApiConfig, LunCoApiPlugin, transports::HttpServerConfig};

app.add_plugins(LunCoApiPlugin::new(LunCoApiConfig {
    http_config: Some(HttpServerConfig { port: 8080 }),
}));
```

The requested loopback port is claimed while the host application is being
built. If another process already owns it, the host exits before starting its
window or simulation loop and reports the port-binding error.

## Package boundary

`lunco-api` has no transport features and can be used by headless domain crates
without Axum, Tokio networking, or browser bindings. Add `lunco-api-transport`
only to an application root that exposes HTTP or the browser bridge:

```toml
lunco-api = { path = "../lunco-api" }
lunco-api-transport = { path = "../lunco-api-transport" }
```

The transport package's `transport-http` feature enables the native listener;
its wasm bridge is selected automatically by the target.

## Entity IDs

The API addresses entities by **numeric** `GlobalEntityId` (a `u64`, defined in
`lunco-core`). The `ApiEntityRegistry` resource maintains a
bidirectional `GlobalEntityId ↔ Bevy Entity` map. Component lifecycle observers
update it immediately on identity insertion, replacement, and removal.
Entity fields in command params use the global entity ID returned by the API:

```json
{ "target": 42 }
```

(`ListEntities` reports the same IDs, so a client reads one from a response and
passes it back as a command parameter. The transport codec preserves values
outside the signed in-process integer range as decimal text.)
