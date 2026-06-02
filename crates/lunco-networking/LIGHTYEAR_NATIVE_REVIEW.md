# LIGHTYEAR_NATIVE_REVIEW.md

**Architecture review: migrating rover state + prediction to LIGHTYEAR-NATIVE**
(replication + prediction + `lightyear_inputs` + `lightyear_avian3d`), replacing our
snapshot-message sync + hand-rolled predict-own.

Author: lead networking architect. Date: 2026-05-30.
Status: REVIEW. Decisive recommendations, source-anchored. Inputs = 5 firsthand maps
(4 reads of lightyear 0.26.4 replication/prediction/inputs/tick/avian crates + 1 of our
architecture). All lightyear anchors are crate-relative under
`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`; ours are repo-relative.

---

> ## ⏳ TODO / WATCH-ITEM — migrate to lightyear-native prediction when it's viable
>
> **Decision (2026-05-30):** we are NOT going lightyear-native now. We hand-roll
> input-replay reconciliation over our own **f64** avian (see `PREDICTION_RECONCILIATION.md`,
> D2). lightyear stays transport/netcode/sync only.
>
> **The blocker is purely upstream:** `lightyear_avian3d`/`lightyear_replication` 0.26.4 pin
> **avian `^0.5`** and are **f32-only** (no `parry-f64` feature). Our physics is avian-0.6.1 **f64**
> (load-bearing for big-space/lunar-orbital precision). lightyear release notes through 0.26.0
> (2026-01) have **never mentioned f64**.
>
> **MIGRATE to lightyear-native when BOTH preconditions are met (track both):**
> 1. lightyear targets **avian 0.6+** (it ships ~monthly, so plausible), AND
> 2. `lightyear_avian3d`/`lightyear_replication` gain a **`parry-f64` / f64** path (NOT yet signaled).
>
> The hand-rolled design is **deliberately shaped to make this migration cheap**: the input is
> already `seq`-stamped per-vessel (→ `ActionState<A>`), state is `{Position,Rotation,LinearVelocity,
> AngularVelocity}` (exactly what `lightyear_avian3d` replicates), and identity stays `GlobalEntityId`
> (→ `PreSpawned(hash==gid)`). The replication math in `lightyear_avian3d` is **already
> precision-generic** (`types_3d.rs` uses `avian::math::Scalar`), so a **fork** to f64 is *moderate*
> (avian 0.5→0.6 bump + an `f64` feature flag) — see §6 Q1 — and is the fallback if upstream stalls.
> Forking still inherits the D6/D7/codec/identity costs in §3, so it's a strategic move, not a quick win.
>
> **Owner action when revisiting:** re-run this review against the then-current lightyear; the
> seams that migrate first are M4 (inputs → `ActionState`) and M2 (snapshot → `Replicate`).

The user has committed to "do it via lightyear." This is **HOW**, honestly costed — not
whether. But "via lightyear" already has two readings, and the honest answer forces the
distinction up front:

- **lightyear-as-transport** (what we ship today — `Frame`/channels, netcode, WebTransport),
- **lightyear-NATIVE** (adopt `lightyear_replication` + `lightyear_prediction` +
  `lightyear_inputs` + `lightyear_avian3d` for rover state and prediction).

D1 ("lightyear committed") is satisfied by the first. This review is specifically about the
second, and it must be read against **two** competing in-tree designs it would replace: the
original D2 smooth-correction, **and** the newer hand-rolled input-replay design dated the
same day as this review (`PREDICTION_RECONCILIATION.md`, 2026-05-30), which re-steps avian
f64 manually.

---

## 1. VERDICT UP FRONT

### Full lightyear-NATIVE prediction is NOT VIABLE for our stack as-is. The blocker is hard, not a matter of effort.

**The single biggest blocker is a double avian incompatibility, and it is a wall, not a hill:**

**(1a) Version: `lightyear_avian3d 0.26.4` and `lightyear_replication 0.26.4` require avian3d `^0.5`. We are pinned to avian3d 0.6.1.** `^0.5` is semver-incompatible with `0.6.1`.
- `lightyear_avian3d-0.26.4/Cargo.toml:62-64` → `[dependencies.avian3d] version = "0.5"`.
- `lightyear_replication-0.26.4/Cargo.toml:59-62` → `[dependencies.avian3d] version = "0.5"`.
- Confirmed in our repo: `Cargo.toml:110` → `avian3d = { version = "0.6.1", ... }`.
- The `Diffable` impls and the whole avian plugin are compiled against avian 0.5's API and 0.5 plugin/set names (`PhysicsTransformPlugin`, `PhysicsSystems`), several of which avian 0.6.1 renamed. You cannot link lightyear 0.26.4's replication/avian path against avian 0.6.1 at all.

