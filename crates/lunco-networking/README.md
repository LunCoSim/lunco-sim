# lunco-networking

Networking layer for LunCoSim — the transparent bridge between simulation state and wire protocols.

**Domain crates never import this crate.** They declare `app.replicate::<MyComponent>()` and the networking layer handles wire format, compression, and protocol translation silently.

---

## Table of Contents

- [Architecture Overview](#architecture-overview)
- [Transport Abstraction](#transport-abstraction)
- [Authentication & Authorization](#authentication--authorization)
- [Two Event Types: CommandMessage vs AuthorizedCommand](#two-event-types-commandmessage-vs-authorizedcommand)
- [ECS Replication Model](#ecs-replication-model)
- [Authority & Possession](#authority--possession)
- [Collaborative Editing](#collaborative-editing)
- [Edit History & Networked Undo](#edit-history--networked-undo)
- [Yjs for Modelica Code Collaboration](#yjs-for-modelica-code-collaboration)
- [Dynamic USD Support](#dynamic-usd-support)
- [Client-Side Prediction](#client-side-prediction)
- [Compression Stack](#compression-stack)
- [Space-Standards Compatibility](#space-standards-compatibility)
- [Interest Management](#interest-management)
- [Entity Identity Mapping](#entity-identity-mapping)
- [What Domain Code Sees](#what-domain-code-sees)
- [Existing Solutions Evaluated](#existing-solutions-evaluated)
- [Implementation Phases](#implementation-phases)
- [Bandwidth Budget](#bandwidth-budget)
- [Cargo Feature Matrix](#cargo-feature-matrix)
- [References](#references)

---

## Architecture Overview

LunCoSim networking follows the same layered model as Godot's `MultiplayerAPI` and NASA's VIPER rover hybrid architecture:

```
┌─── Domain Code (lunco-mobility, lunco-celestial, lunco-obc) ─────┐
│  DigitalPort(i16), PhysicalPort(f32), DVec3, CommandMessage       │
│  ← NO networking types anywhere                                   │
└──────────────────────┬────────────────────────────────────────────┘
                       │  lunco-networking (transparent shim)
                       │
    ┌──────────────────┼──────────────────┐
    │                  │                  │
    ▼                  ▼                  ▼
┌─────────┐    ┌─────────────┐    ┌─────────────┐
│ Internal │    │ CCSDS /     │    │ DDS /       │
│ Game     │    │ YAMCS       │    │ ROS2        │
│ Protocol │    │ Bridge      │    │ Bridge      │
│          │    │             │    │             │
│ renet2 + │    │ CCSDS pkts  │    │ DDS topics  │
│ replicon │    │ XTCE XML    │    │ SpaceROS    │
│ compress │    │ PUS headers │    │ nodes       │
└────┬─────┘    └──────┬──────┘    └──────┬──────┘
     │                 │                  │
     ▼                 ▼                  ▼
 LunCo clients   YAMCS mission    ROS2 nav/
 (web/native)    control system   perception
```

### Four-Layer Plugin Architecture (Extended)

```
Layer 4: UIPlugins            — bevy_workbench, lunco-ui, domain ui/panels
Layer 3: SimulationPlugins    — Rendering, Cameras, Lighting, 3D viewport, Gizmos
Layer 2: DomainPlugins        — Celestial, Avatar, Mobility, Robotics, OBC, FSW
Layer 2b: NetworkingPlugin    — lunco-networking (transport, replication, auth, bridges)
Layer 1: SimCore              — MinimalPlugins, ScheduleRunner, big_space, Avian3D
```

Networking is a **Layer 2b** domain plugin — self-contained, headless-compatible, removable without affecting simulation correctness.

### Layered Auth Model

```
┌─── Domain Systems ───────────────────────────────────────┐
│  Observers react to CommandMessage (local)               │
│  Observers react to AuthorizedCommand (remote, verified) │
└───────────────────┬──────────────────────────────────────┘
                    │
    ┌───────────────┼────────────────┐
    ▼               ▼                ▼
┌─────────┐  ┌──────────┐   ┌──────────────┐
│ Proven- │  │ Auth     │   │ Transport    │
│ ance    │  │ Layer    │   │ Layer        │
│ Inject  │  │          │   │ (renet2)     │
└─────────┘  └──────────┘   └──────────────┘
```

**Key principle**: `CommandMessage` stays pure. It never carries origin. Local systems trigger it directly. Remote commands arrive as raw bytes, get auth-verified, then get wrapped in `AuthorizedCommand` at the ECS boundary.

---

## Transport Abstraction

### Compile-Time Selection (Godot Pattern)

Transport is selected via Cargo features — zero runtime overhead, swap without code changes:

```toml
[features]
transport-udp  = ["bevy_renet2/netcode"]         # Desktop: native UDP (default)
transport-ws   = ["bevy_renet2/ws_client"]       # Browser: WebSockets (WASM)
transport-wt   = ["bevy_renet2/webtransport"]    # Future: WebTransport
transport-server = ["bevy_renet2/netcode"]       # Dedicated server mode
```

```rust
// In LunCoNetworkingPlugin::build()
#[cfg(feature = "transport-udp")]
app.add_plugins(RenetPlugin::<NetcodeTransport>::default());

#[cfg(feature = "transport-ws")]
app.add_plugins(RenetPlugin::<WebSocketTransport>::default());
```

### Why renet2 + bevy_replicon

| Crate | Role | Why Chosen |
|---|---|---|
| **renet2** | Transport abstraction + channels | UDP, WebSockets, WebTransport, Steam. Modular, actively maintained. Bevy 0.18 compatible. |
| **bevy_replicon** | ECS entity/component replication | Most popular Bevy networking crate. Automatic component diffing. Transport-agnostic. |
| **bevy_replicon_renet2** | Integration of both | Renet2 as backend for replicon. Seamless. |

### Rejected Alternatives

| Alternative | Why Rejected |
|---|---|
| **bevy_networking_turbulence** | Archived since 2022, outdated WebRTC |
| **bevy_eventwork** | Niche, less mature, fewer features |
| **Custom protocol** | Years of work to match renet2's reliability, fragmentation, encryption |

---

## Authentication & Authorization

### The Problem

The transport layer (renet2) knows **which connection** sent a message (a numeric handle). But domain systems need to know **who** sent it, **what they're allowed to do**, and this identity must be **cryptographically verifiable** — not a client-provided field that can be forged.

### The Architecture

```
┌─────────────────────────────────────────────────────────┐
│  Transport Layer (renet2)                               │
│  "Message came from connection handle #47"              │
│  → Provides: connection_id (opaque handle)              │
│  → Doesn't know: identity, roles, permissions           │
└──────────────┬──────────────────────────────────────────┘
               │
               ▼
┌─────────────────────────────────────────────────────────┐
│  Auth Layer                                             │
│  "Connection #47 = session 'abc123', role: Operator"    │
│  → Maps connection_id → verified Session               │
│  → Validates: can this session send this command?       │
│  → Rejects: unauthorized, expired, revoked sessions     │
└──────────────┬──────────────────────────────────────────┘
               │
               ▼
┌─────────────────────────────────────────────────────────┐
│  Provenance Injection                                   │
│  Wraps the command with verified authorship            │
│  → AuthorizedCommand { session_id, command }            │
│  → This is a DIFFERENT event type from CommandMessage   │
└──────────────┬──────────────────────────────────────────┘
               │
               ▼
┌─────────────────────────────────────────────────────────┐
│  Domain Systems (edit log, physics, FSW, etc.)          │
│  → Listen to AuthorizedCommand for attributed actions   │
│  → Listen to CommandMessage for local-only actions      │
│  → Neither needs to know WHERE the command came from    │
└─────────────────────────────────────────────────────────┘
```

### Session

```rust
/// A verified connection with identity and permissions.
/// Created on successful authentication.
/// Destroyed on disconnect or timeout.
#[derive(Clone, Debug)]
pub struct Session {
    pub id: SessionId,              // Cryptographically random
    pub connection_id: u64,         // Maps to renet2's client_id
    pub identity: Identity,         // Who you are (verified)
    pub roles: HashSet<Role>,       // What you can do
    pub connected_at: Instant,
    pub last_activity: Instant,
}

/// Proven identity, verified at connect time.
pub enum Identity {
    /// Shared secret token (simple, for local dev)
    Token(String),
    /// Public key (Ed25519 signature verified)
    PublicKey([u8; 32]),
    /// Certificate chain (for production with CA)
    Certificate(Vec<u8>),
}

#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Role {
    /// Can observe state but send NO commands
    Observer,
    /// Can possess vessels, drive, edit scene
    Operator,
    /// Can edit Modelica code, run simulations
    ModelicaEngineer,
    /// Can configure FSW parameters, upload command sequences
    FswEngineer,
    /// Can manage sessions, kick users, change sim settings
    Admin,
}
```

### ACL (Access Control Lists)

```rust
// Role → allowed command names (wildcard "*" = all)
let mut acl = HashMap::new();

acl.insert(Role::Observer, HashSet::new()); // Nothing

acl.insert(Role::Operator, HashSet::from([
    "POSSESS", "RELEASE",
    "DRIVE_ROVER", "BRAKE_ROVER",
    "SPAWN_ENTITY:*",          // Wildcard for all spawn types
    "TRANSFORM_CHANGED",
    "PARAMETER_CHANGED",
    "WIRE_CONNECTED",
    "UNDO",
]));

acl.insert(Role::Admin, HashSet::from(["*"]));  // Everything
```

### The Auth Registry

```rust
#[derive(Resource)]
pub struct AuthRegistry {
    /// Active sessions indexed by connection_id (from transport)
    by_connection: HashMap<u64, SessionId>,
    /// Sessions indexed by ID
    sessions: HashMap<SessionId, Session>,
    /// Session secrets (HMAC keys, one per session)
    secrets: HashMap<SessionId, HmacKey>,
    /// Role → allowed command names
    command_acl: HashMap<Role, HashSet<String>>,
}

impl AuthRegistry {
    /// Called when a new connection authenticates.
    pub fn authenticate(
        &mut self,
        connection_id: u64,
        credentials: AuthCredentials,
    ) -> Result<SessionToken, AuthError>;

    /// Called for every incoming message. Returns None if unauthorized.
    pub fn resolve(&mut self, connection_id: u64) -> Option<&Session>;

    /// Can this session send a command with the given name?
    pub fn can_send(&self, session_id: &SessionId, command_name: &str) -> bool;

    /// Called on disconnect
    pub fn disconnect(&mut self, connection_id: u64);
}
```

---

## Two Event Types: CommandMessage vs AuthorizedCommand

This is the key architectural distinction. **`CommandMessage` stays clean.** It never carries origin information.

```rust
/// Local command — no provenance needed.
/// Generated by local UI input, simulation systems, timers.
#[derive(Event, Debug, Clone)]
pub struct CommandMessage {
    pub id: u64,
    pub target: Entity,
    pub name: String,
    pub args: SmallVec<[f64; 4]>,
    pub source: Entity,
    // NO origin field. NO client_id. Pure data.
}

/// Networked command — authorship verified by auth layer.
/// Only created by the networking plugin when injecting remote messages.
#[derive(Event, Debug)]
pub struct AuthorizedCommand {
    pub session_id: SessionId,  // Verified identity
    pub timestamp: Instant,     // Server time of receipt
    pub command: CommandMessage,
}
```

### How They Flow

```
LOCAL USER clicks "spawn rover"
  → CommandMessage { name: "SPAWN_ENTITY:skid_rover", ... }
  → commands.trigger(cmd)  ← EventReader<CommandMessage> catches it
  → Observer runs, entity spawns
  → No edit history attribution needed (local action)

REMOTE USER clicks "spawn rover"
  → Client serializes: NetworkCommand { command_id, target_global_id, args }
  → renet2 sends bytes → server receives from connection_id #47
  → Auth layer: resolve(#47) → Session { id: "abc123", roles: [Operator] }
  → ACL check: can "abc123" send "SPAWN_ENTITY"? YES
  → Entity resolver: target_global_id → local Entity
  → commands.trigger(AuthorizedCommand {
        session_id: "abc123",
        timestamp: Instant::now(),
        command: CommandMessage { name: "SPAWN_ENTITY:skid_rover", ... },
    })
  → EventReader<AuthorizedCommand> catches it
  → Observer runs, entity spawns
  → EditLog records: "Session abc123 spawned skid_rover" (verified attribution)
```

**Domain observers can listen to either or both:**

```rust
fn on_spawn_command(
    mut local: EventReader<CommandMessage>,      // Local spawns
    mut remote: EventReader<AuthorizedCommand>,  // Remote spawns (with verified authorship)
    mut commands: Commands,
    catalog: Res<SpawnCatalog>,
    mut edit_log: ResMut<EditLog>,
    lamport: ResMut<LamportClock>,
) {
    for cmd in local.read() {
        // ... spawn ...
    }
    for auth_cmd in remote.read() {
        // ... spawn ...
        edit_log.push(EditEvent::Spawn {
            author: auth_cmd.session_id,  // Verified, can't forge
            // ...
        });
    }
}
```

### Network Injection Point (Server Side)

```rust
/// Server: receives raw bytes, outputs AuthorizedCommand events.
fn process_incoming_commands(
    mut renet_server: ResMut<RenetServer>,
    mut commands: Commands,
    mut auth: ResMut<AuthRegistry>,
    entity_resolver: Res<EntityResolver>,
    cmd_dict: Res<CommandDictionary>,
) {
    for connection_id in renet_server.clients_id() {
        let Some(session) = auth.resolve(connection_id) else {
            renet_server.disconnect(connection_id);
            continue;
        };

        while let Some(msg) = renet_server.receive_message(CHANNEL_CMD, connection_id) {
            let net_cmd: NetworkCommand = bincode::deserialize(&msg).unwrap();
            let cmd_name = cmd_dict.name_from_id(net_cmd.command_id);

            // Authorization: can this session send this command?
            if !auth.can_send(&session.id, &cmd_name) {
                warn!("Session {} denied permission for command {}", session.id, cmd_name);
                continue;
            }

            // Resolve global ID → local Entity
            let target = entity_resolver.resolve(net_cmd.target_global_id).unwrap();

            // Inject as AuthorizedCommand — provenance attached HERE,
            // at the boundary between network and ECS world.
            commands.trigger(AuthorizedCommand {
                session_id: session.id,
                timestamp: Instant::now(),
                command: CommandMessage {
                    id: generate_command_id(),
                    target,
                    name: cmd_name,
                    args: net_cmd.args.iter().map(|&f| f as f64).collect(),
                    source: Entity::PLACEHOLDER, // Not meaningful for remote commands
                },
            });
        }
    }
}
```

---

## ECS Replication Model

### How It Works

```rust
// Domain code — no networking awareness:
#[derive(Component, Clone, Copy, Reflect)]
struct RoverMobilityState {
    position: DVec3,
    velocity: DVec3,
    wheel_speed: f32,
    brake_applied: bool,
}

// Networking plugin — registers custom serializer transparently:
app.replicate::<RoverMobilityState>()
    .set_serializer::<RoverMobilityStateSerializer>();
```

Replicon diffs components every tick, sends only changes. The serializer converts `DVec3` → quantized `u16×3`, booleans → bit-packed flags, etc. Domain code is oblivious.

### Replication Direction

| Component | Direction | Notes |
|---|---|---|
| `GlobalTransform` | Server → All | Physics-driven, authoritative |
| `DigitalPort` | Server → All | FSW register state visible to all |
| `PhysicalPort` | Server → All | Actuator/sensor values |
| `RoverMobilityState` | Server → All | Wheel angles, speeds, brake state |
| `CelestialBody` | Server → All | Ephemeris = shared ground truth |
| `NetworkAuthority` | Server → All | Who controls what |
| `GlobalEntityId` | Server → All | Stable identity for all clients |
| Avatar camera | Local only | **Never replicated** — per-client input device |

### Domain-Owned Replication (No Central Registry)

Replication declarations live in each domain crate, not in a central registry. This mirrors the UI plugin pattern:

```
lunco-mobility/src/replication.rs  → LunCoMobilityReplicationPlugin
lunco-fsw/src/replication.rs       → LunCoFswReplicationPlugin
lunco-celestial/src/replication.rs → LunCoCelestialReplicationPlugin
lunco-hardware/src/replication.rs  → LunCoHardwareReplicationPlugin
```

Each domain declares what crosses the network:

```rust
// lunco-mobility/src/replication.rs — declares its own physical state
app.replicate::<RoverMobilityState>();
app.replicate::<DifferentialDrive>();
app.replicate::<WheelRaycast>();
app.replicate::<Suspension>();

// lunco-fsw/src/replication.rs
app.replicate::<DigitalPort>();
app.replicate::<PhysicalPort>();
app.replicate::<Wire>()
    .set_serialization(/* custom serializer: serialize scale, skip Entity fields */);
```

The binary wires it up for multiplayer:

```rust
app.add_plugins(LunCoMobilityPlugin)
   .add_plugins(LunCoFswPlugin)
   .add_plugins(LunCoNetworkingPlugin)              // transport + auth + EditLog
   .add_plugins(LunCoMobilityReplicationPlugin)      // mobility types
   .add_plugins(LunCoFswReplicationPlugin);          // fsw types
```

**Single-player:** replication plugins are not added. Zero networking footprint. `bevy_replicon` is an optional dependency, compiled out.

**Dependency direction:**

```
lunco-mobility  → lunco-networking (optional, feature: networking)
lunco-fsw       → lunco-networking (optional, feature: networking)
lunco-networking → lunco-core (for GlobalEntityId type only)

NO reverse dependencies.
NO aggregator crate.
```

### What Replicates vs What Stays Local

```
REPLICATED (declared in domain replication submodules):
  GlobalTransform          ← Position/orientation
  DigitalPort              ← FSW register state
  PhysicalPort             ← Engineering value
  RoverMobilityState       ← Wheel angles, speeds
  CelestialBody params     ← Orbital state
  MotorActuator            ← Torque being applied
  BrakeActuator            ← Brake pressure

NOT REPLICATED (never registered):
  Wire.source/target       ← Local join, rebuilt per-process
  FlightSoftware.port_map  ← Local port map, rebuilt per-process
  ControllerLink           ← Avatar + vessel always in same World
  PendingWheelWiring       ← Temporary during USD loading
  FrameBlend               ← Local camera animation
  Selected, GizmoPrevPos   ← Local editor state
  SpawnGhost               ← Local spawn preview
  Avatar cameras           ← Per-client input device
```

Component fields like `Wire.source` and `FlightSoftware.port_map` **are not changed**. They use `Entity` and stay as `Entity`. When processes split, each process creates its own Wire entities and port maps — the local join is rebuilt, not serialized. Per-field serialization is handled by custom serializers in domain replication submodules.

---

## Authority & Possession

### Current Single-User Model

```
Click rover → ControllerLink → VesselIntent → DRIVE_ROVER commands
```

### Multi-User Model (Networked)

```
Click rover → RequestAuthority → Server grants/denies → ControllerLink → local control
```

### NetworkAuthority Component

```rust
#[derive(Component)]
pub struct NetworkAuthority {
    pub owner_session: Option<SessionId>,  // Which session controls this
    pub pending_request: Option<SessionId>, // Session waiting for control
}
```

### Possession Negotiation Flow

```
Session A (Operator)                  Server                         Session B (Observer)
   │                                    │                                │
   │── RequestAuthority(rover1) ───────>│                                │
   │                                    │── GrantAuthority(rover1) ───>  │ (notify: A controls rover1)
   │<─ AuthorityGranted(rover1) ────────│                                │
   │                                    │                                │
   │ [Local control begins]            │                                │
   │ [DRIVE_ROVER → server]            │                                │
   │                                    │── Replicate(rover1 state) ───> │
   │                                    │                                │
   │── ReleaseAuthority(rover1) ──────> │                                │
   │                                    │── AuthorityReleased(rover1) ──>│ (notify: rover1 free)
```

### Command Flow Over Network

```
Client:
  User clicks rover → raycast → CommandMessage { "POSSESS", target: rover }
  → Serialize: NetworkCommand { command_id: POSSESS_ID, target_global_id, args }
  → renet2 send

Server:
  Receive from connection #47
  → Auth: resolve(#47) → session "abc123" (Operator role)
  → ACL: can Operator send POSSESS? YES
  → Resolve global_id → server Entity
  → commands.trigger(AuthorizedCommand { session_id: "abc123", command: ... })
  → Existing on_possess_command observer runs — zero changes needed
  → NetworkAuthority updated: owner_session = Some("abc123")
  → Replicated to all clients via replicon
```

---

## Collaborative Editing

### The Model: Event Sourcing

Every sandbox edit is recorded as a structured `EditEvent`. The `EditLog` is the append-only history — enough data to replay or reverse any operation.

```rust
#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum EditEvent {
    /// Entity was spawned (rover, prop, terrain, component)
    Spawn {
        entity_id: GlobalEntityId,
        op_id: u64,            // Lamport clock — defines ordering
        author: SessionId,     // Verified by auth layer
        timestamp_ms: u64,
        asset_path: String,     // "vessels/rovers/skid_rover.usda"
        transform: TransformData,
        parent_grid: GlobalEntityId,
    },

    /// Entity was deleted
    Delete {
        entity_id: GlobalEntityId,
        op_id: u64,
        author: SessionId,
        timestamp_ms: u64,
    },

    /// Transform was changed (gizmo drag)
    TransformChanged {
        entity_id: GlobalEntityId,
        op_id: u64,
        author: SessionId,
        timestamp_ms: u64,
        old_transform: TransformData,
        new_transform: TransformData,
    },

    /// Component parameter was changed (inspector panel)
    ParameterChanged {
        entity_id: GlobalEntityId,
        op_id: u64,
        author: SessionId,
        timestamp_ms: u64,
        component: String,     // "Suspension"
        field: String,         // "spring_constant"
        old_value: serde_json::Value,
        new_value: serde_json::Value,
    },

    /// Wire was connected (FSW port → physical port)
    WireConnected {
        wire_id: GlobalEntityId,
        op_id: u64,
        author: SessionId,
        timestamp_ms: u64,
        source_entity: GlobalEntityId,
        target_entity: GlobalEntityId,
        scale: f32,
    },

    /// Undo of a previous operation (networked undo)
    Undo {
        undone_op_id: u64,     // Which operation is being reversed
        op_id: u64,
        author: SessionId,
        timestamp_ms: u64,
    },

    /// Spawn catalog was modified (new entry added at runtime)
    CatalogEntryAdded {
        op_id: u64,
        author: SessionId,
        timestamp_ms: u64,
        entry: SpawnCatalogEntry,
    },
}
```

### Recording — Zero Changes to Existing Observers

The existing observers (`on_spawn_entity_command`, gizmo systems) don't change. A **recording system** watches their results:

```rust
/// Records EditEvents when entities are spawned/deleted/modified.
/// Runs on the SERVER only — server is the source of truth for history.
fn record_edit_events(
    mut commands: Commands,
    mut edit_log: ResMut<EditLog>,
    lamport: ResMut<LamportClock>,
    q_new: Query<(Entity, &GlobalEntityId), Added<SpawnedMarker>>,
    spawn_meta: Query<&SpawnMetadata>,
) {
    for (entity, global_id) in q_new.iter() {
        let meta = spawn_meta.get(entity).unwrap();
        edit_log.push(EditEvent::Spawn {
            entity_id: *global_id,
            op_id: lamport.tick(),
            author: meta.author_session_id,
            timestamp_ms: unix_millis(),
            asset_path: meta.asset_path.clone(),
            transform: TransformData::from_entity(&entity, &commands),
            parent_grid: meta.grid_id,
        });
        commands.entity(entity).insert(HistoryRecorded);
    }
}
```

### Lamport Clock for Ordering

```rust
#[derive(Resource)]
pub struct LamportClock {
    counter: u64,
}

impl LamportClock {
    pub fn tick(&mut self) -> u64 {
        self.counter += 1;
        self.counter
    }

    /// Merge with incoming remote timestamp — always advance
    pub fn receive(&mut self, remote: u64) -> u64 {
        self.counter = self.counter.max(remote) + 1;
        self.counter
    }
}
```

This guarantees that all `op_id` values are monotonically increasing across all clients, defining a total order for the edit history.

### Conflict Resolution

When two users edit the same entity simultaneously:

```
User A: moves Rover to position X (client clock = 10)
User B: moves Rover to position Y (client clock = 7)

Both commands arrive at server:
Server receives A's move → assigns op_id = 11
Server receives B's move → assigns op_id = 12

Server applies in op_id order:
  1. A's move (Rover → X)
  2. B's move (Rover → Y)  ← final state

Both clients receive both events in op_id order:
  Client A sees: my move applied, then B's move overwrote it
  Client B sees: A's move applied, then my move applied
  → Both converge to position Y
```

### Replication: State, Not Topology

```
┌─────────────────────────────────────────────────────────────┐
│  REPLICATE (sent over network)                               │
│                                                              │
│  DigitalPort.raw_value          ← FSW register state         │
│  PhysicalPort.value             ← Engineering value          │
│  Wire.scale                     ← Calibrator coefficient     │
│  FlightSoftware.brake_active    ← Boolean state              │
│  DifferentialDrive config       ← Steering parameters        │
│  Suspension settings            ← Spring/damper constants    │
│  WheelRaycast hit data          ← Ground contact info        │
│  GlobalTransform                ← Position/orientation       │
│  RoverVessel / Vessel markers   ← Entity type                │
│  CelestialBody params           ← Orbital state              │
│  MotorActuator state            ← Torque being applied       │
│  BrakeActuator state            ← Brake pressure             │
│                                                              │
├─────────────────────────────────────────────────────────────┤
│  DON'T REPLICATE (reconstructed locally from USD)            │
│                                                              │
│  Wire.source/target Entity      ← Same USD on both sides    │
│  FlightSoftware.port_map        ← Same USD on both sides    │
│  PendingWheelWiring             ← Rebuilt during USD load   │
│  ModelicaModel state            ← Runs server-side only     │
│  FrameBlend animation           ← Local camera effect       │
│  Avatar camera components       ← Per-client input device   │
│  Selected / GizmoPrevPos        ← Local editor state        │
│  SpawnGhost                     ← Local spawn preview       │
│  TerrainHeightmap / SurfaceMesh ← Loaded from asset         │
│  GravityModel / Propagators     ← Loaded from asset/config  │
└─────────────────────────────────────────────────────────────┘
```

---

## Edit History & Networked Undo

### The Edit Log

```rust
#[derive(Resource)]
pub struct EditLog {
    events: Vec<EditEvent>,
    /// Checkpoint for fast replay — full state snapshot at this point
    last_checkpoint: Option<Checkpoint>,
}

impl EditLog {
    /// Replay all events from start (or checkpoint) to reconstruct state.
    /// Used when a new client joins — they get the full history.
    pub fn replay_from_start(&self, world: &mut World) {
        let events = self.last_checkpoint
            .as_ref()
            .map(|cp| &self.events[cp.event_index..])
            .unwrap_or(&self.events);
        for event in events {
            self.apply_event(world, event);
        }
    }

    /// Reverse an event (undo).
    pub fn reverse_event(&self, world: &mut World, op_id: u64) {
        let event = self.events.iter().find(|e| e.op_id() == op_id).unwrap();
        match event {
            EditEvent::Spawn { entity_id, .. } => {
                despawn_by_global_id(world, *entity_id);
            }
            EditEvent::Delete { entity_id, asset_path, transform, .. } => {
                respawn_entity(world, *entity_id, asset_path, transform);
            }
            EditEvent::TransformChanged { entity_id, old_transform, .. } => {
                set_transform(world, *entity_id, *old_transform);
            }
            EditEvent::ParameterChanged { entity_id, component, field, old_value, .. } => {
                set_parameter(world, *entity_id, component, field, old_value);
            }
            _ => {}
        }
    }
}
```

### Networked Undo

Current undo is local-only (Ctrl+Z → `UndoStack::pop()` → despawn). For networking:

```
User A presses Ctrl+Z
  → Client sends UNDO command (referencing the last op_id they authored)
  → Server receives UNDO, auth verifies session A
  → Server reverses the operation
  → Server broadcasts the reversal as EditEvent::Undo
  → All clients (including User A) apply the reversal
```

```rust
fn handle_networked_undo(
    mut edit_log: ResMut<EditLog>,
    lamport: ResMut<LamportClock>,
    incoming_undos: EventReader<AuthorizedCommand>,
) {
    for auth_cmd in incoming_undos.read() {
        let op_to_reverse = auth_cmd.command.args[0] as u64;
        edit_log.reverse_event(&mut commands, op_to_reverse);

        // Broadcast the reversal to all clients as a notification
        broadcast_notification(EditEvent::Undo {
            undone_op_id: op_to_reverse,
            op_id: lamport.tick(),
            author: auth_cmd.session_id,
            timestamp_ms: unix_millis(),
        });
    }
}
```

### Edit History Timeline Panel

The UI can display a full audit trail:

```
┌─ Edit History ──────────────────────────────────┐
│ 14:32:01  Alice    Spawned "Skid Rover"          │
│ 14:32:15  Bob      Moved "Skid Rover" (gizmo)    │
│ 14:32:42  Alice    Changed Suspension.k_damp     │
│ 14:33:01  Bob      Spawned "Solar Panel"          │
│ 14:33:10  Alice    Undid "Moved Skid Rover"       │
│ 14:33:45  System   USD file reloaded: scene.usda  │
└──────────────────────────────────────────────────┘
```

---

## Yjs for Modelica Code Collaboration

### Why Yjs

Modelica `.mo` files are **text**. Multiple users editing the same file simultaneously requires a **CRDT** (Conflict-free Replicated Data Type) to guarantee merge consistency. Regular last-write-wins doesn't work for concurrent text edits.

Yjs solves this deterministically:

```
User A types "parameter Real mass = 100;" at position 0
User B types "// Author: Bob\n" at position 0 (same time)

Yjs CRDT merges both:
  "// Author: Bob\nparameter Real mass = 100;"

No conflict. No merge dialog. Deterministic convergence.
```

### Architecture

```
┌── Modelica Workbench Client A ───────────────┐
│  Code Editor Panel                            │
│  ┌─────────────────────────────────────────┐ │
│  │ parameter Real mass = 100;  ← cursor A  │ │
│  └─────────────────────────────────────────┘ │
│                                               │
│  yrs::Doc ("modelica_file_123")              │
│  └ Text: "// content"                        │
│     └ User A types → Update binary ──────────┼──┐
└──────────────────────────────────────────────┘  │
                                                  │ renet2
                                                  │ CHANNEL_YJS_UPDATE
                                                  │
┌── Modelica Workbench Client B ───────────────┐  │
│  Code Editor Panel                            │  │
│  ┌─────────────────────────────────────────┐ │  │
│  │ // Author: Bob               ← cursor B  │ │  │
│  └─────────────────────────────────────────┘ │  │
│                                               │  │
│  yrs::Doc ("modelica_file_123")              │  │
│  └ Text: "// content"                        │  │
│     └ User B types → Update binary ──────────┼──┼──┐
└──────────────────────────────────────────────┘  │  │
                                                  │  │
                                     ┌────────────┼──┼──┐
                                     │  Server    │  │  │
                                     │ Broadcasts │  │  │
                                     │ updates to │  │  │
                                     │ all peers  ←┼──┘  │
                                     │            ←──────┘
                                     └────────────────────┘
```

### Implementation

```rust
use yrs::{Doc, Text, Transact, Update, UndoManager};

/// Shared collaborative document for a Modelica file.
pub struct CollaborativeModelicaDoc {
    doc: Doc,
    text: Text,
    undo_manager: UndoManager,
}

impl CollaborativeModelicaDoc {
    pub fn new(file_id: &str) -> Self {
        let doc = Doc::with_guid(file_id);
        let text = doc.get_or_insert_text("content");
        let undo_manager = UndoManager::with_scope(&doc, &text);
        Self { doc, text, undo_manager }
    }

    /// Called when update arrives from network.
    pub fn apply_remote_update(&mut self, data: &[u8]) {
        let update = Update::decode_v1(data).unwrap();
        let mut txn = self.doc.transact_mut();
        txn.apply_update(update);
        // Editor UI updates from doc.text().get_string(&txn)
    }
}

/// Network message for Yjs updates.
#[derive(Serialize, Deserialize)]
pub struct YjsUpdate {
    pub file_id: String,   // Which Modelica file
    pub update: Vec<u8>,   // Yjs binary update
    pub author: SessionId,
}
```

### Collaborative Cursors

Yjs awareness protocol shows where other users' cursors are:

```rust
// In the code editor UI panel:
fn render_collaborative_cursors(awareness: Res<YjsAwareness>, mut ui: egui::Ui) {
    for (session_id, cursor_state) in awareness.clients() {
        if cursor_state.file_id == current_file_id {
            draw_cursor_indicator(cursor_state.position, session_color(session_id));
            draw_user_label(session_name(session_id));
        }
    }
}
```

---

## Dynamic USD Support

### File Watching → Broadcast

```rust
/// Watches USD files for changes and triggers reload.
fn watch_usd_files(
    mut watcher: ResMut<UsdFileWatcher>,
    mut commands: Commands,
    usd_entities: Query<(Entity, &UsdPrimPath, &GlobalEntityId)>,
) {
    for changed_path in watcher.poll() {
        // Find all entities loaded from this USD file
        for (entity, usd_path, global_id) in usd_entities.iter() {
            if usd_path.path.contains(&changed_path) {
                commands.trigger(CommandMessage {
                    name: "RELOAD_USD_FILE".into(),
                    target: entity,
                    args: smallvec![],
                    source: Entity::PLACEHOLDER,
                });
            }
        }
    }
}
```

### RELOAD_USD Command Flow

```
Server detects USD file change:
  1. Record "delete all entities from this file" as EditEvents
  2. Actually delete them
  3. Reload USD scene (new entities get new GlobalEntityIds)
  4. Record "spawn all new entities" as EditEvents
  5. Broadcast all EditEvents to clients

Clients receive EditEvents:
  1. Apply deletes (entities vanish)
  2. Apply spawns (new entities appear from shared asset library)
  → Scene converges to match server's reloaded USD
```

### Runtime Catalog Modification

When a user imports a new USD file or adds a custom rover:

```rust
/// Add a new entry to the spawn catalog at runtime.
/// This is itself an EditEvent — it gets recorded and broadcast.
fn handle_add_catalog_entry(
    mut catalog: ResMut<SpawnCatalog>,
    mut edit_log: ResMut<EditLog>,
    lamport: ResMut<LamportClock>,
    entry: SpawnCatalogEntry,
    author: Res<CurrentEditAuthor>,
) {
    catalog.add(entry.clone());

    edit_log.push(EditEvent::CatalogEntryAdded {
        op_id: lamport.tick(),
        author: author.session_id,
        timestamp_ms: unix_millis(),
        entry: entry.clone(),
    });

    // Broadcast to all clients so they also add it to their catalogs
    broadcast_catalog_entry(entry);
}
```

---

## Client-Side Prediction

### What Needs Prediction

| Scenario | Dynamics | Latency tolerance | Needs prediction? |
|---|---|---|---|
| Rover driving on terrain | Fast (meters/sec) | ~50ms noticeable | **Yes** |
| Spacecraft orbital maneuver | Slow (cm/s in orbit) | ~500ms acceptable | No |
| Camera free-flight | Instant (teleport) | N/A | No (local only) |
| Sandbox object placement | Static | ~200ms acceptable | No (ghost preview) |

### Prediction Architecture

```
Client Frame N:
  1. IMMEDIATE: Apply input to LOCAL physics copy → rover moves NOW
  2. SEND: Input to server with sequence number
  3. STORE: Unconfirmed inputs in pending queue

Server Frame N+4 (after RTT):
  Receives input seq:42, applies to authoritative simulation
  Broadcasts snapshot: { rover_pos, rover_vel, confirmed_seq: 42 }

Client Frame N+8 (reconciliation):
  Receives server state: rover at X=100.5
  Client predicted: rover at X=100.3
  → Snap to server position, replay unconfirmed inputs
  → Small visual correction (blend, not teleport)
```

### Why Rover Prediction Is Simple

- **Max speed ~0.5 m/s** — prediction error accumulates slowly
- **80ms RTT at 0.5 m/s = 4cm error** — below visual threshold
- **Terrain physics is deterministic** — server and client run same model
- **No interfering agents** — other rovers don't affect your prediction

### PredictedState Component

```rust
#[derive(Component)]
pub struct PredictedState {
    pub server_confirmed_seq: u32,
    pub pending_inputs: VecDeque<(u32, RoverInput)>,
    pub predicted_pos: DVec3,
    pub predicted_vel: DVec3,
}
```

---

## Compression Stack

Compression is **three layers**, each targeting specific data characteristics. High-level code sees none of it.

### Layer 0: Semantic Compression (Biggest savings — 5-10x)

| Technique | Before | After | Reduction |
|---|---|---|---|
| Position quantization | `DVec3` = 24 bytes | `u16×3` = 6 bytes | 4x |
| Quaternion compression | `f64×4` = 32 bytes | smallest-three = 8 bytes | 4x |
| Delta encoding | Full state every frame | Only changed fields | 17x (typical) |
| Dead reckoning | Position every frame | Position when velocity changes | 120x (rover at const speed) |
| Boolean bit-packing | 8 bools = 8 bytes | 8 bits = 1 byte | 8x |
| VarInt encoding | Entity ID = 8 bytes | Small IDs = 1-2 bytes | 4-8x |
| Command dictionary | `String("DRIVE_ROVER")` = 15 bytes | `u8` command_id = 1 byte | 15x |

### Layer 1: Protocol-Level Compression (2-3x)

- Variable-length integers for sequence numbers, entity IDs
- Bit-packed component masks (which components changed)
- Sparse field omission (don't send what client already knows)
- Command name dictionary encoding

### Layer 2: General-Purpose Compression (1.5x)

- **LZ4** for messages > 32 bytes (fast, low overhead)
- **Zstd** for bulk data (better ratio, slightly slower)
- Applied per-message after semantic layer
- Threshold-based: small messages skip compression (overhead > savings)

### Complete Pipeline

```
Message created (RoverMobilityState)
  → Semantic encoding: DVec3 → quantized, f32 → u16, bool → bit
  → Delta encoding: only fields that changed since last send
  → Binary layout: varint IDs + quantized values = ~21 bytes
  → LZ4 (if > 32 bytes threshold): skip for small messages
  → Final: 21 bytes (vs original 76 bytes = 3.6x reduction)
```

### Per-Channel Compression Policy

```rust
enum CompressionPolicy {
    Never,                  // Small, frequent messages (input at 60Hz)
    Threshold(usize),       // Compress if > N bytes
    Always,                 // Large bulk data (ephemeris, telemetry archive)
}
```

| Channel | Policy | Reason |
|---|---|---|
| Unreliable input | Never | Too small, too frequent |
| Reliable commands | Threshold(32) | Commands vary in size |
| State snapshots | Threshold(64) | Can be large for full state |
| Ephemeris data | Always | Large bulk transfers |
| Yjs updates | Always | Binary updates benefit from LZ4 |

---

## Space-Standards Compatibility

### Structural Mapping: LunCoSim ↔ Space Standards

| LunCoSim Type | XTCE Concept | CCSDS Field |
|---|---|---|
| `DigitalPort` (i16) | `IntegerParameter` | 16-bit raw value |
| `PhysicalPort` (f32) | `FloatParameter` | 32-bit engineering value |
| `Wire` (scale + source) | `PolynomialCalibrator` | Calibration coefficients |
| `CommandMessage` | `MetaCommand` | TC packet |
| `CommandMessage.name` | `MetaCommand` name | APID routing |
| `CommandMessage.args` | `ArgumentList` | Data field |
| `GlobalEntityId` | `Parameter` name path | — |
| `ActionStatus` | `EnumeratedArgumentType` | Enum encoding |
| `CommandResponse` | PUS acknowledgment | PUS secondary header |
| `Session` / `AuthRegistry` | PUS User Management | Service type 1 |

**`DigitalPort` being `i16` isn't coincidence** — it's exactly the size of a typical spacecraft telemetry register.

### Three-Layer Compatibility Model

#### Layer A: Internal Game Protocol

renet2 + replicon, compressed, quantized, delta-encoded. Used for client ↔ server game state sync. **Opaque to YAMCS.**

#### Layer B: XTCE Schema (Auto-Generated)

```xml
<SpaceSystem name="LunCoSim" shortName="LCS"
    xmlns="http://www.omg.org/spec/XTCE/20180204">
  <TelemetryMetaData>
    <ParameterTypeSet>
      <IntegerParameterType name="DigitalPortType">
        <IntegerDataEncoding sizeInBits="16" signed="true"/>
      </IntegerParameterType>
      <FloatParameterType name="PhysicalPortType">
        <FloatDataEncoding sizeInBits="32"/>
      </FloatParameterType>
    </ParameterTypeSet>
    <ParameterSet>
      <Parameter name="Rover/Mobility/DriveCommand"
                 parameterTypeRef="DigitalPortType"/>
      <Parameter name="Rover/Mobility/WheelSpeed"
                 parameterTypeRef="PhysicalPortType">
        <UnitSet><Unit>rad/s</Unit></UnitSet>
      </Parameter>
    </ParameterSet>
    <ContainerSet>
      <SequenceContainer name="RoverMobilityFrame">
        <EntryList>
          <ParameterRefEntry parameterRef="Rover/Mobility/DriveCommand"/>
          <ParameterRefEntry parameterRef="Rover/Mobility/WheelSpeed"/>
        </EntryList>
      </SequenceContainer>
    </ContainerSet>
  </TelemetryMetaData>
  <CommandMetaData>
    <MetaCommandSet>
      <MetaCommand name="DRIVE_ROVER">
        <ArgumentList>
          <Argument name="Forward" argumentTypeRef="FloatArgType"/>
          <Argument name="Steer" argumentTypeRef="FloatArgType"/>
        </ArgumentList>
        <CommandContainer name="DRIVE_ROVER_CC">
          <EntryList>
            <ArgumentRefEntry argumentRef="Forward"/>
            <ArgumentRefEntry argumentRef="Steer"/>
          </EntryList>
        </CommandContainer>
      </MetaCommand>
    </MetaCommandSet>
  </CommandMetaData>
</SpaceSystem>
```

**Auto-generated from Bevy `Reflect` types.** Add a new component → it appears in XTCE automatically.

#### Layer C: CCSDS Packet Stream (External)

```
CCSDS Primary Header (6 bytes, big-endian):
┌─────────────────────────────────────┐
│ Ver(3)│Type(1)│Sec(1)│APID_hi(3)    │ Byte 0
│ APID_lo(8)                           │ Byte 1
│ SeqFlags(2)│SeqCount_hi(6)          │ Byte 2
│ SeqCount_lo(8)                       │ Byte 3
│ DataLength(16)                       │ Bytes 4-5
└─────────────────────────────────────┘

PUS Secondary Header (when SecFlag=1):
┌─────────────────────────────────────┐
│ PUS_Version(8)                      │
│ ServiceType(8) │ ServiceSubtype(8)  │
│ DestinationId(8)                    │
│ CCSDSTime(7 bytes)                  │
└─────────────────────────────────────┘

Data Field: parameter values, command args, etc.
```

### YAMCS Bridge

Server-side system pushes telemetry to YAMCS via WebSocket:

```
LunCoSim Server → YAMCS WebSocket API
  Parameters: [
    { name: "Rover1/Mobility/DriveCommand", raw: 16384, eng: 0.5, time: ... },
    { name: "Rover1/Mobility/WheelSpeed", raw: 0, eng: 0.025, time: ... },
  ]
```

YAMCS loads the XTCE XML, decodes incoming CCSDS packets, displays in mission control dashboard. **Zero changes to simulation code.**

---

## Interest Management

### Problem: State Explosion

1000 entities × 20 components × 60Hz = 1,200,000 updates/sec → unsustainable. Solution: **only send what each client needs.**

### Interest Levels

```
HIGH interest (possessed entity ±500m):
  → Full replication + prediction + all subsystems at 60Hz

MEDIUM interest (visible entities, same habitat):
  → State replication only at 10Hz, no prediction

LOW interest (rest of lunar city):
  → Aggregate stats only (power grid total, habitat O2 level)
  → Event-driven updates only, no entity-level detail
```

### Bandwidth Budget After Interest Management

```
Per client:
  HIGH (1 entity):   312 bytes/s
  MEDIUM (3 entities): 900 bytes/s
  LOW (aggregates):   52 bytes/s
  Protocol overhead: 200 bytes/s
  ─────────────────────────────
  Total: ~1.5 KB/s per client
  10 clients: ~15 KB/s server egress

Without interest management: ~500 KB/s (33x more)
```

---

## Entity Identity Mapping

### The Problem

Bevy `Entity` IDs are process-local. Entity `5v3` on client means nothing on server. `Entity` is an index into this specific World's entity storage — it has no meaning outside this process, like a file descriptor.

### Solution: GlobalEntityId as a Component

`GlobalEntityId` is a **component** attached to every entity at spawn time. Domain code assigns it once and never interacts with it again. The networking layer reads it from entities at boundary crossings.

```rust
// Defined in lunco-core — just a type, like DigitalPort
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Reflect, Default)]
#[reflect(Component, Default)]
pub struct GlobalEntityId(pub u64);  // ULID-derived
```

### The Key Design Rule

**GlobalEntityId is a component, never a field type.** Domain code uses `Entity` everywhere — in queries, in component fields (`Wire.source`, `ControllerLink.vessel_entity`, `FlightSoftware.port_map`), in hierarchy (`ChildOf`). The networking layer reads `GlobalEntityId` from entities when crossing boundaries (serialization, command resolution, edit logging).

```
Domain code (always uses Entity):
  query.iter() → Entity → access components
  Wire { source: Entity, target: Entity }
  CommandMessage { target: Entity }

Networking layer (translates at boundary):
  Send:    Entity(2v0) → read GlobalEntityId component → send u64
  Receive: network bytes → resolve GlobalEntityId → Entity(4v0)

Domain code (always uses Entity):
  CommandMessage { target: Entity(4v0) } → observer runs
```

### Why Not GlobalEntityId Everywhere

Using `GlobalEntityId` in component fields would add a HashMap lookup to every system iteration:

```rust
// BAD: every system needs Entity to access components
fn some_system(
    query: Query<(Entity, &GlobalEntityId, &SomeComponent)>,
    resolver: Res<EntityResolver>,
) {
    for (local_entity, global_id, component) in query.iter() {
        // Can't avoid Entity — Bevy requires it for component access.
        // GlobalEntityId is just extra baggage here.
    }
}

// GOOD: networking layer reads GlobalEntityId at boundary
fn replicate_state(
    q_changed: Query<(Entity, &GlobalEntityId, &SomeComponent), Changed<SomeComponent>>,
) {
    for (entity, global_id, component) in q_changed.iter() {
        // Only the networking layer reads GlobalEntityId.
        // Domain systems never see it.
    }
}
```

### Assignment: Automatic via Observer

Domain code never manually assigns `GlobalEntityId`. The networking layer's observer catches entities that become replicated:

```rust
// In lunco-networking: fires when bevy_replicon marks an entity as replicated
app.add_observer(|trigger: On<Add<Replicated>>, mut commands: Commands,
                 q_has_id: Query<(), With<GlobalEntityId>>| {
    let entity = trigger.target();
    if q_has_id.get(entity).is_err() {
        commands.entity(entity)
            .insert(GlobalEntityId(ulid::Ulid::new().as_u128() as u64));
    }
});
```

`Replicated` is a marker component added by bevy_replicon when an entity has at least one registered replication component. This observer fires automatically — zero changes to spawn sites.

| Creation Path | Gets GlobalEntityId? | How |
|---|---|---|
| USD-loaded rover | ✅ Yes | Has `RoverMobilityState` → registered → observer fires |
| DigitalPort spawned by FSW | ✅ Yes | Has `DigitalPort` → registered → observer fires |
| User-spawned rover (sandbox) | ✅ Yes | Has `RoverVessel` → registered → observer fires |
| Avatar / camera | ❌ No | No replicated components → observer never fires |
| SpawnGhost | ❌ No | No replicated components → observer never fires |
| UI panels | ❌ No | No replicated components → observer never fires |

### The EntityRegistry

Each process maintains a bidirectional map, auto-populated from `Added<GlobalEntityId>`:

```rust
#[derive(Resource)]
pub struct EntityRegistry {
    local_to_global: HashMap<Entity, GlobalEntityId>,
    global_to_local: HashMap<GlobalEntityId, Entity>,
}

fn sync_entity_registry(
    mut registry: ResMut<EntityRegistry>,
    new_entities: Query<(Entity, &GlobalEntityId), Added<GlobalEntityId>>,
    removed: RemovedComponents<GlobalEntityId>,
) {
    for (entity, id) in new_entities.iter() {
        registry.local_to_global.insert(entity, *id);
        registry.global_to_local.insert(*id, entity);
    }
    // Clean up despawned entries...
}
```

### Cross-Process Resolution

```
Server's World:
  Entity(2v0) + GlobalEntityId(0xDEADBEEF0001) = /SkidRover
  Entity(3v0) + GlobalEntityId(0xDEADBEEF0002) = /SkidRover/Wheel_FL

Client's World (same scene, different process):
  Entity(4v0) + GlobalEntityId(0xDEADBEEF0001) = /SkidRover     ← SAME GlobalEntityId
  Entity(5v0) + GlobalEntityId(0xDEADBEEF0002) = /SkidRover/Wheel_FL

Command resolution:
  Client: Entity(4v0) → GlobalEntityId(0xDEADBEEF0001) → network
  Server: network → GlobalEntityId(0xDEADBEEF0001) → Entity(2v0) → trigger observer
```

### Why ULID-Derived u64

| Option | Size | Ordering | Collision Risk |
|---|---|---|---|
| UUID (u128) | 16 bytes | No | Near-zero |
| String | 36+ bytes | Lexicographic | None |
| ULID subset (u64) | 8 bytes | **Monotonic** | Near-zero |

ULID gives time-ordered IDs (timestamp in high bits) — useful for the EditLog, since op_id ordering matches entity creation order. Two entities spawned at the same millisecond get different random portions.

---

## What Domain Code Sees

```rust
// lunco-mobility/src/lib.rs — ZERO networking awareness

#[derive(Component, Clone, Copy, Reflect)]
#[reflect(Component)]
struct DriveCommand {
    digital: DigitalPort,    // i16 — FSW register
    physical: PhysicalPort,  // f32 — actual wheel speed
}

fn apply_drive_commands(
    mut query: Query<(&DriveCommand, &mut GlobalTransform)>,
) {
    for (drive, mut transform) in query.iter_mut() {
        let speed = drive.physical.value;
        transform.translation += DVec3::Z * speed as f64 * dt;
    }
}
```

That's it. Replication, compression, CCSDS export, YAMCS bridge, auth, edit history — all handled by `lunco-networking` plugin registered at startup.

---

## Existing Solutions Evaluated

### Why NOT Build on ROS2 / SpaceROS

| Concern | ROS2/SpaceROS | LunCoSim (Bevy ECS) |
|---|---|---|
| **Determinism** | Async, non-deterministic message ordering | Deterministic system ordering |
| **Single binary** | Requires `ros2 daemon`, DDS, multiple processes | `cargo run` |
| **Headless-first** | DDS infrastructure always running (~100MB+) | `MinimalPlugins` only, KB footprint |
| **WASM/browser** | DDS doesn't work in browsers | Native WebSocket/WebTransport support |
| **f64 / big_space** | ROS2 uses `float64`, no grid system | `DVec3`, `f64`, big_space grids |
| **TDD** | Test requires DDS infrastructure | `App::new()` + `world.update()` |
| **rclrs maturity** | Just introduced at FOSDEM 2026, early stage | Mature Bevy 0.18 ecosystem |

**Decision**: Don't replace — **bridge**. LunCoSim stays Bevy ECS internally but can communicate with ROS2 nodes and DDS publishers over a transparent bridge layer.

### VIPER Rover Hybrid Pattern

NASA's VIPER lunar rover uses **cFS (flight) + ROS2 (autonomy)** hybrid:

```
cFS (deterministic, safety-critical)
  ┌ CMD app (CCSDS)
  ┌ TLM app (CCSDS)
  ┌ Health & Safety
  └ Software Bus
      ↕  UDP/Protobuf bridge
  ROS2 Nodes (autonomy, perception, navigation)
```

LunCoSim maps to this pattern:
- `lunco-obc` + `CommandMessage` ≈ cFS Software Bus
- `lunco-mobility` + `lunco-robotics` ≈ ROS2 nodes
- Networking layer ≈ the bridge

### Other Standards Landscape

| Standard | Status in LunCoSim | What's Needed |
|---|---|---|
| **CCSDS Space Packets** | ❌ Missing | 6-byte primary header encoder/decoder, big-endian |
| **XTCE (CCSDS 660.0)** | ⚠ Partial structure | Auto-generation from `Reflect` types |
| **PUS (ECSS)** | ❌ Missing | Secondary header, service types 1-20 |
| **DDS (OMG)** | ❌ Missing | Topic bridge (DDS ↔ CommandMessage) |
| **cFS Software Bus** | ⚠ Conceptual match | UDP bridge ↔ CommandMessage |
| **F Prime serialization** | ❌ Missing | `Fw::Serialize` compatible format |
| **CCSDS Time** | ❌ Missing | 7-byte CCSDS time code resource |
| **CFDP (file delivery)** | ❌ Missing | For USD/asset sync over network |

---

## Implementation Phases

### Phase 1: Foundation (Transport + Replication + Auth)

- [ ] **1.1** Add `bevy_replicon` as optional dependency to each domain crate (`[features] networking = ["dep:bevy_replicon"]`)
- [ ] **1.2** Add `replication.rs` submodules to domain crates (`lunco-mobility`, `lunco-fsw`, `lunco-celestial`, `lunco-hardware`)
- [ ] **1.3** Implement `Session`, `SessionId`, `Identity`, `Role`, `AuthRegistry` in lunco-networking
- [ ] **1.4** Implement `AuthorizedCommand` event type
- [ ] **1.5** Implement transport abstraction with feature-gated selection (UDP/WebSocket)
- [ ] **1.6** Integrate `bevy_replicon` with renet2 backend in lunco-networking
- [ ] **1.7** Observer: `On<Add<Replicated>>` → auto-assign `GlobalEntityId` to replicated entities
- [ ] **1.8** Implement `EntityRegistry` (local ↔ global ID mapping)
- [ ] **1.9** Implement per-field serializers in domain replication submodules (e.g., Wire scale-only serialization)
- [ ] **1.10** Implement server-side command injection (auth check → `AuthorizedCommand`)
- [ ] **1.11** Implement `CommandDictionary` (string → u8 encoding)
- [ ] **1.12** Wire replication plugins in multiplayer binary configuration

### Phase 2: Collaborative Editing

- [ ] **2.1** Implement `EditEvent` enum with all variants
- [ ] **2.2** Implement `EditLog` resource with append-only history
- [ ] **2.3** Implement `LamportClock` resource for total ordering
- [ ] **2.4** Implement recording system (reads `Added<GlobalEntityId>` → creates EditEvent)
- [ ] **2.5** Implement gizmo edit recording (TransformChanged events)
- [ ] **2.6** Implement parameter change recording (ParameterChanged events)
- [ ] **2.7** Implement EditLog replay (for new client join)
- [ ] **2.8** Implement EditLog checkpoint (periodic state snapshot)

### Phase 3: Networked Undo

- [ ] **3.1** Implement `EditLog::reverse_event()` for all EditEvent variants
- [ ] **3.2** Implement UNDO command handling via AuthorizedCommand
- [ ] **3.3** Implement undo broadcast as StateNotification to all clients
- [ ] **3.4** Implement edit history timeline panel (UI plugin)

### Phase 4: Client-Side Prediction

- [ ] **4.1** Implement `PredictedState` component
- [ ] **4.2** Implement client-side rover prediction system
- [ ] **4.3** Implement server state snapshot system
- [ ] **4.4** Implement reconciliation system (snap + replay pending inputs)
- [ ] **4.5** Implement input sequence tracking and confirmation

### Phase 5: Compression

- [ ] **5.1** Implement position quantization (`DVec3` → `u16×3` relative to grid origin)
- [ ] **5.2** Implement quaternion compression (smallest-three, 8 bytes)
- [ ] **5.3** Implement delta encoding tracker (only send changed fields)
- [ ] **5.4** Implement dead reckoning for constant-velocity entities
- [ ] **5.5** Implement LZ4 per-channel compression with threshold policy
- [ ] **5.6** Implement boolean bit-packing and VarInt encoding

### Phase 6: Interest Management

- [ ] **6.1** Implement `NetworkInterest` component + `InterestDetail` enum
- [ ] **6.2** Implement spatial interest system (distance-based subscription)
- [ ] **6.3** Implement possession-based interest (auto-HIGH for controlled entity)
- [ ] **6.4** Implement aggregate stats system for LOW-interest entities
- [ ] **6.5** Implement interest-based replication filter

### Phase 7: Yjs for Modelica Collaboration

- [ ] **7.1** Add `yrs` dependency to `lunco-modelica`
- [ ] **7.2** Implement `CollaborativeModelicaDoc` wrapper
- [ ] **7.3** Implement Yjs update channel (renet2 CHANNEL_YJS_UPDATE)
- [ ] **7.4** Implement sync systems (local edit → network, network → local doc)
- [ ] **7.5** Implement Yjs awareness protocol (collaborative cursors)
- [ ] **7.6** Update code editor panel to use Yjs-backed document

### Phase 8: Dynamic USD Support

- [ ] **8.1** Implement USD file watcher (`notify` crate)
- [ ] **8.2** Implement `RELOAD_USD_FILE` command handler
- [ ] **8.3** Implement USD reload → record deletes + spawns as EditEvents
- [ ] **8.4** Implement runtime catalog modification + broadcast
- [ ] **8.5** Implement catalog entry sync to all clients

### Phase 9: Space Standards Bridge

- [ ] **9.1** Implement CCSDS primary header encoder/decoder
- [ ] **9.2** Implement PUS secondary header encoder/decoder
- [ ] **9.3** Implement CCSDS time code (7-byte format)
- [ ] **9.4** Implement XTCE auto-generator from Bevy `Reflect` registry
- [ ] **9.5** Implement YAMCS WebSocket bridge
- [ ] **9.6** Implement CCSDS packet builder from `DigitalPort`/`PhysicalPort`

### Phase 10: ROS/DDS Bridge (Hardware-in-Loop)

- [ ] **10.1** Implement DDS topic bridge (DDS topics ↔ CommandMessage)
- [ ] **10.2** Implement cFS Software Bus UDP bridge
- [ ] **10.3** Implement F Prime Protobuf bridge
- [ ] **10.4** Implement SpaceROS node integration

### Phase 11: UI Plugin

- [ ] **11.1** Implement `lunco-networking-ui` (Layer 4)
- [ ] **11.2** Connection status panel (connect/disconnect, latency, packet loss)
- [ ] **11.3** Authority panel (request/release vessel control)
- [ ] **11.4** Peer list viewer (who's connected, what they possess)
- [ ] **11.5** Interest debug visualizer (show what you're subscribed to)
- [ ] **11.6** Edit history timeline panel
- [ ] **11.7** Collaboration panel (who's editing what, collaborative cursors)

---

## Bandwidth Budget

### Full Scenario: 10 Clients, Lunar City

```
Assets: 15 rovers, 1 habitat, 3 spacecraft, power grid, habitat systems

Per client (average):
  HIGH interest (1 possessed rover):
    Position: 6 bytes × 60Hz (dead reckoning skips 80%) = 72 B/s
    Rover state (flags, speed): 4 bytes × 60Hz = 240 B/s
    Subtotal: ~312 B/s

  MEDIUM interest (3 nearby rovers):
    Position: 6 bytes × 10Hz (dead reckoning skips 50%) = 180 B/s
    Rover state: 4 bytes × 10Hz = 120 B/s
    Subtotal: 300 B/s × 3 = 900 B/s

  LOW interest (rest of base, aggregates):
    Power grid total: 8 bytes × 2Hz = 16 B/s
    Habitat status: 4 bytes × 0.05Hz = 0.2 B/s
    Spacecraft orbits: 12 bytes × 1Hz × 3 = 36 B/s
    Subtotal: ~52 B/s

  Protocol overhead (headers, acks): ~200 B/s

Per client: ~1.5 KB/s
10 clients: ~15 KB/s server egress

Compare to uncompressed naive: ~500 KB/s (33x reduction)
```

### Savings Breakdown

| Technique | Savings |
|---|---|
| Interest management | 10x (only send relevant entities) |
| Position quantization | 4x (24 → 6 bytes) |
| Dead reckoning | 5x (send 1 in 5 position updates) |
| Delta encoding | 3x (only changed fields) |
| Boolean/VarInt packing | 2x (small values, bit flags) |
| LZ4 compression | 1.5x (on remaining bytes) |
| **Total** | **~33x** |

---

## Cargo Feature Matrix

```
Feature              Native Desktop    Browser (WASM)    Dedicated Server
────────────────────────────────────────────────────────────────────────
transport-udp        ✅                ❌                ✅
transport-ws         ✅                ✅                ✅
transport-wt         ✅                ✅ (future)       ✅
transport-server     ❌                ❌                ✅
────────────────────────────────────────────────────────────────────────
Replication          ✅                ✅                ✅ (authoritative)
Prediction           ✅                ✅                ❌ (server doesn't predict)
CCSDS export         ✅                ❌ (no raw sockets)✅
YAMCS bridge         ✅                ❌                ✅
DDS bridge           ✅                ❌                ✅
Edit history         ✅                ✅                ✅ (server-side)
Yjs collaboration    ✅                ✅                ✅
USD file watch       ✅                ❌ (no fs access) ✅
────────────────────────────────────────────────────────────────────────
```

---

## References

- [renet2](https://github.com/UkoeHB/renet2) — Transport abstraction (UDP, WS, WT, Steam)
- [bevy_replicon](https://github.com/simgine/bevy_replicon) — ECS replication for Bevy
- [bevy_replicon_renet2](https://github.com/simgine/bevy_replicon_renet) — Renet2 backend for replicon
- [yrs (Yjs Rust)](https://github.com/y-crdt/y-crdt) — CRDT-based collaborative editing
- [CCSDS 133.0-B-2 Space Packet Protocol](https://ccsds.org/Pubs/133x0b2e2.pdf) — 6-byte primary header standard
- [CCSDS 660.0-B-2 XTCE](https://ccsds.org/Pubs/660x0b2.pdf) — XML Telemetric and Command Exchange
- [YAMCS](https://docs.yamcs.org/) — Mission control system with WebSocket API
- [NASA cFS](https://github.com/nasa/cFS) — core Flight System framework
- [F Prime (JPL)](https://github.com/nasa/fprime) — Flight software framework (Ingenuity helicopter)
- [SpaceROS](https://github.com/space-ros) — Hardened ROS2 for space robotics
- [VIPER Rover Architecture](https://ntrs.nasa.gov/api/citations/20250004148/downloads/viper-2025-04-24.pdf) — cFS + ROS2 hybrid pattern
