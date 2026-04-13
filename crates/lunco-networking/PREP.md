# lunco-networking: Preparation Plan

## Principle

Add **types and markers now** so domain crates can reference them. **Don't implement** networking, auth, or replication yet — just define the contracts. This prevents future refactoring when the real features land.

**Key rules:**
- `GlobalEntityId` is a component, never a field type
- Domain code uses `Entity` everywhere — queries, hierarchy, component fields
- Networking layer reads `GlobalEntityId` from entities at boundary crossings
- Domain crates own their own replication submodules (feature-gated)
- No central aggregator crate — the binary wires everything together

---

## The Responsibility Model

```
┌─────────────────────────────────────────────────────────────────┐
│  ENTITY LIFECYCLE                                                │
│                                                                  │
│  1. SPAWN (domain code)                                          │
│     commands.spawn((RoverVessel, /* ... */))                     │
│     → Domain code knows nothing about networking                 │
│                                                                  │
│  2. RUN (domain systems)                                         │
│     query.iter() → Entity → do physics/FSW/celestial logic      │
│     → Domain code uses Entity, never GlobalEntityId             │
│                                                                  │
│  3. BOUNDARY CROSS (networking layer intercepts)                 │
│     Replicon: reads GlobalEntityId component → serializes       │
│     Commands: reads GlobalEntityId from network → resolves      │
│     → Domain code is unaware of the translation                 │
│                                                                  │
│  4. HISTORY (networking layer records)                           │
│     EditLog: records GlobalEntityId + session + op_id           │
│     → Domain code doesn't record history                        │
└─────────────────────────────────────────────────────────────────┘
```

### What Uses What

| Code Layer | Uses Entity | Uses GlobalEntityId | Why |
|---|---|---|---|
| Domain systems (physics, FSW, celestial) | ✅ Everywhere | ❌ Never | Bevy ECS uses Entity for queries |
| Component fields (Wire, ControllerLink, port_map) | ✅ Entity | ❌ Never | Local references within one process |
| Entity spawn sites | ✅ Bevy's spawn API | ❌ Assigned by observer | Networking observer adds it after domain spawns |
| Networking: EntityRegistry | ✅ Maps to/from | ✅ Maps to/from | Boundary resolution |
| Networking: Replicon serializers | ❌ | ✅ Reads from component | Attaches stable ID to state updates |
| Networking: Command resolver | ✅ After resolution | ✅ From network message | Resolves global → local for observers |
| Networking: EditLog | ❌ | ✅ Records in history | Identity survives process restarts |

### What Does NOT Change

| Thing | Current | After Prep | Why |
|---|---|---|---|
| `Wire { source: Entity, target: Entity }` | Unchanged | Unchanged | Local join within one process. Per-field serialization handled in domain's replication submodule. |
| `FlightSoftware.port_map: HashMap<String, Entity>` | Unchanged | Unchanged | Local port lookup. Serialization handled in domain's replication submodule. |
| `ControllerLink.vessel_entity: Entity` | Unchanged | Unchanged | Avatar and vessel always in same World. |
| `PendingWheelWiring` fields | Unchanged | Unchanged | Temporary local reference during USD loading. |

---

## Architecture: Domain-Owned Replication

No central aggregator crate. No `lunco-replication`. The pattern mirrors `lunco-ui` correctly:

```
lunco-networking (infrastructure):
  → Provides: Replicon backend, GlobalEntityId, auth, EditLog
  → Depends on: only lunco-core
  → Imports ZERO domain types

Domain crates depend on lunco-networking (feature-gated):
  lunco-mobility/src/replication.rs  → LunCoMobilityReplicationPlugin
  lunco-fsw/src/replication.rs       → LunCoFswReplicationPlugin
  lunco-celestial/src/replication.rs → LunCoCelestialReplicationPlugin
  lunco-hardware/src/replication.rs  → LunCoHardwareReplicationPlugin

Binary wires it up for multiplayer:
  app.add_plugins(LunCoMobilityPlugin)
  app.add_plugins(LunCoFswPlugin)
  app.add_plugins(LunCoNetworkingPlugin)              // backend
  app.add_plugins(LunCoMobilityReplicationPlugin)     // mobility types
  app.add_plugins(LunCoFswReplicationPlugin)          // fsw types
```