**(1b) f64: `lightyear_avian3d` is f32-only by construction, and our rover physics is f64. THIS IS THE LOUD ONE.** Our avian is `features = ["f64", "parry-f64", ...]` (`Cargo.toml:110`). lightyear_avian3d cannot be made f64:
- Its `default` feature force-enables f32: `lightyear_avian3d-0.26.4/Cargo.toml:46-50` → `default = ["std","3d","avian3d/parry-f32"]`. The only precision feature it exposes is `f32` (`Cargo.toml:52`). **There is no `f64`/`parry-f64` feature anywhere** in `lightyear_avian3d`, `lightyear_replication`, or the umbrella `lightyear`.
- avian's `f32`/`f64` are **mutually-exclusive whole-module selectors** (`avian3d-0.5.0/src/math/mod.rs:7-17`: `#[cfg(feature="f32")] mod single` vs `#[cfg(feature="f64")] mod double`). The umbrella pulls `lightyear_avian3d` **without `default-features=false`** (`lightyear-0.26.4/Cargo.toml:229-234`), so `avian3d/f32` turns on in the feature-unified graph. If our app also requests `avian3d/f64`, **both** modules compile → duplicate `Scalar`/`Vector`/`Quaternion` → **compile error**. The two cannot coexist in one build.
- Even the code hardcodes the downcast: `correction_3d.rs:174-181` `to_transform` does `translation: pos.f32(), rotation: rot.f32()`; `plugin.rs:541-572` calls `.f32()` everywhere. The visual/correction path is f32 by design.

**What this means, plainly:** there is **no supported configuration** of lightyear 0.26.4 that gives you native replication + prediction over your avian3d-0.6.1-f64 rover bodies. To get lightyear-native physics prediction you would have to **(a) downgrade avian to 0.5.x AND (b) convert all rover physics from f64 to f32**. Item (b) is not a networking change — it reverses a deliberate simulation-precision decision (our whole stack is f64 SI-canonical for orbital/lunar scales; `lunco-axes-and-units` canonical frame is f64). Downgrading avian and de-precisioning the physics to satisfy a networking crate is a tail-wagging-dog reversal we should not accept.

### Secondary blockers (each independently disqualifying for "full native"), all from the maps:

- **D2 conflict by construction.** lightyear prediction *is* re-running `FixedMain` (the physics) N ticks on `Confirmed<C>` mismatch — `lightyear_prediction-0.26.4/src/rollback.rs:826-843` loops `world.run_schedule(FixedMain)` per rolled-back tick. There is **no "correct-without-resim" mode**. The avian doc states the default outright: *"for Predicted entities, your `Position` is replicated as `Confirmed<Position>`. This triggers an immediate rollback"* (`lightyear_avian3d-0.26.4/src/plugin.rs:1-6`). That is exactly the full-avian-rollback D2 rules out because avian's solver is global + non-deterministic across wasm.
- **Identity reversal (D3/D4).** lightyear's wire identity is the **raw bevy `Entity` + a per-connection, runtime-allocated `RemoteEntityMap`** (`lightyear_serde-0.26.4/src/entity_map.rs:92-97`); on spawn it `world.spawn(...)` a brand-new local entity and maps `remote→local` (`lightyear_replication-0.26.4/src/receive.rs:877,914`). Our `GlobalEntityId` is a stable u64 **derived identically on both peers with zero coordination** (`lunco-core/src/identity.rs:85-109`). These are opposite philosophies (detail in §3).
- **Serde, not Reflect.** lightyear replication/prediction require `Serialize/Deserialize + Clone + PartialEq` on every networked component (`lightyear_replication-0.26.4/src/registry/registry.rs:162`; `SyncComponent`, `lightyear_prediction-0.26.4/src/lib.rs:49`). Our op-log codec is Reflect-based (`wire.rs:245,318`). The two codecs would coexist, not merge.

### The decisive recommendation

**Do NOT pursue full lightyear-native prediction now. It requires reversing avian-0.6.1, f64, D2, D3, and D4 simultaneously — five locked or load-bearing decisions — to satisfy one crate version that does not even support our precision.**

Instead: **keep lightyear as the transport/netcode/sync substrate (D1 stands), and adopt the hand-rolled input-replay reconciliation already designed in `PREDICTION_RECONCILIATION.md` (2026-05-30) for the owned rover.** That design re-steps **our own avian f64**, keeps `GlobalEntityId`, keeps the op-log, and is the mainstream-FPS shape (predict-own + reconcile-the-one-owned-body) that lightyear-native would have given us — minus the f64/version wall. It is strictly less risky and loses nothing we can actually use.

Native is re-openable later **only** if a future lightyear targets avian 0.6 **and** gains a `parry-f64` path. Until both exist, native is off the table. The migration plan in §5 is therefore a *partial-adoption* plan, and §6 lists the forks for the human — the biggest being "do we down-precision to f32 to unlock native," which I recommend answering **no**.

---

## 2. WHAT LIGHTYEAR GIVES US (and which hand-rolled part it would replace)

This is the honest mapping of each native piece to the problem it solves and the hand-rolled
part it would retire — **conditional on the f64/version wall being gone**, which it is not. Read
this as "what we forgo by not going native," so the trade is visible.

