# Transport Abstraction — one backend, multi-transport server

Goal: **one server, many transports at once**, so a desktop client (UDP) and a
browser client (WebTransport) connect to the same world and see each other. We
**pick one backend and stick with it** — no backend-swap abstraction. The only
things that vary are the *sync layer* (UDP/WebTransport/WebSocket/memory) and the
*role* (server/client/host).

The guiding principle (already the PREP.md rule): **domain crates never import the
networking backend.** They speak only our semantic API. All transport/backend
specifics live inside `lunco-networking`. This keeps domain code clean — and as a
free side effect, *if* we ever did swap backends it'd be contained — but that is
not a goal we design or test for.

---

## The one idea that makes it work: erase the transport at the session boundary

```
  native client ──UDP──────────┐
  browser client ──WebTransport─┤
  browser client ──WebSocket────┤──▶  [ accept ]  ──▶  Peer { SessionId }  ──▶  replication + commands
  host's own client ──Memory────┘                       (transport-erased)        (backend-agnostic)
```

Above the `Peer` boundary, **nothing knows or cares which transport a client used.**
Replication fans out to all peers; a UDP desktop player and a WebTransport browser
player are in one world. The transport kind survives only as a diagnostic tag.

Both candidate backends already merge multiple transports into one connection set
(renet2: multiple server sockets → one `RenetServer`; lightyear: multiple server
links → one replication room). Our facade just exposes that uniformly.

---

## Layer 1 — Transport selection (config, not plumbing)

We do **not** re-implement netcode. We select and configure what the backend opens.

```rust
/// One sync protocol. Selection-level only.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TransportKind {
    Memory,        // in-process: listen-server host's own client, integration tests
    Udp,           // native ↔ native, lowest latency (no browser)
    WebTransport,  // QUIC/TLS — browsers AND native
    WebSocket,     // TCP (ws:// dev / wss:// prod) — browser fallback
}

/// SERVER opens many at once → one session pool.
#[derive(Resource, Default)]
pub struct ServerListeners(pub Vec<Listener>);

pub struct Listener {
    pub kind: TransportKind,
    pub bind: SocketAddr,
    pub tls: Option<TlsConfig>,   // required for WebTransport + wss
}

/// CLIENT opens exactly one — platform-appropriate.
#[derive(Resource)]
pub struct ClientConnect {
    pub kind: TransportKind,
    pub server: ServerAddr,       // URL (web transports) or SocketAddr (udp)
    pub cert: CertTrust,          // CaTrusted | Hashes(Vec<[u8;32]>) | InsecureDevWs
}
```

Typical wiring:
- **Native host (also a player):** `ServerListeners([Memory, Udp, WebTransport, WebSocket])` + a local `Memory` client.
- **Desktop joiner:** `ClientConnect { Udp, .. }` (or WebTransport).
- **Browser joiner:** `ClientConnect { WebTransport, .. }` (or `WebSocket` fallback).

---

## Layer 2 — Peer identity (transport-erased)

```rust
/// The only connection handle domain code ever sees.
#[derive(Component, Clone, Copy)]
pub struct Peer {
    pub session: SessionId,    // lunco-core::commands::SessionId
    pub kind: TransportKind,   // diagnostics/UI only — never branch domain logic on it
}
```

`ConnectionId` (backend-opaque) → `SessionId` (ours) → `GlobalEntityId` (the
entities that peer controls). The first two hops live in the facade; domain code
starts at `SessionId`/`GlobalEntityId`, both already in `lunco-core`.

---

## Layer 3 — Logical channels (semantic, mapped to backend)

Domain code addresses channels by intent, not by transport feature:

```rust
pub enum Delivery { ReliableOrdered, ReliableUnordered, Unreliable }

pub struct Channel { pub name: &'static str, pub delivery: Delivery }

// Canonical set:
//   INPUT      Unreliable        rover throttle/steer @60Hz  (Mutation, ControlStream)
//   COMMANDS   ReliableOrdered   possess, spawn, scene edits (Mutation, CommandBus)
//   SNAPSHOTS  Unreliable        replicated state deltas
//   BULK       ReliableUnordered ephemeris, cosim history, asset sync
```