### Example: Domain Replication Submodule

```rust
// lunco-mobility/src/replication.rs
#[cfg(feature = "networking")]
use bevy::prelude::*;
#[cfg(feature = "networking")]
use bevy_replicon::prelude::*;
#[cfg(feature = "networking")]
use crate::{RoverMobilityState, DifferentialDrive, WheelRaycast, Suspension};

/// Declares which mobility components cross the network boundary.
#[cfg(feature = "networking")]
pub struct LunCoMobilityReplicationPlugin;

#[cfg(feature = "networking")]
impl Plugin for LunCoMobilityReplicationPlugin {
    fn build(&self, app: &mut App) {
        app.replicate::<RoverMobilityState>();
        app.replicate::<DifferentialDrive>();
        app.replicate::<WheelRaycast>();
        app.replicate::<Suspension>();
    }
}
```

```rust
// lunco-fsw/src/replication.rs
#[cfg(feature = "networking")]
use bevy::prelude::*;
#[cfg(feature = "networking")]
use bevy_replicon::prelude::*;
#[cfg(feature = "networking")]
use crate::{DigitalPort, PhysicalPort, Wire};

#[cfg(feature = "networking")]
pub struct LunCoFswReplicationPlugin;

#[cfg(feature = "networking")]
impl Plugin for LunCoFswReplicationPlugin {
    fn build(&self, app: &mut App) {
        app.replicate::<DigitalPort>();
        app.replicate::<PhysicalPort>();
        // Wire has Entity fields — custom serializer
        app.replicate::<Wire>()
            .set_serialization(
                |writer, wire| bincode::serialize_into(writer, &wire.scale),
                |reader, _map| {
                    let scale: f32 = bincode::deserialize_from(reader)?;
                    Ok(Wire {
                        source: Entity::PLACEHOLDER,
                        target: Entity::PLACEHOLDER,
                        scale,
                    })
                },
            );
    }
}
```

### Cargo.toml Changes (Future)

```toml
# lunco-mobility/Cargo.toml
[features]
networking = ["dep:bevy_replicon"]

[dependencies]
bevy_replicon = { version = "...", optional = true }

# lunco-fsw/Cargo.toml
[features]
networking = ["dep:bevy_replicon"]

[dependencies]
bevy_replicon = { version = "...", optional = true }
```

### The Dependency Graph

```
lunco-networking → lunco-core (for GlobalEntityId type only)

lunco-mobility  → lunco-networking (optional, feature: networking)
lunco-fsw       → lunco-networking (optional, feature: networking)
lunco-celestial → lunco-networking (optional, feature: networking)
lunco-hardware  → lunco-networking (optional, feature: networking)

NO reverse dependencies.
NO aggregator crate.
```

### Single-Player = Zero Overhead

When the `networking` feature is disabled:
- `bevy_replicon` not compiled (optional dependency)
- Replication modules excluded via `#[cfg(feature = "networking")]`
- No GlobalEntityId assigned
- No tracking systems run
- Domain crates compile exactly the same code

---

## What To Change Now (P0 — Foundation)

These changes are **purely additive**. No behavior changes. No new runtime dependencies (except `ulid` for GlobalEntityId generation in the networking observer).

### 1. lunco-core — Add identity and domain types

**File: `crates/lunco-core/src/lib.rs`**

Add `GlobalEntityId`, `SessionId`, `Role`, `Domain`:

```rust
// ── NEW: Stable entity identity (cross-process, cross-network) ──

/// Stable identifier valid across ALL processes and network boundaries.
/// Assigned once at entity creation, never changes.
///
/// This is the primary key for entity resolution when Entity IDs
/// are local to each process (which they always are in a distributed setup).
///
/// # Design
///
/// GlobalEntityId is a **component** attached to entities. Domain code
/// assigns it at spawn time and never interacts with it again.
/// The networking layer reads it from entities at boundary crossings
/// (serialization, command resolution, edit logging).
///
/// # Why u64?
///
/// Derived from ULID: timestamp in high bits (monotonic ordering for
/// EditLog), random in low bits (collision-free across all processes).
#[derive(Component, Clone, Copy, PartialEq, Eq, Hash, Reflect, Default)]
#[reflect(Component, Default)]
pub struct GlobalEntityId(pub u64);

// ── NEW: Session and role types (for future auth) ──

/// Opaque session identifier. Assigned by auth layer on successful connection.
/// Domain code only uses this for attribution (edit history, undo).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
pub struct SessionId(pub u64);

/// Role-based access control (defined now, enforced later).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
pub enum Role {
    Observer,
    Operator,
    ModelicaEngineer,
    FswEngineer,
    Admin,
}

/// Domain partitioning (defined now, routed later).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
pub enum Domain {
    Physics,
    Fsw,
    Celestial,
    Collaboration,
    Visualization,
}
```

Fix marker components to have `Clone + Copy + Reflect`:

```rust
// Before:
#[derive(Component)]
pub struct Avatar;

// After:
#[derive(Component, Clone, Copy, Reflect, Default)]
#[reflect(Component, Default)]
pub struct Avatar;

// Same for Vessel, RoverVessel, SelectableRoot, Ground:
#[derive(Component, Clone, Copy, Reflect, Default)]
#[reflect(Component, Default)]
pub struct Vessel;

#[derive(Component, Clone, Copy, Reflect, Default)]
#[reflect(Component, Default)]
pub struct RoverVessel;

#[derive(Component, Clone, Copy, Reflect, Default)]
#[reflect(Component, Default)]
pub struct SelectableRoot;

#[derive(Component, Clone, Copy, Reflect, Default)]
#[reflect(Component, Default)]
pub struct Ground;
```

**File: `crates/lunco-core/src/architecture.rs`**

Add `Clone + Copy + Reflect` to `ControllerLink`:

```rust
// Before:
#[derive(Component, Debug)]
pub struct ControllerLink {
    pub vessel_entity: Entity,
}

// After:
#[derive(Component, Debug, Clone, Copy, Reflect)]
#[reflect(Component)]
pub struct ControllerLink {
    pub vessel_entity: Entity,
}
```

**File: `crates/lunco-core/Cargo.toml`**

Add dependency:
```toml
serde = { version = "1", features = ["derive"] }
```

---

### 2. lunco-networking — GlobalEntityId observer

**File: `crates/lunco-networking/src/lib.rs`**

The networking plugin assigns GlobalEntityId to entities that will be replicated:

```rust
/// Observer: assigns GlobalEntityId to any entity that gets a replicated component.
///
/// This runs AFTER domain plugins register their replication types.
/// The `Replicated` marker is added by bevy_replicon automatically when
/// an entity has at least one registered component.
pub fn assign_global_entity_id(
    trigger: On<Add<Replicated>>,
    mut commands: Commands,
    q_has_id: Query<(), With<GlobalEntityId>>,
) {
    let entity = trigger.target();
    if q_has_id.get(entity).is_err() {
        commands.entity(entity)
            .insert(GlobalEntityId(ulid::Ulid::new().as_u128() as u64));
    }
}
```

Register in `LunCoNetworkingPlugin::build()`:
```rust
app.add_observer(assign_global_entity_id);
```

Add `ulid` dependency to `lunco-networking/Cargo.toml`:
```toml
ulid = { version = "1", features = ["serde"] }
```

---

### 3. lunco-networking — Stub `#[network(ignore)]` macro

**File: `crates/lunco-networking/Cargo.toml`**

Add proc-macro crate reference:
```toml
[dependencies]
lunco-networking-macro = { path = "lunco-networking-macro" }
```

**File: `crates/lunco-networking/lunco-networking-macro/Cargo.toml`**

```toml
[package]
name = "lunco-networking-macro"
version = "0.1.0-dev"
edition = "2021"

[lib]
proc-macro = true

[dependencies]
```

**File: `crates/lunco-networking/lunco-networking-macro/src/lib.rs`**