| Lightyear piece | Problem it solves | Replaces (ours) | Usable for us as-is? |
|---|---|---|---|
| **`lightyear_replication` (`Replicate`, `PredictionTarget`, `Confirmed<C>`)** | Server→client component sync; per-entity opt-in tag (`lightyear_replication-0.26.4/src/send/components.rs:757-781`); auto-`Confirmed<C>` co-located on the predicted entity (`registry/replication.rs:282-318`) | Our hand-rolled `SnapshotMsg`/`gather_snapshot`/`ingest_snapshots` (M2). Our `SnapshotEntry{gid,t,r}` (`wire.rs:50-55`) → lightyear's per-component delta + `Diffable` | **Partly.** The *model* is what we want for M2-Interpolated remote bodies. But identity = raw `Entity`+map, codec = Serde, and it ships zero avian-f64 components. Net: not adoptable for the rover state path without the f64 wall removed. |
| **`lightyear_prediction` (rollback engine, `PredictionHistory<C>`, `add_prediction`)** | Client-side predict + server reconcile via `Confirmed<C>` vs history mismatch → re-sim `FixedMain` (`rollback.rs:184+`, `:826-843`) | The would-be input-seq/replay loop from `PREDICTION_RECONCILIATION.md` (§4): compare-at-seq, snap, re-step avian | **No.** Its only reconcile mode is **re-run-FixedMain rollback** (D2 conflict) and it requires the avian-f64-incompatible state path. `RollbackMode::Disabled` exists (`manager.rs:43`) but then you keep none of what prediction buys — you're back to hand-rolled. |
| **`lightyear_inputs` (native: `ActionState<A>`, `InputBuffer`, redundant unreliable send, server apply, replay)** | Per-tick *held-control state* buffered, redundantly sent, replayed deterministically every rollback tick (`lightyear_inputs_native-0.26.4`; buffer `lightyear_inputs-0.26.4/src/input_buffer.rs:30-40`; replay re-reads buffer `client.rs:331-356`) | The *input-seq + unacked `InputFrame` ring + per-tick emission* we'd otherwise hand-roll (`PREDICTION_RECONCILIATION.md` §4.4, the held-key carry-forward) | **Conceptually the cleanest single win, but coupled.** It hard-depends on `InputTimeline` reaching `IsSynced` (`lightyear_prediction-0.26.4/src/rollback.rs:258`, `manager.rs:95`) — i.e. it drags in the prediction/sync stack and lightyear's `Tick` ownership. You cannot take inputs+replay without prediction's timeline. So it's not separable from the rollback engine we can't use. |
| **`lightyear_frame_interpolation`** | Smooth render between fixed ticks (display one tick behind, lerp by overstep) (`lightyear_frame_interpolation-0.26.4/src/lib.rs:1-12`) | Our `interpolate_proxies` / `INTERP_DELAY=0.12s` render smoothing (`commands.rs:251-310`) | **No.** It mandates disabling avian's `PhysicsInterpolationPlugin` + `PhysicsTransformPlugin` (`lightyear_avian3d-0.26.4/src/plugin.rs:19-31`) — same avian-0.5 surgery, same wall. Our render smoothing already works. |
| **`lightyear_avian3d` (`Diffable` for Position/Rotation, lerp/slerp helpers, the plugin)** | Delta-compress avian pose; wire prediction into avian's schedule | The pose-packing in `SnapshotEntry` + our manual avian `Position` writeback (`wire.rs:365-376`) | **No — this is the wall itself** (avian ^0.5, f32-only; §1). |
| **`lightyear_sync` / `Tick` / `LocalTimeline`** | Client tick estimate of server + slew/snap to `server+RTT/2` (`lightyear_sync-0.26.4/src/client.rs:50-64`, `timeline/sync.rs`) | Nothing yet — D6 is unbuilt | **Independent of the wall, and we already pull `Client/ServerPlugins` with our 60 Hz tick** (`shared.rs:38,51,70`). This is the one piece adoptable today, but per `PREDICTION_RECONCILIATION.md` §1.4 the hand-rolled design **does not need it** (seq-based reconcile, not tick-based). |

**Bottom line of §2:** the genuine native candidates are M2 (replicated/predicted), M4
(`lightyear_inputs`), M6 (Tick/sync) — exactly as `SYNC_ARCHITECTURE.md` already classified.
But M2 and M4 both route through the avian-f64-incompatible prediction stack, and M4 can't be
taken without M2's timeline. So the *only* native piece we can actually adopt as-is is M6's
tick-sync — and our hand-rolled reconcile design explicitly doesn't require it. **Net usable
native surface for the rover today: effectively zero.**

---

## 3. THE HARD CONFLICTS WITH LOCKED DECISIONS

### 3.1 IDENTITY (D3) — the crux. Resolution: **they coexist; lightyear does NOT and must not own identity. `GlobalEntityId` stays the cross-peer name.**

This is the decisive conflict, so it gets a decisive answer.

