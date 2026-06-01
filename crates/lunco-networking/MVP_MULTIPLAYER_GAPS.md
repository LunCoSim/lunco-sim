# MVP multiplayer gap analysis — "N people, each creates + possesses + drives their own rover"

Target scenario, concrete: several people connect to one running world; each **creates**
a rover, **possesses** it, and **individually drives only their own** rover; everyone
sees everyone's rovers move. This doc reviews the whole networking corpus + the current
single-process code against that scenario, names what's missing, and gives the honest
critical path.

Reviewed 2026-05-30 against the code (two read-only audits) + all `crates/lunco-networking/*.md`.

---

## ⏩ STATUS UPDATE 2026-05-31 — much of this is now BUILT (read this first)

This doc was written 2026-05-30 and **predates** the transport/ownership/prediction
commits. Reconciled against the committed code, the picture today is:

| Stage | 2026-05-30 status | **2026-05-31 actual** |
|---|---|---|
| 1 Connect (transport) | spike only | ✅ **DONE** — lightyear WebTransport host+client wired in-app (`server.rs`/`client.rs`), `SessionId` allocation, `SessionRegistry`, late-join replay (`ad638410`) |
| 2 Per-user identity | substrate only | ✅ session table + handshake (session+tick); **G3 server-owned avatar still client-local** |
| 3 Create a rover | local only | ✅ **DONE** — `SpawnEntity` over wire + replicate (`apply_replicated_spawns`); **G2 collision FIXED** (`SkipContentStamp` → Authoritative id) |
| 4 Possess | local only | ✅ **DONE** — over-wire `PossessVessel` + server ownership validation + `broadcast_ownership` (`f9976ed5`); **G4 drive-auth enforced** via `authorize()` |
| 5 Individually drive + predict | all missing | ✅ **CORE DONE** — 20 Hz snapshot replication, input-replay **prediction + reconciliation** (`717f8d66` + reconcile extraction); polish (tick-sync/jitter) remains |
| G5 disconnect cleanup | unspecified | ✅ **DONE** — `on_server_disconnected` → `release_session` frees owned entities |
| gap A big_space coords | missing | 🟡 **PARTIAL** — f64 `pos` + `cell` now on the wire; per-client cell→origin rebase still TODO (cells are 0 today). See DESIGN_GAPS §A. |

**So the laggy-but-correct loop (stages 1–4 + drive) is essentially built and
committed.** What still genuinely blocks the *full* experience: end-to-end
verification under real RAM headroom, tick-sync + server jitter buffer for
feels-right-under-latency, G3 server-provisioned avatars for true N-user, and
cosim-value replication (Ph5). The per-row "Missing" notes below are the original
2026-05-30 analysis — treat the table above as the current truth.

---

## The scenario decomposes into 5 stages

| # | Stage | Mechanism | Phase that delivers it |
|---|---|---|---|
| 1 | **Connect** — N clients join a host's world | transport | Ph2 (P2.3) — spike proved it (Ph0), not yet integrated into the app |
| 2 | **Per-user identity** — each client = a distinct session with its own avatar | M1 + handshake | Ph1 substrate built; **session→avatar binding missing** |
| 3 | **Create a rover** — client makes a new rover at runtime | M3 + M1 + M2 | `SpawnEntity` command exists; **identity + replication of the spawn missing** |
| 4 | **Possess** — bind my avatar → my rover, server-validated | M3 | `PossessVessel` exists (local); **wire + authority missing** |
| 5 | **Individually control** — drive only my rover; others see it move; I predict mine | M4 + M2 | **all missing** (input isolation, state replication, prediction) |

**Honest verdict: the scenario spans Ph2 + Ph3 + Ph4, not Ph2 alone.** "Drive and others see it" is M2 state replication (Ph3); "drive *only mine*, smoothly" is M4 input + prediction + per-session authority (Ph4). Ph2 by itself gets you *possess + spawn appear* (reliable, no motion). See "Critical path" below.

---

## What's built vs specified vs missing