**Honest caveat (drives the abstraction's fallback rule):** WebSocket is TCP — it
has **no unreliable mode**. So `Unreliable` channels *degrade to reliable* for
WebSocket peers (more latency under packet loss, still correct). UDP/WebTransport
get true unreliable datagrams. The facade encodes this per-transport fallback so
domain code can always ask for `Unreliable` and get the best each peer supports.

---

## Layer 4 — What domain crates declare (the whole public surface)

```rust
// In a domain crate's replication submodule (feature-gated, never imports a backend):
app.replicate::<Transform>();                              // state sync
app.replicate::<DifferentialDrive>();
app.declare_channel::<DriveRover>(SyncChannel::ControlStream);   // ties #[Command] + Mutation envelope to a channel
app.declare_channel::<PossessVessel>(SyncChannel::CommandBus);
```

`declare_channel` is the bridge from the **existing** `#[Command]`/`Mutation<P>`
layer to the sync layer: it picks the transport channel from the `SyncChannel` tag
(`ControlStream`→INPUT, `CommandBus`→COMMANDS), serializes the envelope, and
resolves `GlobalEntityId`↔`Entity` at the boundary via `ApiEntityRegistry`. No new
command types — reuses what `lunco-api` already dispatches.

That's the entire domain-facing API: `replicate::<T>()`, `declare_channel::<C>()`,
read `Peer`/`SessionId`. Transports, certs, channels, backend — all invisible.

---

## Layer 5 — Compile-time transports + role (one backend, always linked)

```toml
[features]
# transports — additive; gate per platform
transport-memory       = []   # always available
transport-udp          = []   # native only
transport-webtransport = []   # native + wasm
transport-websocket    = []   # native + wasm

# roles
server = []   # opens ServerListeners
client = []   # opens ClientConnect
host   = ["server", "client", "transport-memory"]  # listen-server
```

Build profiles (the chosen backend is a normal dependency, not feature-gated):
- **Native host:** `host, transport-udp, transport-webtransport, transport-websocket`
- **Desktop client:** `client, transport-udp, transport-webtransport`
- **Web client (wasm):** `client, transport-webtransport, transport-websocket` *(no udp — won't compile on wasm)*

**Transport swap = flip a feature / edit `ServerListeners`.** No code change.

---

## Which backend (pick one, then commit)

The multi-transport-server requirement is satisfied by both candidates, so it
doesn't force the choice — the spike (STACK_COMPARISON §2–3) decides on prediction
quality + host robustness:
- **renet2 + replicon** is purpose-built for heterogeneous clients on one server —
  Layer 1's multi-listener maps almost directly onto its multiple server sockets.
- **lightyear** supports multiple server transports into one replication room *and*
  gives prediction/rollback for free (Layer 4 commands feed its input prediction) —
  the current lean.

Once chosen, it's a plain dependency. Layers 1–4 are *our* types regardless, so
domain crates stay backend-clean either way — but we build and test against the one
backend only.

---

## Dev-vs-prod transport, concretely (ties to the LAN-dev tiers)

| Phase | Server listeners | Browser client | Desktop client | Certs |
|---|---|---|---|---|
| Solo dev | `[Memory, Udp]` | — | Memory/UDP | none |
| Early LAN | `[Udp, WebSocket]` | `ws://` | `Udp` | **none** (http page + ws://) |
| QUIC LAN | `[Udp, WebTransport, WebSocket]` | `WebTransport` (wss page) | `Udp` | mkcert CA on both machines |
| Prod | `[WebTransport, WebSocket]` (+Udp if native clients) | `WebTransport` | `Udp`/`WebTransport` | real CA |

Same binary, same domain code — only `ServerListeners` + the feature set differ.
