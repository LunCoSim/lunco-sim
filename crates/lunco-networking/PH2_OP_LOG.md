# Phase 2 scope — M3 op-log over the sync layer

**Goal (from IMPLEMENTATION_PLAN):** ship the existing typed `#[Command]` mutations
over the network, reliable + ordered, routed by `SyncChannel`. First phase that
links lightyear behind the `networking` feature (D7). No smooth motion yet — that's
M2/M4 (Ph3/Ph4).

**Verify:** two clients connect; each `SpawnEntity`s a rover (both appear on **both**
peers with **distinct** ids); each possesses **its own**; a possess targeting another
session's rover is rejected. Reliable, no interpolation, no motion yet.

**Ph2 = stages 1–4 of the MVP scenario** (connect → identity → create → possess). Stage 5
("drive only mine, everyone sees it") needs Ph3 (motion) + Ph4 (prediction). Full arc +
the 5 gaps (G1–G5): [`MVP_MULTIPLAYER_GAPS.md`](MVP_MULTIPLAYER_GAPS.md).

---

## Reality check (what the plan assumed vs. what's actually there)

Grounded in the current code (2026-05-29):

| Plan assumed | Actual state | Ph2 must |
|---|---|---|
| `declare_channel` routes by `SyncChannel` | `SyncChannel` enum exists (`commands.rs`, renamed from `Replication`) but is **declared-only, never consulted**; no per-command metadata anywhere | **build** the channel-metadata registry |
| commands carry `Mutation<P>` | commands fire **bare** via `commands.trigger(Event)` → `On<T>` observer; `Mutation<P>` envelope exists but wraps nothing today | **wrap** at the seam |
| "serialize any command" | each `#[Command]` derives `Serialize+Deserialize+Reflect`, but there is **no unified codec** — only per-request reflect-deserialize in `api_command_dispatcher` | **build** a command codec (reuse Reflect + shared `TypeRegistry`) |
| `lunco-networking` wired in | crate is a **no-op skeleton**, no lightyear dep, feature flags commented out, not added to any binary | **add** lightyear + plugin + wire one binary |

Good news — the substrate is genuinely ready: `#[Command]` structs are already
`Serialize/Deserialize/Reflect` (`lunco-command-macro` lines 147–159), `Mutation<P>`/
`OpId`/`SessionId` are sync-shaped (`commands.rs`), `ApiEntityRegistry` resolves
**both** directions (`registry.rs:17–31`), and Ph1 gives us `GlobalEntityId`/
`IsServer`. So Ph2 is plumbing, not new domain logic.

---

## The single seam

`crates/lunco-api/src/executor.rs:92–137` — `api_command_dispatcher`:
JSON → `ApiCommandEvent` → reflect-deserialize → `reflect_event.trigger(world, instance)` → `On<T>` observer.

- **Incoming** (apply a sync mutation): we already have most of it — `api_command_dispatcher`
  deserializes a `(short_type_name, params_json)` into a typed event and triggers it,
  and `resolve_ids_in_json` (`executor.rs:145–180`) maps `GlobalEntityId→Entity`. The
  network receiver feeds the same path.
- **Outgoing** (capture a local mutation to send): the inverse doesn't exist. We capture
  at the command boundary, serialize via Reflect, and map `Entity→GlobalEntityId`
  (inverse of `resolve_ids_in_json`).

---

## Remote calls: our commands **are** RPC (and that's enough — with one gap)

A `#[Command]` is a typed Bevy event + observer; add a sync envelope + reliable
channel + server validation and you have, by definition, a remote procedure call.
So Ph2 doesn't add a *separate* RPC system — it gives the command we already have a
sync layer. The real question is whether the command/op-log covers all the *shapes* of
remote call we need.

**Reference models.** Unreal has three RPC directions × reliable/unreliable +
validation: `Server` (client→server, caller **must own** the actor or it's dropped;
`WithValidation` kicks cheats), `Client` (server→the owning client, targeted),
`NetMulticast` (server→all). Hard doctrine: **RPCs are edge-triggered events, not
state** — late-joiners never see past RPCs; persistent things go through *replicated
properties*. Even movement is RPC-shaped (`ServerMove` input + `ClientAdjustPosition`
correction). lightyear has no such decorators: it offers **Messages/Triggers over
reliable/unreliable Channels**, targeted at *send* time (to a peer, or broadcast),
plus recent **networked events/triggers** (a Bevy observer fired on the remote — the
same shape as our `#[Command]`). Our M3 maps onto a `Mutation<SyncCommand>` message
(or networked trigger) on one `OrderedReliable` channel.