| Capability | Built (code) | Specified (docs) | Missing |
|---|---|---|---|
| Transport / backend | Ph0 spike only (throwaway clone) | TRANSPORT_ABSTRACTION, STACK_COMPARISON | real lightyear integration in-app (Ph2 P2.3) |
| Identity (M1) | ✅ `Provenance`, `GlobalEntityId`, deterministic hash, USD stamping, 23 tests | IDENTITY, DECISIONS D3 | — |
| Clock (M6) | ✅ `SimTick`, `advance_sim_tick`, `IsServer` | — | lightyear Tick binding (D6, Ph3/4) |
| `WireChannel` tag | enum exists, **declared-only, never consulted** | PH2_OP_LOG P2.1 | the registry + `declare_channel` + routing |
| `SessionId` | a bare `u64` type, **no allocation, no map** | TRANSPORT_ABSTRACTION (`Peer`) | allocation on connect + session table |
| Create rover | ✅ `SpawnEntity` typed command + `SpawnCatalog` (skid_rover, ackermann_rover) | D4 | over-wire spawn, **identity on spawned entity**, replication |
| Possess | ✅ `PossessVessel` → `ControllerLink` (local) | PH2_OP_LOG possession §, ontology AcquireStream | wire + server validation + replicate `ControllerLink` |
| Drive | ✅ `DriveRover` → FlightSoftware ports → cosim | MECHANISM_SELECTION | per-session routing, **input isolation**, authority check |
| State replication (M2) | ❌ | DESIGN_GAPS A/C, plan Ph3 | everything (Ph3) |
| Prediction (M4) | ❌ | DESIGN_GAPS F/G, plan Ph4 | everything (Ph4) |
| Multi-client tests | Tier-1 (identity) only | NETWORKING_TEST_PLAN Tier-2 (planned) | all Tier-2 written |

---

## NEW gaps this review surfaced (not previously documented)

### G1 — Input isolation: input mapping must run for the LOCAL avatar only  ⚠️ critical, undocumented
`lunco-controller::translate_intents_to_commands` reads **process-global**
`ButtonInput<KeyCode>` and fans WASD to **every** `(VesselIntentState, ControllerLink)`
controller (`lunco-controller/src/lib.rs:57`). In single-process there's one avatar so
that's fine. The instant there are two possessing avatars in one `World`, **both rovers
receive identical drive commands** — the system can't tell "whose keyboard."

This bites two ways in MP and the plan addresses neither:
- **On a client:** it must map raw input to `DriveRover` **only for its own avatar**
  (the one bound to `LocalAvatar` / `SessionId::LOCAL`), never for replicated proxies of
  other players' avatars. Gate `translate_intents_to_commands` to `With<LocalAvatar>` (or
  equivalent), not "any ControllerLink."
- **On the server:** it must **not** run input→command mapping for remote clients at all
  — their input arrives as `DriveRover`/ControlStream messages over the wire. The server
  consumes those; it doesn't re-derive them from a keyboard it doesn't have.

**Fix:** raw-input→intent→command is a *local-avatar-only* concern. Add a `LocalAvatar`
marker (the client's own avatar) and gate the intent system on it. Remote avatars (if
replicated at all) never run input mapping. This is the input-side mirror of the
possession authority gate.

### G2 — Runtime-spawned rovers need `Authoritative` identity, and that COLLIDES with the USD loader's auto-`Content` stamping  ⚠️ critical
The catalog rovers are USD assets (`skid_rover` → `vessels/rovers/skid_rover.usda`). The
USD loader (`lunco-usd-bevy::instantiate_usd_prim`) **unconditionally** stamps
`Provenance::Content { source: asset_path, path: prim_path }` on every prim. So if user A
and user B each `SpawnEntity("skid_rover")`, both rover roots derive the **same**
`GlobalEntityId` from `(usd, skid_rover.usda, /SkidRover)` — **the B.1 instancing
collision** (DESIGN_GAPS §B.1), previously deferred as "single-instance MVP is fine."

**This scenario is exactly multi-instance.** B.1 is no longer deferrable — it's on the
critical path the moment two people spawn the same rover model.

**Resolution (sharpens D4):** runtime-spawned entities are `Provenance::Authoritative`
(server-allocated unique root id), **not** `Content`. The geometry still loads locally
from the shared USD asset on every peer (no geometry streaming), but the *identity* of a
runtime instance is server-minted and unique per spawn; children are `Derived { parent:
root_id, role }`. The spawn broadcast (`SpawnEntity` mutation) carries the server's root
id so peers converge (the PH2 "runtime-spawn caveat").

**The conflict to resolve:** the USD loader auto-stamps `Content`, which is correct for
**startup-scene** prims but **wrong for runtime-spawned instances**. The spawn path must
either (a) suppress the loader's `Content` stamping for runtime subtrees and stamp
`Authoritative`+`Derived` instead, or (b) the loader must distinguish "I'm loading the
startup stage" from "I'm instancing at runtime." Currently `spawn.rs` stamps **nothing**
and relies on the loader — so today every runtime rover would silently collide.

### G3 — Avatars are identity-less client-side singletons; MP needs server-owned, session-bound avatars
Avatars (`lunco-avatar/src/lib.rs:331`, `lunco-client/.../sandbox.rs:559`) spawn
client-side with **no `Provenance`, no `GlobalEntityId`**, as a de-facto singleton. For MP:
- the server provisions one `Provenance::Authoritative` avatar per connecting `SessionId`;
- the client learns its own avatar's id via the handshake → stores `LocalAvatar`;
- other players' avatars need **not** replicate to you (you see rovers, not cameras) —
  this keeps it cheap. Only `ControllerLink` (possession) and rover state replicate.

Currently none of this exists; the avatar is born local and anonymous.