```rust
//! Attribute macro that marks a field to be excluded from network serialization.
//!
//! Currently a pass-through stub. When domain crates implement custom
//! serializers for replication, they use this attribute to skip Entity
//! references and other local-only data.

use proc_macro::TokenStream;

/// Pass-through stub: does nothing now, honored by custom serializers later.
///
/// # Example (future usage in domain replication submodules)
/// ```ignore
/// // In lunco-fsw/src/replication.rs:
/// app.replicate::<Wire>()
///     .set_serialization(
///         // serialize: skip #[network(ignore)] fields
///         |writer, wire| /* ... */,
///         // deserialize: skip #[network(ignore)] fields
///         |reader, map| /* ... */,
///     );
/// ```
#[proc_macro_attribute]
pub fn network(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}
```

**File: `crates/lunco-networking/src/lib.rs`**

Re-export the macro:

```rust
pub use lunco_networking_macro::network;
```

---

## What NOT To Change

| Thing | Skip Because |
|---|---|
| USD loading code | Observer catches entities after spawn. Zero changes needed. |
| Sandbox spawn code | Observer catches entities after spawn. Zero changes needed. |
| `Wire` component fields | Local join within one process. Per-field serialization handled in domain's replication submodule. |
| `FlightSoftware.port_map` | Local port map. Serialization handled in domain's replication submodule. |
| `ControllerLink.vessel_entity` | Avatar and vessel always in same World. |
| `PendingWheelWiring` fields | Temporary local reference during USD loading. |
| `FrameBlend` fields | Camera animation, same World. |
| `AuthorizedCommand` event | Needs auth layer to create it meaningfully. `CommandMessage` is sufficient until then. |
| `EditEvent` / `EditLog` | Needs recording systems, LamportClock. Adding empty types creates dead code. |
| `AuthRegistry` / `Session` impl | No auth implementation yet. `SessionId` type is enough for domain crates to reference. |
| `NetworkAuthority` component | Needs possession negotiation systems. Add when Phase 2 starts. |
| `CrossDomainWire` | Only needed when processes actually split. |

---

## Summary of Changes

| Crate | File | Change | Lines |
|---|---|---|---|
| **lunco-core** | `lib.rs` | Add GlobalEntityId, SessionId, Role, Domain types | ~40 |
| **lunco-core** | `lib.rs` | Add Clone+Copy+Reflect to Avatar, Vessel, RoverVessel, SelectableRoot, Ground | ~15 |
| **lunco-core** | `architecture.rs` | Add Clone+Copy+Reflect to ControllerLink | ~2 |
| **lunco-core** | `Cargo.toml` | Add `serde` dependency | ~1 |
| **lunco-networking** | `lib.rs` | Observer: `On<Add<Replicated>>` → GlobalEntityId | ~10 |
| **lunco-networking** | `Cargo.toml` | Add `ulid` dependency | ~1 |
| **lunco-networking** | `lib.rs` | Re-export `#[network]` macro | ~2 |
| **lunco-networking** | `Cargo.toml` | Add proc-macro crate reference | ~1 |
| **lunco-networking** | `macro/Cargo.toml` | New proc-macro crate | ~8 |
| **lunco-networking** | `macro/src/lib.rs` | Pass-through stub macro | ~15 |

**Total: ~95 lines across 6 files. Zero behavior changes. Zero changes to domain spawn sites.**

---

## What This Enables Later

After these changes:

1. **GlobalEntityId type exists** in lunco-core, ready for any crate to use
2. **Observer assigns it automatically** when bevy_replicon marks entities as replicated
3. **Session/Role/Domain types exist** — domain crates can reference them
4. **`#[network(ignore)]` stub compiles** — domain crates can annotate fields

When Phase 1 (transport + replication) starts:
- Add `bevy_replicon` as optional dependency to each domain crate
- Add `replication.rs` submodules to domain crates
- Wire replication plugins in multiplayer binary
- Observer auto-assigns GlobalEntityId to replicated entities
- **Zero changes to USD loading, sandbox spawning, or domain systems**

When process splitting starts:
- Implement `DomainRouter`, `CollabServer` in lunco-networking
- Domain replication submodules handle per-field serialization
- **No changes to Wire, port_map, or any component field types**

**Nothing needs to be undone or refactored.** The foundation is laid now; features are bolted on later.
