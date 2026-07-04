# Sync architecture — how *everything* stays in sync, by design

The capstone. The other docs cover pieces (identity, transport, gaps). This one
answers the whole question: **across all possible cases, how is state kept
consistent — derivably, not feature-by-feature?**

> **Thesis:** you never choose a sync mechanism per feature. You classify each
> piece of state along four axes; the axes *determine* the mechanism. There are
> only **seven mechanisms**, and every syncable thing in the system maps to exactly
> one. "Synced by architecture" = the classification is declared (often implied by
> provenance), enforced by design, and the runtime routes accordingly. Nothing is
> hand-wired, and nothing can leak onto — or fall off — the sync layer by accident.

---

## 1. The four classifying axes

Every datum answers four questions. The answers are mostly *forced* by what the
data is, so classification is near-automatic.

1. **Provenance** (see README → *Entity Identity Mapping*): `Content` · `Derived` · `Authoritative` · `Local`.
2. **Temporal character**: `Static` · `Continuous` (high-rate) · `Discrete` (events) · `ConcurrentText`.
3. **Authority**: `Server` · `ClientOwned(+validate)` · `Shared(CRDT)` · `LocalOnly`.
4. **Receiver-computability**: `Reconstructible` (from shared content) · `Predictable` (from local inputs) · `Opaque` (must be received).

These aren't free-floating — they interlock. `Content` ⇒ `Reconstructible`.
`Local` ⇒ `LocalOnly` ⇒ never synced. Server-only cosim forces ⇒ `Opaque` ⇒ can't
be predicted. The classification mostly falls out of provenance + "does it change,
and can the receiver reproduce it?"

---

## 2. The seven mechanisms (the complete set of "how")

| # | Mechanism | Direction / guarantee | Convergence |
|---|---|---|---|
| **M1** | **Content reconstruction** — *not transmitted*; every peer loads the same content-addressed source and derives identical entities, ids, wiring | none (zero bytes) | bit-identical baseline |
| **M2** | **State replication (snapshot)** — server→clients, periodic, last-writer-wins; receiver-role = **Predicted** (autonomous proxy) or **Interpolated** (simulated proxy) | unreliable + delta | LWW convergence |
| **M3** | **Command / op-log** — reliable, ordered `Mutation<P>`; server validates, stamps `OpId`, broadcasts; event-sourced | reliable, total order | convergent replay |
| **M4** | **Input stream** — client→server, high-rate, unreliable + redundant, sim-tick-stamped; the basis of prediction | best-effort, redundant | (lossy ok) |
| **M5** | **CRDT** — concurrent structured/text docs (yrs/Yjs) | reliable, commutative | CRDT merge |
| **M6** | **Shared clock** — server-authoritative sim-tick + dt + warp/pause; the substrate every other channel is stamped in | reliable + estimated offset | tick alignment |
| **M7** | **Local-only** — never synced; owned/recomputed per peer | — | n/a |

Seven. That's the whole vocabulary. Everything below is *routing*.

---

## 3. The complete case matrix (every syncable thing → one mechanism)