### G4 — `DriveRover` / `PossessVessel` handlers do no ownership validation
Both observers act on whatever `target` they're handed. On the server this must be gated:
*does the sender's session own (possess) this rover?* Otherwise any client drives any
rover. This is the `authorize(origin, cmd, target)` gate — see G1 (input side) and the
possession authority §. Defense-in-depth: client only emits for its own rover, **and**
server validates regardless.

### G5 — Disconnect cleanup is unspecified
Provisioning has a mirror: when a session drops, release the `ControllerLink` it held
(free its rover), and decide its avatar's fate (despawn, or leave the rover ownerless).
Not in any doc. Needed so a crashed client doesn't lock a rover forever.

---

## Critical path to the scenario (what unlocks at each phase)

```
Ph2  connect + identity + possess + spawn-appears (RELIABLE, NO MOTION)
     ├─ P2.3 lightyear in-app, one reliable channel, IsServer from plugin
     ├─ SessionId allocation on connect + session table          (fills the stub)
     ├─ handshake: server provisions Authoritative avatar/session → client LocalAvatar  (G3)
     ├─ SpawnEntity over wire: server spawns, Authoritative root id in envelope,
     │    suppress loader Content-stamp for runtime subtree        (G2 — REQUIRED here)
     ├─ PossessVessel over wire + server ownership validation      (G4)
     └─ replicate ControllerLink so peers see who owns what        (needs minimal M2)
     ▶ DEMO: two clients connect, each spawns a rover (both appear on both), each
       possesses its own. Rovers do NOT move yet. Possession is enforced.

Ph3  rovers move and everyone sees it (M2 state replication)
     ├─ replicate (CellCoord, Transform) + drive state, per-client origin rebase (gap A)
     ├─ role by computability: driven rover = Predicted candidate; cosim body = Interpolated (gap C)
     └─ interpolation buffer folded into existing Translation/RotationInterpolation
     ▶ DEMO: each client drives its rover (input still server-authoritative / laggy),
       all clients see all rovers move (interpolated). The scenario "works" but feels laggy.

Ph4  individual control feels right (M4 input + prediction)
     ├─ input isolation: intent→command for LocalAvatar only       (G1 — REQUIRED here)
     ├─ ControlStream/INPUT channel, redundant tick-stamped sends, server jitter buffer
     ├─ predict owned rover locally; smooth error-correct toward server (D2, not rollback)
     └─ disconnect cleanup                                          (G5)
     ▶ DEMO: you drive your rover with zero perceived latency; others interpolated;
       you cannot drive anyone else's. This is the full target scenario.
```

**So:** the scenario is a **Ph2→Ph3→Ph4** arc. A laggy-but-correct version is reachable at
**end of Ph3**; the "individually control, feels good" version needs **Ph4**.

---

## Pulled-forward / re-scoped items (vs the original plan)

- **B.1 instancing fix → REQUIRED in Ph2** (was deferred). G2. Runtime spawns = Authoritative, not Content.
- **Session→avatar handshake → minimal version in Ph2** (was Ph6 late-join). G3. Just avatar-id + sim-tick + scene-id.
- **Input-isolation (`LocalAvatar` gating) → Ph4** but the `LocalAvatar` marker itself lands in Ph2 (handshake produces it). G1.
- **Ownership validation → Ph2** (possession) extended in Ph4 (drive). G4.
- **SpawnEntity** promoted from Ph2 "stretch goal" to **core Ph2** — it's stage 3 of the scenario, not optional.

---

## Test additions for the scenario (extend NETWORKING_TEST_PLAN Tier-2)

- **two-clients-spawn** — A and B each `SpawnEntity("skid_rover")`; assert **distinct**
  `GlobalEntityId`s (the B.1/G2 regression guard) and both entities exist on both peers.
- **possession-isolation** — A possesses rover-A, B possesses rover-B; a `DriveRover`
  from A targeting rover-B is **rejected** server-side (G4).
- **input-isolation** — in a 2-avatar headless world, driving input maps to **one**
  rover, not both (G1).
- **disconnect-frees-rover** — A drops; rover-A's `ControllerLink` is released (G5).

---

## One-paragraph answer

The substrate (identity, clock, the command system, `SpawnEntity`, `PossessVessel`) is in
place and is genuinely reusable — the scenario does **not** need new verbs. What's missing
is (1) the wire itself (lightyear in-app, one reliable channel, `SessionId` allocation),
(2) a **session→avatar handshake** so each client knows which avatar is its own, (3)
fixing runtime-spawn identity so two rovers from the same USD asset don't collide (the B.1
gap, now on the critical path), (4) **input isolation** so each client drives only its own
rover, (5) **server-side ownership validation**, and then (6) **state replication (Ph3)**
so rovers visibly move and (7) **prediction (Ph4)** so your own rover feels responsive.
Stages 1–4 of the scenario land across Ph2; stage 5 needs Ph3 then Ph4.