**The two models are opposite:**
- **Ours (D3):** `GlobalEntityId(u64)` is a *pure function of `Provenance`* — `derive_id` FNV-1a64 folded to 53 bits (`lunco-core/src/identity.rs:85-109`). Content/Derived ids are **computed identically and independently on every peer, zero bytes, zero coordination** (the "shared stable name" model — Unreal net-stable-names / content-GUID). It is **not a per-connection handle.**
- **Lightyear's:** the wire identity *is the sender's raw bevy `Entity`* (index+generation), and the receiver `world.spawn`s a fresh local entity and records `remote→local` in a per-connection, runtime-allocated, **non-deterministic** `RemoteEntityMap` (`lightyear_serde-0.26.4/src/entity_map.rs:92-97`; spawn-and-map at `lightyear_replication-0.26.4/src/receive.rs:877,914`). Server `Entity(42v1)` → whatever local `Entity` the client's `World::spawn` happened to hand out.

**Can they coexist?** Yes — and the firsthand map confirms the mechanism is *already there and benign*:
- lightyear **assigns no ids of its own** and **never consults `GlobalEntityId`**. They are orthogonal. You replicate `GlobalEntityId` as **just another component** (it impls the needed traits) and resolve local↔global at the boundary yourself — exactly the seam our Ph2/wire already has (`resolve_ids_in_json` / `api_id_for`, used `wire.rs:255,303`).
- Cross-component entity references inside replicated components are mapped via `EntityMap`/`MapEntities`, and crucially **`EntityMap::get_mapped` leaves an unmapped entity unchanged** (`lightyear_serde-0.26.4/src/entity_map.rs:29-41`). So a replicated `GlobalEntityId` is left alone — correct.

