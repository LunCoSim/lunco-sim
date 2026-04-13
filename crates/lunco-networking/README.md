# lunco-networking

Networking layer for LunCoSim — the transparent bridge between simulation state and wire protocols.

**Domain crates never import this crate.** They declare `app.replicate::<MyComponent>()` and the networking layer handles wire format, compression, and protocol translation silently.

---

## Table of Contents

- [Architecture Overview](#architecture-overview)
- [Transport Abstraction](#transport-abstraction)
- [ECS Replication Model](#ecs-replication-model)
- [Authority & Possession](#authority--possession)
- [Client-Side Prediction](#client-side-prediction)
- [Compression Stack](#compression-stack)
- [Space-Standards Compatibility](#space-standards-compatibility)
- [Interest Management](#interest-management)
- [Entity Identity Mapping](#entity-identity-mapping)
- [What Domain Code Sees](#what-domain-code-sees)
- [Existing Solutions Evaluated](#existing-solutions-evaluated)
- [Implementation Phases](#implementation-phases)
- [Bandwidth Budget](#bandwidth-budget)

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
Layer 2b: NetworkingPlugin    — lunco-networking (transport, replication, bridges)
Layer 1: SimCore              — MinimalPlugins, ScheduleRunner, big_space, Avian3D
```

Networking is a **Layer 2b** domain plugin — self-contained, headless-compatible, removable without affecting simulation correctness.

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
| Avatar camera | Local only | **Never replicated** — per-client input device |

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
    pub owner_client_id: Option<u64>,  // None = uncontrolled
    pub pending_request: Option<u64>,   // Client ID waiting for control
}
```

### Possession Negotiation Flow

```
Client A                          Server                          Client B
   │                                │                                │
   │── RequestAuthority(rover1) ───>│                                │
   │                                │── GrantAuthority(rover1) ───>  │ (notify: A controls rover1)
   │<─ AuthorityGranted(rover1) ────│                                │
   │                                │                                │
   │ [Local control begins]        │                                │
   │ [DRIVE_ROVER → server]        │                                │
   │                                │── Replicate(rover1 state) ───> │
   │                                │                                │
   │── ReleaseAuthority(rover1) ──> │                                │
   │                                │── AuthorityReleased(rover1) ──>│ (notify: rover1 free)
```

### CommandMessage Over Network

The existing `CommandMessage` fabric serializes directly:

```rust
// Client side:
let cmd = CommandMessage { name: "POSSESS", target: rover, source: avatar, .. };
let net_cmd = NetworkCommand {
    name: cmd.name.clone(),
    target_global_id: entity_to_global_id(cmd.target),
    source_client_id: MY_CLIENT_ID,
    args: cmd.args.clone(),
};
renet_client.send_message(CHANNEL_RELIABLE, bincode::serialize(&net_cmd));

// Server side:
// Deserialize → resolve GlobalEntityId → Entity → commands.trigger(cmd)
// Same observer runs. Zero changes to on_possess_command.
```

**Key insight**: `CommandMessage` is structurally equivalent to cFS Software Bus messages. The bridge is serialization format, not architecture.

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

### Dynamic Interest Updates

```rust
fn update_client_interests(
    avatars: Query<(&GlobalTransform, &PossessedEntity), With<Avatar>>,
    mut interests: Query<&mut NetworkInterest>,
) {
    for (avatar_pos, possessed) in avatars.iter() {
        // HIGH: possessed entity
        if let Some(entity) = possessed.0 {
            interest.detail_level.insert(client_id, InterestDetail::Full);
        }
        // MEDIUM: everything within 2km radius
        // LOW: everything else → aggregate stats only
    }
}
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

Bevy `Entity` IDs are process-local. Entity `5v3` on client means nothing on server.

### Solution: GlobalEntityId

```rust
/// Stable identifier valid across client/server boundary
#[derive(Component, Clone, Copy)]
pub struct GlobalEntityId(pub u64);  // ULID or snowflake ID

/// Per-client registry
pub struct EntityRegistry {
    local_to_global: HashMap<Entity, GlobalEntityId>,
    global_to_local: HashMap<GlobalEntityId, Entity>,
}
```

### Lifecycle

```
Server spawns rover:
  → Assign GlobalEntityId(ulid::new())
  → Replicate GlobalEntityId to all clients
  → Clients create local entity, store mapping

Client sends command:
  → Resolves local Entity → GlobalEntityId
  → Sends GlobalEntityId over network

Server receives command:
  → Resolves GlobalEntityId → server Entity
  → Triggers CommandMessage with local Entity
```

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

That's it. Replication, compression, CCSDS export, YAMCS bridge — all handled by `lunco-networking` plugin registered at startup.

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

### Phase 1: Foundation (Transport + Replication)

- [ ] **1.1** Add dependencies: `bevy_renet2`, `bevy_replicon`, `bevy_replicon_renet2`, `ulid`, `bincode`, `serde`, `lz4_flex`
- [ ] **1.2** Implement `GlobalEntityId` component + `EntityRegistry` resource
- [ ] **1.3** Implement transport abstraction with feature-gated selection (UDP/WebSocket)
- [ ] **1.4** Integrate `bevy_replicon` with renet2 backend
- [ ] **1.5** Register baseline component serializers (`GlobalTransform`, `DigitalPort`, `PhysicalPort`)

### Phase 2: Authority & Commands

- [ ] **2.1** Implement `NetworkAuthority` component + possession negotiation systems
- [ ] **2.2** Implement `NetworkCommand` serialization (CommandMessage over wire)
- [ ] **2.3** Implement command dictionary (string → u8 encoding)
- [ ] **2.4** Implement server-side command injection (deserialize → `commands.trigger()`)
- [ ] **2.5** Implement client disconnect → release all authority

### Phase 3: Compression

- [ ] **3.1** Implement position quantization (`DVec3` → `u16×3` relative to grid origin)
- [ ] **3.2** Implement quaternion compression (smallest-three, 8 bytes)
- [ ] **3.3** Implement delta encoding tracker (only send changed fields)
- [ ] **3.4** Implement dead reckoning for constant-velocity entities
- [ ] **3.5** Implement LZ4 per-channel compression with threshold policy
- [ ] **3.6** Implement boolean bit-packing and VarInt encoding

### Phase 4: Client-Side Prediction

- [ ] **4.1** Implement `PredictedState` component
- [ ] **4.2** Implement client-side rover prediction system
- [ ] **4.3** Implement server state snapshot system
- [ ] **4.4** Implement reconciliation system (snap + replay pending inputs)
- [ ] **4.5** Implement input sequence tracking and confirmation

### Phase 5: Interest Management

- [ ] **5.1** Implement `NetworkInterest` component + `InterestDetail` enum
- [ ] **5.2** Implement spatial interest system (distance-based subscription)
- [ ] **5.3** Implement possession-based interest (auto-HIGH for controlled entity)
- [ ] **5.4** Implement aggregate stats system for LOW-interest entities
- [ ] **5.5** Implement interest-based replication filter

### Phase 6: Space Standards Bridge

- [ ] **6.1** Implement CCSDS primary header encoder/decoder
- [ ] **6.2** Implement PUS secondary header encoder/decoder
- [ ] **6.3** Implement CCSDS time code (7-byte format)
- [ ] **6.4** Implement XTCE auto-generator from Bevy `Reflect` registry
- [ ] **6.5** Implement YAMCS WebSocket bridge
- [ ] **6.6** Implement CCSDS packet builder from `DigitalPort`/`PhysicalPort`

### Phase 7: ROS/DDS Bridge (Hardware-in-Loop)

- [ ] **7.1** Implement DDS topic bridge (DDS topics ↔ CommandMessage)
- [ ] **7.2** Implement cFS Software Bus UDP bridge
- [ ] **7.3** Implement F Prime Protobuf bridge
- [ ] **7.4** Implement SpaceROS node integration

### Phase 8: UI Plugin

- [ ] **8.1** Implement `lunco-networking-ui` (Layer 4)
- [ ] **8.2** Connection status panel (connect/disconnect, latency, packet loss)
- [ ] **8.3** Authority panel (request/release vessel control)
- [ ] **8.4** Peer list viewer (who's connected, what they possess)
- [ ] **8.5** Interest debug visualizer (show what you're subscribed to)

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
```

---

## References

- [renet2](https://github.com/UkoeHB/renet2) — Transport abstraction (UDP, WS, WT, Steam)
- [bevy_replicon](https://github.com/simgine/bevy_replicon) — ECS replication for Bevy
- [bevy_replicon_renet2](https://github.com/simgine/bevy_replicon_renet) — Renet2 backend for replicon
- [CCSDS 133.0-B-2 Space Packet Protocol](https://ccsds.org/Pubs/133x0b2e2.pdf) — 6-byte primary header standard
- [CCSDS 660.0-B-2 XTCE](https://ccsds.org/Pubs/660x0b2.pdf) — XML Telemetric and Command Exchange
- [YAMCS](https://docs.yamcs.org/) — Mission control system with WebSocket API
- [NASA cFS](https://github.com/nasa/cFS) — core Flight System framework
- [F Prime (JPL)](https://github.com/nasa/fprime) — Flight software framework (Ingenuity helicopter)
- [SpaceROS](https://github.com/space-ros) — Hardened ROS2 for space robotics
- [VIPER Rover Architecture](https://ntrs.nasa.gov/api/citations/20250004148/downloads/viper-2025-04-24.pdf) — cFS + ROS2 hybrid pattern