**Coverage — the call shapes, mapped:**

| Shape | Unreal | Ours | Ph2 status |
|---|---|---|---|
| client → server request | `Server` RPC | M3 command | ✅ core |
| server → all broadcast | `NetMulticast` | M3 broadcast after validate | ✅ |
| server → originator response | return / `Client` | `Ack`/`Reject` | ✅ |
| **server → specific client, unsolicited** (kick, "possession denied", toast) | `Client` RPC | `Mutation` with **target `SessionId`** | ⚠️ same envelope, needs targeted send — small add, do when a use case appears |
| continuous state (pose) | replicated property | **M2** (Ph3) | ✅ **not** a command, by design |
| high-rate input (drive) | `ServerMove` RPC | **M4** (Ph4) | ✅ separate channel, by design |

**So commands are enough for the discrete authoritative-action category** — and must
**not** be widened to carry state (M2) or input (M4); that mechanism split is the
whole point. The only true gap vs Unreal is **server-initiated targeted
notifications** (Unreal's `Client` RPC): not a new subsystem, just the same `Mutation`
sent to one `SessionId` instead of broadcast. Defer until needed.

**Our op-log is *richer* than a plain RPC:** `OpId` (idempotent dedupe) + `parent_gen`
(causal ordering / optimistic concurrency) make M3 an *event-sourced op-log*, not
fire-and-forget — exactly right for Modelica/USD document edits, which plain Unreal
RPCs can't express. The flip side: for a purely **cosmetic one-shot** (sound, particle,
hit-flash) the full envelope is overkill — the answer there is a *lightweight unreliable
command* (same command type, unreliable channel, no `Ack`), still a channel choice not
new machinery. Don't build it preemptively.

**Authority is orthogonal to the channel (the possession gate).** The static
`SyncChannel` tag picks the channel/reliability; whether *this* client may issue a
command against *that* entity is **runtime, possession-driven** — Unreal drops un-owned
`Server` RPCs, Mirror's `[Command]` `requiresAuthority`, Godot's `@rpc("authority")`.
Ours: the client only emits a command for the entity it possesses, **and the server
validates against the possession map regardless** (defense-in-depth = Unreal
`WithValidation`). `PossessVessel` is the command that *establishes* that authority —
which is why it's Ph2's headline. The Predicted-vs-Interpolated role split (owner
predicts, others interpolate) is downstream of the same possession, and lives in M2
(Ph3), not in the `SyncChannel` tag.

## Possession over the sync layer — identity + authority, not a new verb

The command already exists: **`PossessVessel { avatar: Entity, target: Entity }`**
(`lunco-avatar/src/commands.rs:11`), handled by `on_possess_command`
(`lunco-avatar/src/lib.rs:1233`). It's `Authoritative`, so M3 carries it for free:

```
client fires PossessVessel{avatar,target}
  → Mutation<SyncCommand> on CommandChannel
  → SERVER runs on_possess_command (inserts ControllerLink{vessel_entity:target} on avatar)
  → ControllerLink replicates (M2, Ph3) → every client sees who controls what
```

Possession today is **pure component state** — `ControllerLink` on an `Avatar`
(`lunco-controller/src/lib.rs:43`). Input routing reads it:
`translate_intents_to_commands` queries `(VesselIntentState, ControllerLink)` and emits
`DriveRover{ target: link.vessel_entity }` (`lunco-controller/src/lib.rs:57`). It's
already per-avatar with no global state — multiple avatars can each possess a different
vessel. So nothing about the *verb* needs to change. **Two things the single-process
model never had to answer do:**

### Gap 1 — "which avatar is *mine*?" (the per-session binding)

`on_possess_command` falls back to "first `Avatar` entity" when none is specified
(`lib.rs:1248`). On a network that's wrong: each client must act on *its own* avatar.
We need a **`SessionId → avatar` binding**:

- **On connect (server):** spawn/assign an avatar for that `SessionId`, stamp it
  `Provenance::Authoritative` (server-minted id, D4), record `SessionId → GlobalEntityId`
  in a server-side `SessionAvatars` map.
- **Handshake reply (server→client):** tell the client its own avatar's
  `GlobalEntityId` (rides the late-join handshake, gap I — `scene_id`, sim-tick, warp
  state, **+ your-avatar-id**).
- **On the client:** resolve that id → local `Entity`, store as a `LocalAvatar`
  resource. The client fills `PossessVessel.avatar` from `LocalAvatar` instead of
  "first avatar found."

This is the minimum new state Ph2 possession needs. It's small — one map server-side,
one resource client-side — but it is **not** optional: without it, two clients fight
over the same avatar.

### Gap 2 — "may this client possess this target?" (server validation)

`PossessVessel` is unconditional today. Over the sync layer the **server must validate** in
the P2.4 apply step before broadcasting:

- reject if `target` is already possessed by a *different* session
  (scan `ControllerLink`s, or keep a `target → SessionId` reverse map);
- reject if the inbound `avatar` ≠ the avatar bound to the sender's `SessionId`
  (a client may only possess *through its own* avatar — defense-in-depth, the
  Unreal-`WithValidation` analogue);
- reject if the target isn't possessable.

Rejected → `Reject` back to origin (reuse the existing enum; add a `NotAuthorized`
variant), no broadcast. Accepted → apply + broadcast + `Ack`. **Possession is the
headline validation case** that exercises the whole P2.4 server path — it's not a side
note, it's the demo.

### Net additions for possession (none are a new command)

| Need | What | Where | Phase |
|---|---|---|---|
| who-am-I | `SessionId → avatar` map | server, on-connect | Ph2 (handshake) |
| client knows its avatar | `LocalAvatar` resource (from handshake id) | client | Ph2 |
| client targets own avatar | fill `PossessVessel.avatar` from `LocalAvatar` | client | Ph2 |
| may-I gate | validate sender-avatar + target-not-taken | server P2.4 | Ph2 |
| others see possession | `ControllerLink` via `app.sync` | M2 | Ph3 |
| owner predicts / others interpolate | role split downstream of possession | M2 | Ph3 |

> **Scope note:** the handshake (gap I) is listed in the plan as late-join polish
> (Ph6), but the **minimal your-avatar-id reply is pulled forward into Ph2** because
> possession is meaningless without it. Keep it minimal — just the avatar id + sim-tick
> + scene-id; full snapshot/op-log-checkpoint baseline stays Ph6.

## The four new pieces (and where they live, per D7)

D7 split: **facade + metadata always-on; transport/codec-send/receive behind `feature="networking"`.**

### P2.1 — SyncChannel metadata registry  *(always-on; `lunco-api` or `lunco-core`)*
A resource mapping **command short-type-name → `SyncChannel`**, plus a tiny extension
domain crates call alongside their existing `register_commands!`:

```rust
app.declare_channel::<DriveRover>(SyncChannel::ControlStream);   // Ph4 channel
app.declare_channel::<PossessVessel>(SyncChannel::CommandBus);
// unregistered ⇒ SyncChannel::Local (safe default — never hits the sync layer)
```

- Always compiled; with the feature off it just fills a `HashMap` nobody reads (cheap,
  keeps domain crates `#[cfg]`-free).
- Keyed by short type name to match `TypeRegistry::get_with_short_type_path` already
  used in dispatch.
- **Don't** touch the `#[Command]`/`#[on_command]` macros — keep this a plain runtime
  call so it's grep-able and macro-independent.

### P2.2 — Command codec  *(always-on types; send/recv behind feature)*
Solve type-erasure by **reusing Reflect + the shared `TypeRegistry`** — *not* a
hand-maintained `AnyCommand` enum (would centralize every domain command in one file,
defeating the decentralized `#[Command]` design).

Sync payload:
```rust
struct SyncCommand { type_name: String, data: serde_json::Value } // Reflect-serialized
// shipped inside Mutation<SyncCommand> { id: OpId, origin: SessionId, parent_gen, payload }
```
- **Serialize:** `TypedReflectSerializer` on the triggered event → `data`; `type_name` =
  short path. (Mirror of the deserialize already in `api_command_dispatcher`.)