| State | Provenance | Temporal | Computability | Mechanism |
|---|---|---|---|---|
| Entity identity | Content/Derived | Static | Reconstructible | **M1** |
| …of runtime entity | Authoritative | Discrete | Opaque | **M3** (existence) |
| Content entity existence/topology | Content | Static | Reconstructible | **M1** |
| Cosim wiring (`SimConnection`) | Content (USD) | Static | Reconstructible | **M1** |
| Asset files (USD/glTF/meshes) | Content-addressed | Static | Reconstructible (fetch by hash) | **M1** |
| Pose of **rover you drive** `(CellCoord,Transform)` | any | Continuous | **Predictable** (local avian) | **M2-Predicted** |
| Pose of **other** rovers | any | Continuous | partial | **M2-Interpolated** |
| Pose of **cosim-driven** body (balloon) | any | Continuous | **Opaque** (server-only forces) | **M2-Interpolated** |
| Velocities / forces | any | Continuous | predicted only | **M2** |
| Cosim outputs (thermal, buoyancy) | Derived from content model | Continuous-slow | Opaque | **M2** (low-rate) |
| Control intent (throttle/steer) | — | High-rate | — | **M4** |
| Possess / Release | — | Discrete | Opaque | **M3** |
| Scene edit (prim add / reparent / attr) | — | Discrete | Opaque | **M3** (or **M5** if concurrent free-edit) |
| Parameter change (inspector) | — | Discrete | Opaque | **M3** |
| Modelica `.mo` source | Content baseline + live edits | ConcurrentText | — | **M1** baseline + **M5** edits |
| Sim clock / dt / time-warp / pause | — | Continuous tick | — | **M6** |
| Ownership / `NetworkAuthority` | — | Discrete | Opaque | **M2** (replicated component) or **M3** |
| Session / roles / presence | — | Discrete | Opaque | **M3** + presence list via **M2** |
| Awareness (others' cursors/cameras) | — | High-rate, soft | — | **M4**-style ephemeral |
| Edit history / undo log | — | Discrete | — | **M3** (the op-log *is* M3) |
| RNG seed | — | Static | — | **M6/M3** (seed once, then deterministic) |
| Camera / selection / panels / gizmo | Local | — | — | **M7** |
| Meshes/materials/interp buffers | Derived-local | — | Reconstructible (recompute) | **M7** |

This table *is* the completeness claim: enumerate the state, classify, route. There
is no row without a mechanism, and no row needs a *new* mechanism.

---

## 4. The tick pipeline — how the mechanisms compose (this is the "synced" part)

The mechanisms don't run independently; they layer in a fixed per-tick order so the
world stays coherent. Everything is stamped in **M6** sim-ticks, never wall-clock.

**Server tick T:**
1. **M6** advance sim-clock (respect warp/pause).
2. **M4** drain client inputs destined for tick T from the per-client jitter buffer.
3. **M3** apply + validate authoritative commands for T; assign `OpId`; append to op-log.
4. Step **cosim** (consume async Modelica results) → step **avian** → authoritative state for T.
5. **M2** capture snapshot, emit deltas (predicted-eligible + interpolated) + **M3** acks + **M6** clock beacon.

**Client tick T (running ~RTT/2 + jitter ahead, per M6):**
1. **M6** sync clock to server, adjust local rate.
2. **M4** sample local input → send (redundant) **and** apply to **M2-Predicted** entities locally (move now).
3. **M2** on snapshot: reconcile predicted (smooth error-correct, *not* full avian rollback); push interpolated into the snapshot buffer.
4. **M3** apply received commands in `OpId` order (idempotent via `OpId` dedupe); **M5** apply CRDT updates (any time, commutative).
5. **M1** content is loaded at join / on content-edit — *never* per tick.
6. **Render** (decoupled): interpolate **M2-Interpolated** at `now − interp_delay`; draw **M2-Predicted** at `now`; **rebase all poses into this client's floating origin** (big_space cell+offset → local).

The render-step rebasing is where gap A lives: M2 carries `(CellCoord, Transform)`;
each client maps it into its *own* origin. Identity (M1) and coordinates (M2) are
orthogonal, so a content entity keeps its derived id while its cell+offset stream.

### 4.1 M2-Predicted — Client Prediction Status

The abstract "reconcile predicted (smooth error-correct, not rollback)" above is
realised on the client as **physics-space error reduction** (Fiedler / Rocket
League model). Canonical as-built summary: README → *Client-Side Prediction* (the
full write-up + debugging story lived in `PREDICT_AND_SMOOTH_PLAN.md` /
`PREDICTION_MEMBERSHIP.md`, now in git history). Shape:

- **Membership (which bodies predict locally).** Three disjoint sets, all client-only:
  - *Owned, actively driven* (`OwnedLocally`, computability rule + drive-grace):
    input-replay predicted, reconciled against the acked input seq.
  - *Predicted props & all remote rovers* (`PredictedDynamic`): run local avian
    `Dynamic`, **state**-reconciled per snapshot (no input seq). Remote rovers are
    predicted so they **yield** to a local push (mutual push), not just push.
  - *Everything else* (interpolated proxies): kinematic, **velocity-driven** toward
    the snapshot curve each tick (not teleported), so their motion enters contact
    resolution. Cosim-opaque bodies (`NotPredictable`) are never predicted (Gap C).
- **Correction = physics-space, never render-space.** A reconciler that diverges
  does **not** seat the pose or touch `Transform`. It parks the delta in a
  `PendingCorrection` component; `drain_pending_corrections` (FixedUpdate, pre-solve)
  bleeds it into avian `Position`/`Rotation` at a hard cap (≤2.5 cm / ≤0.9° per tick,
  τ≈0.12 s). It flows through solve → writeback → render interpolation = a smooth,
  contact-safe slide. Only a gross desync (>6 m) seats directly (real teleport).
- **★ LOAD-BEARING CONSTRAINT — never write `Transform` from game code.** The client
  enables avian `PhysicsInterpolationPlugin::interpolate_all()`, so
  `bevy_transform_interpolation` owns every `Transform` at render rate and treats
  any external `Transform` write as a teleport that **resets that body's easing**.
  Writing `Transform` in netcode (reconcile, smoothing, snapshot-apply) silently
  disables render interpolation for that body → raw fixed-rate stepping = jitter
  visible *only on the client* (the host never reconciles). All M2 correction must
  therefore go through `Position`/`Rotation`/velocity, never `Transform`. This cost
  a multi-hour debug; it is the single most important client-side sync invariant.
- **Velocity, not pose, drives proxies.** `drive_kinematic_proxies` sets
  `LinearVelocity`/`AngularVelocity` = curve feed-forward + soft correction (capped),
  never a deadbeat `(target−pos)/h` (that spiked ~50 m/s and tunnelled contacts).

**Still open:** M4 input hardening (tick-stamped redundant inputs + host de-jitter
buffer) is specced but unbuilt; it shrinks the corrections above at the source under
real latency (today they only stay *smooth*, not *small*). See PLAN §4.

---

## 5. Why the whole thing converges (the correctness argument)

The world is consistent because **each channel is individually convergent and they
are layered, not entangled**:

- **M1** → bit-identical baseline (content-addressed; same input ⇒ same entities/ids).
- **M6** → all peers agree on *which tick* a fact belongs to.
- **M3** → discrete changes have a server-assigned **total order** + idempotent
  replay ⇒ eventual consistency on structure/ownership/edits.
- **M2** → continuous state is **last-writer-wins** from the single authority ⇒
  any client error is erased by the next snapshot (prediction only hides latency;
  it never owns truth).
- **M5** → text/structured concurrent edits **commute** ⇒ convergence without locks.
- **M4** → intentionally lossy; redundancy + jitter buffer absorb loss; nothing
  downstream trusts it as truth (server re-derives).
- **M7** → excluded from the sync layer, so it *cannot* cause divergence.

Because M2 is LWW-from-authority and M3 is totally-ordered-from-authority, a missed
M2 packet self-heals next snapshot and a missed M3 packet is retransmitted —
**there is no state that can drift permanently.** A periodic state-hash beacon
(cheap, since content is deterministic) detects any bug-induced divergence.

---

## 6. All the edge cases, slotted (no special architecture needed)

| Case | Handled by |
|---|---|
| **Late join** | M6 clock + M1 reload content by id (derive same entities) + M2 full snapshot + M3 op-log checkpoint + M5 doc state-vector |
| **Reconnect** | Late-join, fast-forwarded from last-acked `OpId`/snapshot tick |
| **Packet loss** | M4 redundancy · M2 next snapshot fixes · M3 retransmit · M5 idempotent |
| **Two users edit same thing** | M3 server total-order (LWW per field) or M5 merge — never a raw race |
| **Own a cosim-driven entity** | computability=Opaque ⇒ M2-Interpolated, not Predicted (ownership ≠ predictability) |
| **Time-warp / pause** | M6 broadcast; M4 gated; M2 continues at warped rate; cosim dt scales |
| **Host migration** (listen-server host leaves) | world is fully reconstructible from M1 + M3 checkpoint + last M2 ⇒ a new host can resume (hard; deferred, but architecture *permits* it because nothing is unrecoverable) |
| **Desync detection** | periodic M6-stamped state-hash compare |
| **New content format** | new `ContentLoader` namespace → still M1; zero changes elsewhere |
| **New synced component** | declare its `SyncClass` → routed automatically; undeclared ⇒ stays M7 (fail-safe) |

---

## 7. Enforced by design (the same discipline as identity)

Sync class is **declared, never assumed**, and the default is safe:

1. **Local is the default.** A component does *not* cross the sync layer unless it
   declares a `SyncClass`. Forgetting to declare = it stays local (M7) — fail-safe,
   never an accidental leak or accidental authority.
2. **`SyncClass` is derivable from provenance + the four axes**, so most
   declarations are one line and machine-checkable for contradictions (e.g.
   `Local` + `Server` authority = compile/registration error; `Content` +
   `Authoritative` = error).
3. **A single registration point** (mirroring the identity `on_add` hook):
   `app.sync::<T>(SyncClass::…)` routes T to M1/M2/M3/M5; M4/M6 are
   system-provided; M7 needs no registration. Domain crates call only this — they
   never touch a transport or a backend type (the domain-clean invariant).
4. **Contradiction checks panic in debug.** You cannot register state whose axes
   disagree, can't predict an Opaque entity, can't make a Local component
   authoritative.

So "how is everything synced?" has a one-sentence operational answer: **declare
each component's sync class (usually implied by its provenance); the runtime routes
it to one of seven mechanisms; the tick pipeline composes them in fixed order; each
mechanism is independently convergent — therefore the world converges.**

---

## 8. How this maps to the rest of the design
- **M1** ⇐ README → *Entity Identity Mapping* (provenance-derived ids, content loaders).
- **M2/M4/M6** ⇐ the backend (lightyear ships M2-predicted/interpolated + M6
  tick-sync; this is the standing argument for lightyear). README → *Transport
  Abstraction* is the pipes they ride.
- **M2 local write chokepoint** ⇐ `lunco_core::attach::migrate_to_grid` (commit
  `7e5fddce`). When an applied snapshot or a reconciled prediction changes *which
  grid* a body belongs to (e.g. an SOI crossing), the local `(ChildOf, CellCoord,
  Transform)` triple must land atomically — splitting the writes lets an observer
  see a half-migrated `(parent, cell, local_tf)` and mis-tag the entity (the bug
  that marked rover chassis `RigidBody::Static`). `migrate_to_grid` is the single
  sanctioned path; the workspace `clippy.toml` bans raw `add_child` /
  `set_parent_in_place` to enforce it. M2's `(CellCoord, Transform)` payload writes
  through here, so replicated **orientation now survives grid crossings** (the
  helper preserves `tf.rotation`; it previously reset to identity). The §4 render
  rebasing (gap A) reads the cell+offset this path establishes.
- **M3** ⇐ the existing `#[Command]` + `lunco-core::Mutation<P>` envelope (already
  built — it *is* the op-log).
- **M5** ⇐ the Yjs/yrs plan in the README (Modelica text).
- **M7** ⇐ today's local components (camera, selection) — unchanged.
- The hard project-specific work (the former `DESIGN_GAPS.md` A–D; open items now in
  README → *Known gaps*) lives inside M1 (identity), M2 (big_space pose,
  cosim-eligibility), and M6 (sim clock) — each has a home.

The architecture is complete: four axes, seven mechanisms, one tick pipeline, one
convergence argument, one enforcement rule. Every case in §3 and §6 lands in it
without invention.

---

## 9. Selecting a mechanism for a NEW feature (the procedure)

The seven mechanisms are not novel — they're the field's established patterns,
renamed and unified: M1 = "load the static world locally, replicate only dynamic
actors" (Unreal/Source); M2 = snapshot interpolation + autonomous/simulated proxy
(Valve/Unreal); M3 = reliable RPCs / op-log / event sourcing; M4 = redundant
tick-stamped input backlog (Source `usercmd`, Overwatch, GGPO); M5 = CRDT
(Yjs/Figma); M6 = clock sync (NTP-style offset, client runs ahead); M7 =
local/cosmetic. **What's ours is the routing discipline (classify → route →
enforce), not the mechanisms.**

