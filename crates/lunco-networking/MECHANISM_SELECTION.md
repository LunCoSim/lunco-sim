# Using the architecture — grounding, daily usage, and selecting mechanisms for new features

Companion to `SYNC_ARCHITECTURE.md`. Answers three things:
1. **How is each mechanism actually done in real networking?** (so this is the
   standard toolkit, just unified — not something we invented)
2. **How does a developer use it day to day?**
3. **When a new feature arrives, how do we pick the mechanism?** (a procedure)

---

## 1. Each mechanism = a standard netcode technique

Our seven mechanisms are not novel; they're the field's established patterns,
renamed and unified under one classification. Grounding each:

| Ours | Established name | Who does it this way |
|---|---|---|
| **M1** Content reconstruction | "Load the static world locally; replicate only dynamic actors." Net-stable names / content GUIDs. | Unreal (level actors aren't spawn-replicated; only dynamic actors are), Source (BSP map loaded, not streamed), every engine. Our addition: **content-addressed ids** so derivation matches across peers. |
| **M2** State replication (Predicted/Interpolated) | "Snapshot interpolation" + property replication; **autonomous proxy vs simulated proxy**. Quantize + delta + relevancy. | Valve/Source (snapshots @tickrate, client interp), Unreal (RepGraph, dormancy, autonomous/simulated proxy), Mirror/Photon. The AAA core. |
| **M3** Command / op-log | Reliable RPCs / events; command pattern; event sourcing. Idempotent via seq/op-id. | Unreal reliable server RPCs, RTS command logs, distributed-systems event sourcing. Our op-log *is* this. |
| **M4** Input stream | "Move replication": redundant input backlog, tick-stamped; jitter buffer. | Source `usercmd` backlog, Unreal `ServerMove` redundant moves, Overwatch command frames, GGPO inputs. |
| **M5** CRDT | Conflict-free replicated data types for concurrent docs. | Google Docs, Figma, Yjs/yrs. Not from games — from collab. |
| **M6** Shared clock | Clock sync / time synchronization; NTP-style offset; client runs ahead. | Source server tick + interp, Overwatch command-frame sync, Glenn Fiedler "Networked Physics", Photon/Mirror NetworkTime. Foundational everywhere. |
| **M7** Local-only | "Client-side / cosmetic / not replicated." | Universal — camera, UI, cosmetics. |

**Takeaway:** any networking engineer will recognize all seven. What's ours is the
**routing discipline** (classify → route → enforce), not the mechanisms.

---

## 2. How a developer uses it (the daily workflow)

The whole point is that domain authors touch a *tiny* surface. They never see a
socket, a backend, or a serializer.

**To add a networked component:**
```rust
// In a domain crate's replication submodule. Backend types never imported.
app.sync::<Transform>(SyncClass::Continuous);          // → M2 (role decided at runtime by ownership/computability)
app.sync::<NetworkAuthority>(SyncClass::Discrete);     // → M2 replicated component / M3
app.declare_channel::<DriveRover>(WireChannel::ControlStream);   // → M4/M3 (reuses existing #[Command])
```

**To add an entity from content:** stamp provenance (the loader does this) —
identity, existence, and M1 routing are automatic:
```rust
commands.spawn((MyBundle, Provenance::Content { namespace: "usd", source, path }));
```

**To keep something local:** do nothing. **Local is the default** — an undeclared
component never crosses the wire. This is the fail-safe: you can't leak state by
forgetting, only by explicitly declaring.

**What you never do:** open a connection, choose a channel, write a serializer,
branch on `TransportKind`, or assign a `GlobalEntityId`. Those live in
`lunco-networking` behind the registration calls.

---

## 3. Selecting a mechanism for a NEW feature (the procedure)

Run this for each *piece of state* a feature introduces. It terminates at exactly
one mechanism.

```
Step 0 — Does any other peer need it to be the same?
   NO  → M7 (Local). DONE.  (camera, selection, cosmetics, debug overlays)
   YES → continue.

Step 0.5 — Is it a pure function of already-synced state?
   (computable from content + M6 clock + already-replicated state)
   YES → M7, RECOMPUTE locally. DONE.  (don't sync what you can derive — see §4)
   NO  → continue.

Step 1 — Provenance (where does the ENTITY come from)?
   Loaded identically from shared content        → existence/id via M1
   Deterministic sub-part of a parent            → M1 (Derived)
   Born at runtime, not derivable                → existence via M3 (server spawns+replicates)
   (then classify its CHANGING state in Step 2)

Step 2 — Temporal character of the changing part:
   Never changes after creation     → M1 (if content-borne) else M3 (one reliable command)
   Changes every tick / analog      → M2          (go to Step 3)
   Discrete events / toggles / edits→ M3
   Concurrent free text / co-edit   → M5
   High-rate player intent          → M4

Step 3 — If M2: receiver role by COMPUTABILITY (not ownership!):
   Receiver can reproduce from LOCAL inputs (own avian-driven motion) → M2-Predicted
   Receiver cannot (server-only forces, someone else's entity)        → M2-Interpolated

Step 4 — Authority sanity check (reject contradictions):
   Server source of truth        → M2/M3 (default, OK)
   Client owns it                → M4 + server validation
   No single owner / concurrent  → M5
   Local + "must be authoritative"→ STOP, reclassify (debug-panics anyway)

Step 5 — Declare: app.sync::<S>(class) / register_command / stamp Provenance.
   (M6 you don't choose — it's the substrate everything is stamped in.)
```

### Quick rules of thumb (the smells)
- **Low latency on your own action** → M4 + M2-Predicted. *Not* M3 (M3 is
  reliable-but-slow; it's for *correct/ordered*, not *instant*).
- **Must be correct & ordered, not instant** (spawn, edit, possess) → M3.
- **Bulk / static / world geometry** → M1. If you're about to stream something
  every tick that never changes, you picked wrong.
- **Predicting something the client can't compute** → wrong; M2-Interpolated.
- **Two users edit the same thing**: one field → M3 (server LWW is fine);
  free text/structure → M5.
- **You added a component and nothing syncs** → you didn't declare it; default is
  M7 by design.

---

## 4. The principle that saves the most bandwidth: don't sync derived state

Step 0.5 deserves emphasis — it's the **"Reconstructible"** axis applied to
*derived* values, and it's where naive netcode wastes most of its budget.

> **Never replicate what is a deterministic function of already-synced inputs.**
> Recompute it locally instead.

Examples (all → M7 recompute, *not* M2):
- **Day/night lighting, sun angle** — a pure function of (M6 sim-clock + ephemeris
  content). Both peers compute the same lighting; syncing it is pure waste.
- **Meshes, materials, LODs, animation pose** — derived from content + state.
- **Interpolation buffers, smoothed camera** — local presentation.
- **Cosim *wiring*** — derived from USD (M1); only the cosim *outputs* (Opaque) ride M2.

This is also a correctness win: derived state computed from synced inputs *can't*
disagree, whereas separately-replicated derived state can.

---

## 5. Worked examples (running the procedure on hypothetical new features)

| New feature / state | Run-through | Mechanism |
|---|---|---|
| **Robotic-arm joint angles** (you control) | entity from USD (M1); angles change every tick (M2); you drive via local commands → reproducible | M1 entity + **M2-Predicted** (Interpolated for others) |
| **Sample collected → inventory count** | runtime fact, discrete, server-auth | **M3** |
| **Chat message** | discrete, reliable, ordered | **M3** (or M5 if it's a shared editable doc) |
| **Positional voice** | high-rate, ephemeral, loss-tolerant | **M4**-style (likely out-of-band stream) |
| **Terrain dig by rover** | deterministic from a dig command → replay gives same terrain | **M3** op-log (cheaper, content-like) — *not* M2 heightmap streaming |
| **New telemetry value from a new Modelica model** | model wiring from USD (M1); value is server-only cosim output, slow | M1 wiring + **M2 low-rate (Interpolated)** |
| **Day/night lighting** | pure function of M6 clock + ephemeris | **M7 recompute** (Step 0.5) |
| **Procedural rocks scattered by seed** | content if seed is shared → both generate identical set | **M1** (seed in M6/M3 once) |
| **A new "highlight selected" outline** | per-peer view only | **M7** |

Every new feature decomposes into pieces, each landing on one mechanism via the
same procedure. No feature ever needs a new mechanism — if it seems to, the state
was misclassified (re-run Step 0.5 and Step 3 first).

---

## 6. Where this connects
- The procedure's Step 1 ⇐ `IDENTITY.md` (provenance).
- Step 3's Predicted/Interpolated + Step's M6 ⇐ what the chosen backend provides
  (lightyear ships both; see `STACK_COMPARISON.md`).
- `register_command` ⇐ existing `#[Command]`/`Mutation<P>`.
- Build order that respects the dependencies ⇒ `IMPLEMENTATION_PLAN.md`.