**The one place they collide is SPAWN.** Native lightyear replicates spawns and mints a fresh local entity (D4's "runtime = server spawns"), whereas D4 says *content entities are spawned locally on every peer under the derived id and NOT replicated*. lightyear's only hook to honor a pre-existing-entity is **`PreSpawned` hash-matching** (`receive.rs:821-832`; match by hash, `prespawn.rs:35-117`, with a documented "same hash → extra rollbacks" caveat). To make lightyear honor our identity for content entities you'd locally spawn each, attach `PreSpawned(hash == gid)`, and have the server replicate the same hash — i.e. **rebuild M1 on top of lightyear's prespawn matcher and inherit its collision-rollback caveat.**

**RESOLUTION (decisive):**
1. **`GlobalEntityId` stays the authoritative cross-peer identity. Lightyear never owns it.** This is non-negotiable; D3 is the core of M1 and has 23 green proto-tests behind it.
2. For the **transport-only** posture we recommend (§1), this is a non-issue: we don't replicate spawns through lightyear at all — our wire ships `gid` raw (`wire.rs:51,67`) and the client pins via `from_raw`. Keep it.
3. **If** native is ever revisited, content entities go through `PreSpawned(hash==gid)` (accepting the collision caveat, which D3a's debug-time collision check already guards), and only `Authoritative` runtime entities use lightyear's spawn-and-map. That is the *only* coexistence shape — and it's strictly more machinery than we have now for no identity benefit.

**One does NOT have to give for coexistence — but the clean native path WOULD collapse D4's content/runtime split into "everything is Authoritative." We reject that collapse.**

### 3.2 POSSESSION / AUTHORITY — Resolution: **`SessionRegistry` + `authorize()` STAY; lightyear authority is NOT adopted.**

- **Ours:** `SessionRegistry { owners: HashMap<gid, SessionId> }` (`session.rs:110-197`) + a single domain gate `authorize(reg, origin, type_name, target_gid)` (`session.rs:280-296`) that allows `DriveRover`/`BrakeRover` **only if origin owns target_gid**. Possession is itself an op-log **command** (`PossessVessel`/`ReleaseVessel`, `shared.rs:87-88`); the registry is broadcast as `OwnershipMsg` (`server.rs:73-83`).
- **Lightyear's:** authority = "who simulates + replicates this entity," granted by adding `Replicate` (`lightyear_replication-0.26.4/src/authority.rs:1002-1003`), tracked in a server `AuthorityBroker` keyed by local `Entity` (`:156-165`), transferred via `GiveAuthority`/`RequestAuthority` triggers. Separately, *who predicts* is set by `PredictionTarget`, not authority.

**What maps:** the natural fit is server-authoritative (server always `HasAuthority`, `PredictionTarget::to_clients(All)` so the possessing client predicts). **What is replaced:** *nothing should be.* lightyear's `AuthorityBroker` is keyed by `Entity` and **would not, on its own, reject a `DriveRover` from a non-owner** — that domain rule is *our* `authorize()`. Possession is a `Mutation` with `OpId`/dedup/Ack semantics lightyear authority-transfer doesn't model. Migrating possession to lightyear authority **loses** the op-log command semantics and gains a heavier, more general mechanism than our single-owner model needs.

**RESOLUTION:** Keep `SessionRegistry`/`authorize`/possession-as-command unchanged. If native were ever adopted, lightyear authority would be set *from* `SessionRegistry` (server stays `HasAuthority`; `PredictionTarget` driven by ownership) — a one-way derive, never a replacement.

### 3.3 WIRE / COMMANDS (M3 op-log) — Resolution: **op-log STAYS CUSTOM; only the rover's held-control input is a candidate for `lightyear_inputs` — and even that we hand-roll for now.**

The maps draw the line cleanly. lightyear inputs are **per-tick state**, not commands; lightyear replication is **state**, not events. Our op-log carries discrete, reliable, identity-bearing, dedup'd, replay-once events that **neither models**.

**MUST stay custom op-log (M3, `Mutation`/`OpId`, OrderedReliable):**
- **Possession claims** (`PossessVessel`/`ReleaseVessel`) — one-shot reliable, *must not be replayed* (lightyear inputs have `config.ignore_rollbacks` precisely because settings-like actions shouldn't replay, per the inputs map). Replaying possession during rollback would be wrong.
- **Spawn/despawn, USD prim edits, parameter changes, Modelica text** (M3/M5) — discrete, reliable, `GlobalEntityId`-bearing, `OpId`-dedup'd. Lightyear has no op-log; its message bus has no `OpId`/dedup/authority.
- **warp/time-warp** (D5) — host-only discrete control.

**Candidate for lightyear inputs (M4):** the **held control state** of the rover — `forward/back throttle, steer, brake`. This is exactly per-tick state that must be replayed deterministically. `DriveRover{forward,steer}` + `BrakeRover{intensity}` would become the `A` in a native `ActionState<A>`, written each `FixedPreUpdate`, read by physics in `FixedUpdate`, replayed from `InputBuffer.get(tick)`.

**But — two frictions make even M4 not-worth-it now:**
1. **It can't be taken alone.** `lightyear_inputs` replay rides the prediction rollback loop (`lightyear_prediction-0.26.4/src/rollback.rs:826-843`) and hard-requires `InputTimeline` synced (`rollback.rs:258`, `manager.rs:95`). Taking inputs = taking the prediction/sync stack = the avian-f64 wall returns.
2. **Channel + identity split.** Inputs ride lightyear's built-in `InputChannel` (Sequenced-Unreliable) addressed by the **mapped `Entity`** (`lightyear_inputs-0.26.4/src/input_message.rs:27-35,267-273`), not our `gid`/OrderedReliable. Adopting it adds a second client→server channel with different reliability + a second identity boundary.

**RESOLUTION:** Op-log stays fully custom. The rover's held-control input becomes a dense per-vessel `seq`-stamped `DriveRover`/`BrakeRover` on our existing OrderedReliable wire **with input-replay reconciliation done by hand** (`PREDICTION_RECONCILIATION.md` §3.4, §4). This is the lightyear-inputs *shape* (per-tick state, replayed) without the prediction-stack coupling. The day a usable native path exists, this is the seam that migrates to `ActionState<A>` first — it's deliberately built to.

### 3.4 TICK (D6) — Resolution: **adopting prediction WOULD invert D6 on clients; since we're not adopting prediction, D6 is moot for the rover and the hand-rolled design sidesteps it.**

- Lightyear's `Tick` is a **wrapping `u16`** in `LocalTimeline`, incremented once per `FixedMain` in `FixedFirst` (`lightyear_core-0.26.4/src/timeline.rs:106-126`); it wraps at 65536 (~18 min at 60 Hz). Our `SimTick` is `u64`.
- **On a predicting client, lightyear OWNS the clock**: the sync machinery slews/snaps `LocalTimeline` and dilates `Time<Virtual>`→`Time<Fixed>` to keep the client at `server+RTT/2` (`lightyear_sync-0.26.4/src/timeline/sync.rs:242-336`). So D6's "SimTick drives lightyear Tick" is **backwards for clients** if we predict — lightyear must drive `SimTick` there (`SimTick = base_u64 + dewrapped(LocalTimeline.tick)`), and every FixedUpdate side-effect (`advance_sim_tick`, op-log apply, ROS/Copper bridge) must be gated `run_if(not(is_in_rollback))` or it double-fires during the N-tick re-sim.
- **On the server**, `LocalTimeline` is just its FixedUpdate count, so server `SimTick` and lightyear-tick advance in lockstep and `SimTick` can stay master — D6 holds server-side.

**RESOLUTION:** Because we are **not** adopting prediction, lightyear never dilates our client `FixedUpdate`, and D6 stays as written (SimTick authoritative). The hand-rolled reconcile explicitly avoids this: it matches on a dense per-vessel `seq`, not lightyear's tick, and notes our client `SimTick` is already non-monotonic (clobbered to `snap.tick`, `wire.rs:375`) so it adds a never-clobbered `ClientPredictedTick` for the replay ring (`PREDICTION_RECONCILIATION.md` §6.5). No D6 reversal needed. **If native were ever adopted, D6 inverts on clients — flag this as a real future cost.**

### 3.5 D7 FEATURE-GATING — Resolution: **transport-only adoption FITS D7 cleanly; native prediction would BLEED into always-on substrate and break "domain crates byte-identical."**

D7: substrate (`Provenance`, `GlobalEntityId`, `SimTick`, `IsServer`, `SessionRegistry`, the `app.sync`/`register_command` facade) is **always-on**; only `lunco-networking`'s lightyear guts are behind `feature="networking"` (`DECISIONS.md:89-108`; today only `Frame`/channels touch lightyear, `protocol.rs`).

- **Transport-only / hand-rolled reconcile = D7-clean.** The reconcile design adds `seq`/`lv`/`av` fields (`#[serde(default)]`, no envelope bump) and a `reconcile_owned_rover` system in `lunco-sandbox-edit`. The new physics behavior (predict-own + replay) is in domain/client code that runs identically feature-on or feature-off (replay just never has unacked inputs to chew when offline). No lightyear types leak into always-on substrate. Domain crates stay byte-identical.
- **Native = D7-bleed.** Native prediction needs `Confirmed<C>`/`Predicted`/`PredictionHistory<C>`/`PreSpawned` markers **on domain physics components**, and the avian plugin **disables `PhysicsTransformPlugin` + `PhysicsInterpolationPlugin`** (`lightyear_avian3d-0.26.4/src/plugin.rs:19-31`) — i.e. it **changes the substrate physics schedule**, which is supposed to be always-on. Either the feature-off build keeps full avian (two physics configurations to maintain) or the disable leaks into always-on code. D7's "domain crates byte-identical whether or not `networking` is set" guarantee is **broken** by native prediction.

**RESOLUTION:** Native prediction violates D7's central guarantee. Transport-only + hand-rolled reconcile preserves it. Another reason native is rejected now.

---

## 4. TARGET ARCHITECTURE

Given §1's wall, the target end-state is **lightyear-as-substrate + hand-rolled predict-own
reconcile**, NOT lightyear-native prediction. This is the architecture we should build.

### Features enabled
- **lightyear features (already in `lunco-networking/Cargo.toml:30-47`):** `client`, `server`, `netcode`, `webtransport`, `webtransport_self_signed` (+ `webtransport_dangerous_configuration` native). **NOT** `replication`-driven prediction, **NOT** `lightyear_avian3d`, **NOT** `lightyear_inputs`, **NOT** `prediction`. lightyear stays the dumb pipe carrying our `Frame(Vec<u8>)`.
- **avian (unchanged): 0.6.1, f64, parry-f64, parallel, xpbd_joints** (`Cargo.toml:110`). No downgrade, no de-precision.

### What a replicated + predicted rover looks like (ours, not lightyear's)
A single owned rover on a client:
- **Identity:** carries `Provenance` + `GlobalEntityId` (always-on substrate). Wire ships raw `gid`.
- **Markers (ours):** `OwnedLocally` (`session.rs:226-227`) — the single classifier. Owned ⇒ `RigidBody::Dynamic` (`maintain_owned_locally`, `commands.rs:323-358`), full avian f64 + wheel forces locally. All other rovers ⇒ `RigidBody::Kinematic` (`force_kinematic_proxies`, `commands.rs:149-188`), interpolated.
- **Input type (ours, lightyear-inputs-shaped):** `DriveRover{ target, forward, steer, seq:u32, tick:u64 }` + `BrakeRover{ target, intensity, seq, tick }` (`mobility/lib.rs:430-442`), same `seq` per input frame, on OrderedReliable `ControlStream`.
- **Replicated state (ours):** `SnapshotEntry{ gid, t, r, lv, av, last_input_seq }` (`wire.rs:50-55` + the 3 new fields), server→clients on `SnapChannel` (UnorderedUnreliable).
- **Predicted-state history ring (new, client):** `{seq, ClientPredictedTick, Position, Rotation, LinearVelocity, AngularVelocity}` recorded after avian writeback each fixed tick.
- **Reconcile (new, client):** `reconcile_owned_rover` (replaces `correct_owned_prediction`, same slot `FixedPostUpdate` after `PhysicsSystems::Writeback`) — on a snapshot with new `last_input_seq` for an owned gid: compare-at-seq → snap the 4 integrator components → drop acked `InputFrame`s → re-step avian over the ~3–6 unacked → render-smooth the residual onto `Transform` only.

### What stays in our substrate (always-on, unchanged)
`Provenance`/`derive_id` (M1), `GlobalEntityId`, `SimTick`/`IsServer`, `SessionRegistry`/`authorize`/possession-as-command (M3), `WireEnvelope`/`Mutation`/`OpId`/`WireDedup` op-log, the Reflect codec, `WireApplyGuard` echo-guard. None of this moves to lightyear.

### Data flow (new owned-rover path)

```
 CLIENT (predictor)                    WIRE (lightyear = dumb pipe)        HOST (authority)
 ┌──────────────────────────┐                                            ┌───────────────────────────┐
 │ FixedUpdate:             │  DriveRover{fwd,steer,seq,tick}            │ apply_wire_command:       │
 │  sample input            │ ───── OrderedReliable (CmdChannel) ─────▶  │  authorize(reg,origin,gid)│
 │  stamp seq+SimTick       │       (our Frame/WireEnvelope)            │  record AppliedInputSeq   │
 │  push InputFrame ring    │                                            │  → on_drive_rover → ports │
 │  → on_drive_rover ports  │                                            │ FixedUpdate: wheel forces │
 │  → wheel forces          │                                            │  → avian f64 solve        │
 │  → avian f64 solve       │                                            │ FixedPostUpdate:          │
 │  → push predicted-state  │                                            │  gather_snapshot reads    │
 │    history[seq]          │                                            │  Pos/Rot/Lin/Ang +        │
 └───────────┬──────────────┘                                            │  AppliedInputSeq[gid]     │
             │                  SnapshotMsg{tick, entries:[{gid,t,r,     └─────────────┬─────────────┘
             │            ◀──── lv,av,last_input_seq}]}  UnorderedUnreliable           │
 ┌───────────▼──────────────┐        (SnapChannel)                                     │
 │ reconcile_owned_rover    │   (broadcast NetworkTarget::All, one serialize)          │
 │  on new last_input_seq:  │                                                          │
 │   compare history[seq]   │   ── M2-Interpolated path (remote rovers/balloons) ──    │
 │   snap 4 integrator comps│   force_kinematic_proxies + interpolate_proxies          │
 │   drop acked InputFrames │   (unchanged; reads t,r only; ignores lv/av/seq)         │
 │   re-step avian × N(3-6) │                                                          │
 │   render-smooth residual │                                                          │
 └───────────┬──────────────┘                                                          │
             │ Transform offset (visual only, never Position)                          │
 ┌───────────▼──────────────┐                                                          │
 │ Render: predicted @ now  │                                                          │
 │  + big_space rebasing    │   ← gap A (CellCoord/floating-origin) stays OURS         │
 └──────────────────────────┘                                                          │
```

Note: **big_space / floating-origin rebasing (gap A) stays our problem either way** — lightyear replicates a flat `Position`, has no `CellCoord` concept (`SYNC_ARCHITECTURE.md` gap A), so native wouldn't have helped here.

---

## 5. MIGRATION PLAN (phased, each leaving the build green)

This is the `PREDICTION_RECONCILIATION.md` plan, framed as the migration off both old-D2
smooth-correction and away-from any native ambition. Each phase is independently testable.

**P1 — Velocity on the wire + dead-reckon (stepping stone; no seq yet).**
Add `lv`/`av` `#[serde(default)]` to `SnapshotEntry` (`wire.rs:50-55`), `SnapshotSample` (`session.rs:258-263`), `InterpSample` (`commands.rs:192-197`); `gather_snapshot` query `+ &LinearVelocity, &AngularVelocity` (`wire.rs:421`). Owned rover predicts forward using *real* authoritative velocity instead of finite-difference. Kills the worst rubber-band before any seq machinery. **Green:** old corrector still runs, just fed better velocity.

**P2 — Per-tick input emission (the load-bearing fork).**
Move `DriveRover`/`BrakeRover` emission from the `Update`/observer edge-gate (`controller/lib.rs:115-130`) to `FixedUpdate`, dropping `if prev != current` so **every fixed tick emits exactly one owned input** (Gambetta/Source style). This makes replay a trivial 1:1 loop and eliminates the held-key carry-forward hazard (`PREDICTION_RECONCILIATION.md` §4.4, §6.1). **This is the riskiest phase** (see below). **Green:** behavior identical for a held key; only emission cadence changes.

**P3 — Seq + history ring + server ack (no reconcile yet).**
Add `seq:u32`/`tick:u64` to `DriveRover`/`BrakeRover` (`mobility/lib.rs:430-442`; ride `capture_command` for free, §3.4). Add per-vessel seq counter + `InputFrame` unacked ring (client) + predicted-state history ring. Add `AppliedInputSeq(HashMap<u64,u32>)` (host) written post-`authorize` (`wire.rs:291-300`), read in `gather_snapshot` + connect baseline, stamped as `last_input_seq` per entry. Add `ClientPredictedTick` (never clobbered) for the ring index (§6.5). **Green:** fields flow, nothing consumes them yet.

**P4 — `reconcile_owned_rover` (the swap).**
Replace `correct_owned_prediction` (`commands.rs:402-500`) with snapshot-triggered compare-at-seq → snap 4 integrator components → drop acked → re-step avian × N → render-smooth residual (§4). Re-run the **whole `FixedUpdate`+`FixedPostUpdate`** per replayed tick (NOT bare `PhysicsSchedule`) so wheel forces re-apply (§4.3). Cap N at `MAX_REPLAY_TICKS=12` (§6.8). **Green:** the one-system swap; the rest of the pipeline is steady.

**P5 — Scheduling coherence (apply-vs-gather phase).**
Move `gather_snapshot` (+ seq/velocity read) to run in/after `FixedPostUpdate` so `tick`, velocity, and ack-seq are sampled from the same post-step world (§6.4). **Green:** a schedule move, no wire change.

**Riskiest phase = P2** (per-tick emission). It changes `translate_intents_to_commands` semantics from *sparse latched setpoints* to *per-tick samples*, touching the shared controller path used by both networked and standalone play, and interacts with the latched-port FSW chain (`on_drive_rover` writes a persistent `DigitalPort.raw_value`, `mobility/lib.rs:467-503`). Get this wrong and replay under-applies a held throttle and rubber-bands *worse* than today (`PREDICTION_RECONCILIATION.md` §6.1). It is also the one phase whose correctness isn't local to the networking crate. Land it behind a thorough standalone-play regression before P3.

**Explicitly NOT in this plan:** any avian downgrade, any f32 conversion, any
`lightyear_replication`/`_prediction`/`_inputs`/`_avian3d` link. Those are the native path,
which §1 rejects.

---

## 6. OPEN QUESTIONS / DECISIONS FOR THE HUMAN

These are the genuine forks. My recommendation is bolded on each.

**Q1 (THE big one) — Do we down-precision rover physics to f32 + downgrade avian to 0.5 to unlock lightyear-native prediction?**
This is the *only* way to get native. It reverses f64 (our whole SI-canonical, orbital/lunar-scale precision stance; `lunco-axes-and-units` is f64) and avian 0.6.1, to satisfy a crate that doesn't support our precision. **Recommendation: NO.** The cost (de-precision the simulation, downgrade physics, re-validate every dynamics model) vastly exceeds the benefit (a reconcile loop we can hand-roll over our own f64 avian in ~5 green phases). Re-open only if a future lightyear targets avian 0.6 **and** ships a `parry-f64` path — track both as a watch item.

**Q2 — Identity coexistence: confirm `GlobalEntityId` stays the cross-peer name and lightyear never owns identity?**
**Recommendation: YES, confirm it.** §3.1 shows coexistence is free in the transport-only posture and that even a future native path keeps `GlobalEntityId` as a replicated component + `PreSpawned(hash==gid)` bridge. There is no scenario where we hand identity to lightyear's `RemoteEntityMap`. This should be ratified so it's not re-litigated.

**Q3 — How much of M1–M7 is "torn out" for lightyear? Answer: almost none.**
Per the maps and `SYNC_ARCHITECTURE.md`: M1 (content/det-id), M3 (op-log), M5 (CRDT), M7 (local-only) **stay custom by construction** — lightyear has no equivalent. M2/M4/M6 were the native candidates, and §1–§3 show none is adoptable over avian-0.6.1-f64. **So the honest answer is: nothing in M1–M7 gets torn out; M2/M4 get hand-rolled in the lightyear-inputs *shape*, and M6 stays SimTick-master.** The human should accept that "do it via lightyear" = lightyear-as-transport, not lightyear-native, **until Q1's preconditions exist.** This is the load-bearing expectation to reset.

**Q4 — Ratify the D2 reversal already proposed in `PREDICTION_RECONCILIATION.md` (2026-05-30)?**
That doc reopens D2 from "smooth-correction, never rollback" to "input-replay with on-demand avian re-stepping for the owned rover." It is sound: on a client the owned chassis is the **only `Dynamic` body** (all proxies `Kinematic`), so a replay tick solves a 1-body island — the "global solver / non-determinism" premises that justified old-D2 don't apply, because this is *state replication re-anchoring*, not lockstep. **Recommendation: RATIFY the new D2.** It is what lightyear-native would have given us, minus the f64/version wall, and is the plan in §5. Note this *strengthens* the case against native: we get predict-own + reconcile without any lightyear-prediction dependency.

**Q5 — P2 per-tick input emission: accept the controller-semantics change (sparse latched → per-tick samples)?**
This is the riskiest, least-local change (§5). The clean alternative (held-key carry-forward cursor over edge-only frames, §4.4) avoids the controller change but complicates replay. **Recommendation: accept the per-tick change (P2), but gate it behind a standalone-play regression** — clean replay is worth the one-time controller refactor, and it's the seam that would later migrate to `ActionState<A>` if native ever opens up.

**Q6 — big_space / floating-origin (gap A): out of scope for this migration, confirm?**
lightyear (native or not) has no `CellCoord` concept; our render-rebasing stays ours regardless. **Recommendation: YES, confirm out of scope** — it is orthogonal to the predict/reconcile decision and unaffected by it.

---

## Appendix — decision deltas vs locked DECISIONS.md

| Decision | Native would | This review recommends |
|---|---|---|
| **D1** lightyear committed | satisfied (transport) | **UNCHANGED** — lightyear stays, as transport/netcode/sync |
| **D2** smooth-correction, never rollback | REVERSED to lightyear resim-rollback (D2 conflict by construction) | **REVERSED to hand-rolled input-replay** (`PREDICTION_RECONCILIATION.md`); ratify Q4 |
| **D3/D3a/D3b** deterministic identity | reversed for replicated entities (→ `RemoteEntityMap`/PreSpawned) | **UNCHANGED** — `GlobalEntityId` stays cross-peer name (Q2) |
| **D4** content=local-spawn, runtime=server-spawn | collapsed to all-Authoritative | **UNCHANGED** |
| **D5** time-warp host-only | unaffected | **UNCHANGED** |
| **D6** SimTick drives Tick | INVERTED on predicting clients | **UNCHANGED** (no prediction stack ⇒ no inversion) |
| **D7** wire-only optional, substrate always-on | BLED into always-on (avian plugin disables substrate physics) | **UNCHANGED / preserved** |

The net of this review: **`D1` is the only decision "via lightyear" actually requires, and it's
already met. Full native would reverse D2/D3/D4 and strain D6/D7 — to chase a crate that can't
even run our f64 physics. Recommended path keeps every decision except D2, which we reverse on
our own terms (hand-rolled input-replay over our f64 avian), not lightyear's.**