### The daily surface

Domain authors touch a tiny API — never a socket, backend, or serializer:

```rust
app.sync::<Transform>(SyncClass::Continuous);                   // → M2 (role decided at runtime)
app.sync::<NetworkAuthority>(SyncClass::Discrete);              // → M2 replicated / M3
app.declare_channel::<DriveRover>(SyncChannel::ControlStream);  // → M4/M3 (reuses #[Command])
commands.spawn((MyBundle, Provenance::Content { namespace:"usd", source, path }));  // → M1
// keep something local: do nothing — Local is the DEFAULT (undeclared never crosses the layer)
```

### The procedure (run per *piece of state* a feature introduces — terminates at one mechanism)

```
Step 0 — Does any other peer need it to be the same?
   NO  → M7 (Local). DONE.  (camera, selection, cosmetics, debug overlays)
   YES → continue.

Step 0.5 — Is it a pure function of already-synced state?
   (computable from content + M6 clock + already-replicated state)
   YES → M7, RECOMPUTE locally. DONE.  (don't sync what you can derive)
   NO  → continue.

Step 1 — Provenance (where does the ENTITY come from)?
   Loaded identically from shared content   → existence/id via M1
   Deterministic sub-part of a parent        → M1 (Derived)
   Born at runtime, not derivable            → existence via M3 (server spawns+replicates)

Step 2 — Temporal character of the changing part:
   Never changes after creation      → M1 (if content-borne) else M3 (one reliable command)
   Changes every tick / analog       → M2          (go to Step 3)
   Discrete events / toggles / edits → M3
   Concurrent free text / co-edit    → M5
   High-rate player intent           → M4

Step 3 — If M2: receiver role by COMPUTABILITY (not ownership!):
   Receiver can reproduce from LOCAL inputs (own avian-driven motion) → M2-Predicted
   Receiver cannot (server-only forces, someone else's entity)        → M2-Interpolated

Step 4 — Authority sanity check (reject contradictions):
   Server source of truth         → M2/M3 (default, OK)
   Client owns it                 → M4 + server validation
   No single owner / concurrent   → M5
   Local + "must be authoritative"→ STOP, reclassify (debug-panics anyway)

Step 5 — Declare: app.sync::<S>(class) / register_command / stamp Provenance.
   (M6 you don't choose — it's the substrate everything is stamped in.)
```