- **Deserialize:** exactly today's `api_command_dispatcher` path → trigger `On<T>`.
- Both peers share the same `TypeRegistry` (same registered commands) — that *is* the
  protocol. JSON for Ph2 (debuggable); swap to bincode in the Ph6 compression item.
- **Entity↔GlobalEntityId at the boundary:** receiver already converts via
  `resolve_ids_in_json`; add the inverse on send (`api_id_for`, same heuristic field
  names: `target`/`entity`/`body`/`parent`/`avatar`). ⚠️ the field-name heuristic is
  fragile — see Risks.

### P2.3 — lightyear transport + protocol  *(behind `feature="networking"`)*
- Add `lightyear = { version = "0.26.4", optional = true }`; `networking = ["dep:lightyear", ...]`.
- One reliable channel for Ph2:
  ```rust
  app.add_channel::<CommandChannel>(ChannelSettings { mode: OrderedReliable(..), .. })
     .add_direction(Bidirectional); // client→server requests, server→client broadcasts
  ```
- Register `Mutation<SyncCommand>` as a lightyear **message** on that channel.
- `SyncChannel::CommandBus` → `CommandChannel`. `ControlStream` → **deferred to Ph4**
  (INPUT channel, unreliable+redundant). `Local` → never sent.
- Set Ph1's `IsServer` from the plugin (host/server = `true`, client = `false`).

### P2.4 — send/apply systems + dedupe  *(behind feature)*
- **Send:** capture locally-originated `CommandBus` commands, wrap in `Mutation`
  (`Mutation::local` / `local_against(parent_gen)` already mint `OpId`/`SessionId`),
  Entity→id, push on `CommandChannel`.
- **Server role:** receive client mutation → validate (authority/`parent_gen`; reuse
  `Reject::StaleParent`) → apply locally → **broadcast** to other clients → `Ack` origin.
- **Client role:** receive server mutation → dedupe → apply (trigger event).
- **Dedupe:** a recently-seen `OpId` set (ring/`HashSet` with cap); duplicate ⇒
  `Reject::Duplicate` (already defined), idempotent skip.
- **SessionId:** server assigns one per connection; stamped on inbound mutations for
  attribution.

---

## Capture strategy — the one genuinely tricky bit

Commands fire bare today (`commands.trigger(DriveRover{..})`), and an `On<T>` observer
can't generically "see all commands." Two options:

- **(A) Global observer per replicated command** — `declare_channel::<C>` also adds
  an observer `On<C>` that serializes+enqueues. Clean, but fires for *every* trigger
  incl. ones already arriving from the sync layer → needs an "originated-remotely" guard to
  avoid echo loops (a `RemoteApply` marker/`SessionId != LOCAL` check).
- **(B) Route locally-originated commands through an explicit `apply()` entry** that
  both triggers and enqueues — closer to the `Mutation` envelope's original intent, no
  echo problem, but requires call sites to use it.

**Recommend (A)** with an echo-guard: least disruption to existing call sites, and the
guard is a one-liner (skip enqueue when the current apply originated from the network).
Validate the guard in a Tier-2 test (loopback must not re-broadcast).

---

## Commands to flow in Ph2 (pick the cheap, observable ones)

| Command | Crate | SyncChannel | Note |
|---|---|---|---|
| `PossessVessel` / `ReleaseVessel` | lunco-avatar | **CommandBus** | ownership change — the headline demo |
| a `ParameterChanged`/`MoveEntity` | lunco-sandbox-edit | **CommandBus** | `parent_gen` validation exercised |
| `SpawnEntity` (runtime) | lunco-sandbox-edit | **CommandBus** | **CORE** — stage 3 of the scenario; see G2 ↓ |
| `DriveRover`/`BrakeRover` | lunco-mobility | ControlStream | **declare now, route in Ph4** |

**Runtime-spawn = CORE this phase (it's "create a rover"), and B.1 must be fixed here (G2 /
DESIGN_GAPS §B.1 — no longer deferrable).** For the client to *see a spawned entity appear*
via the op-log, the server allocates its `GlobalEntityId` and the broadcast mutation must
**carry that id** so peers converge. The catalog rovers are USD assets (`skid_rover.usda`),
so the naive "Content-instanced spawns need no id, they converge deterministically" path is
**wrong for runtime instances**: two spawns of the same asset derive the **same** Content id
→ collision. Therefore:

- **Runtime-spawned rovers get `Provenance::Authoritative`** (server-allocated unique root,
  id in the envelope) **+ `Derived` children — NOT `Content`.** Geometry still loads locally
  from the shared USD on each peer (no streaming).
- The USD loader's unconditional `Content` stamp is right for **startup-scene** prims but must
  be **suppressed for runtime subtrees**; the spawn path stamps `Authoritative`+`Derived`
  instead. (Today `spawn.rs` stamps nothing and relies on the loader → would silently collide.)

The spawned entity's *pose* still doesn't replicate until Ph3 (M2) — at Ph2 it appears at its
spawn position and stays put. The Ph2 demo is **two clients each spawn + possess their own
rover** (stages 1–4), not just parameter change.

---

## Explicitly deferred to later phases
- Optimistic client apply + reconciliation → Ph4 (Ph2 can be server-authoritative-apply
  only; correctness over latency).
- `ControlStream`/INPUT channel, jitter buffer, redundancy → Ph4.
- Tick-stamping mutations in `SimTick` → not needed for reliable-ordered; M6 drive is Ph3/Ph4.
- bincode/compression → Ph6.
- Multi-transport (`TRANSPORT_ABSTRACTION.md`) — Ph2 uses one WebTransport+memory server;
  full UDP/WS fan-out is a later transport item.

---

## Tier-2 tests (headless, lightyear memory/crossbeam transport — no real net)
Per `NETWORKING_TEST_PLAN.md`:
1. **command-arrives** — client sends `PossessVessel`, server applies, state changes.
2. **server-broadcasts** — a second client sees the change.
3. **dedupe** — same `OpId` twice ⇒ applied once (`Reject::Duplicate`).
4. **id-resolves** — `Entity` field round-trips Entity→GlobalEntityId→Entity across the boundary.
5. **no-echo** — a sync-applied command does **not** get re-broadcast (validates the capture guard).
6. **stale-parent** — `parent_gen` mismatch ⇒ `Reject::StaleParent`, no apply.
7. **two-clients-spawn (G2 guard)** — A and B each `SpawnEntity("skid_rover")`; assert the
   two roots get **distinct** `GlobalEntityId`s (the B.1 collision regression guard) and both
   entities exist on both peers.
8. **possession-isolation (G4)** — A possesses rover-A, B possesses rover-B; a `PossessVessel`
   (or, in Ph4, `DriveRover`) from A targeting rover-B is **rejected** server-side.

---

## Risks / open items
- **Entity-field heuristic** (`resolve_ids_in_json` field-name matching) is fragile for
  serialization both ways. If a command names an entity field something unlisted, it
  silently won't translate. Consider a typed newtype (`NetEntity`) or a `#[net_entity]`
  field attribute as a follow-up; for Ph2, extend the name list + add a debug assert
  that no raw `Entity::to_bits` value escapes to the sync layer.
- **Reflect-serialize fidelity** — confirm `TypedReflectSerializer`/`Deserializer`
  round-trips every Ph2 command (esp. `Entity`, `f64`, enums) before relying on it.
- **Echo loop** — the capture guard is load-bearing; test #5 must pass before any broadcast.
- **`declare_channel` placement** — `lunco-core` (next to `SyncChannel`) vs `lunco-api`
  (next to the dispatcher it feeds). Lean `lunco-api`: it already owns dispatch + the
  `TypeRegistry` view, and `lunco-core` shouldn't grow a command-routing concept.

---

## Suggested build order
1. P2.1 `declare_channel` + registry (always-on) — domain crates annotate, no behavior change.
2. P2.2 codec types + Reflect ser/de round-trip test (no transport).
3. P2.3 lightyear dep + `networking` feature + plugin + `IsServer`, one binary.
4. P2.4 send/apply/dedupe + echo-guard; Tier-2 tests 1–6.
5. Demo: host possess → client sees it (the verify gate).

Each step builds + (1–2) test green before the next; `-j2`, no broad sweeps.