### Rules of thumb (the smells)
- **Low latency on your own action** → M4 + M2-Predicted. *Not* M3 (M3 is
  reliable-but-slow — for *correct/ordered*, not *instant*).
- **Must be correct & ordered, not instant** (spawn, edit, possess) → M3.
- **Bulk / static / world geometry** → M1. Streaming something every tick that
  never changes = wrong pick.
- **Predicting something the client can't compute** → wrong; M2-Interpolated.
- **Two users edit the same thing:** one field → M3 (server LWW); free text → M5.
- **Added a component and nothing syncs** → you didn't declare it; default is M7.

### The biggest bandwidth saver: don't sync derived state (Step 0.5)

> Never replicate what is a deterministic function of already-synced inputs.
> Recompute it locally.

Day/night lighting & sun angle (pure function of M6 clock + ephemeris), meshes /
materials / LODs / animation pose, interpolation buffers & smoothed camera, cosim
*wiring* (derived from USD — only cosim *outputs* ride M2) → all **M7 recompute**,
not M2. This is also a correctness win: derived state computed from synced inputs
*can't* disagree.

### Worked examples

| New feature / state | Mechanism |
|---|---|
| Robotic-arm joint angles (you control) | M1 entity + **M2-Predicted** (Interpolated for others) |
| Sample collected → inventory count | **M3** (runtime, discrete, server-auth) |
| Chat message | **M3** (or M5 if a shared editable doc) |
| Positional voice | **M4**-style (likely out-of-band stream) |
| Terrain dig by rover | **M3** op-log (deterministic replay) — *not* M2 heightmap streaming |
| New telemetry from a new Modelica model | M1 wiring + **M2 low-rate (Interpolated)** |
| Day/night lighting | **M7 recompute** (Step 0.5) |
| Procedural rocks scattered by seed | **M1** (seed shared once via M6/M3) |
| "Highlight selected" outline | **M7** (per-peer view) |

No feature ever needs a new mechanism — if it seems to, the state was misclassified
(re-run Step 0.5 and Step 3 first).
