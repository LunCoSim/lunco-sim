# Spec 034 — Control Authority: Autopilot as a User

**Status**: Proposal (rev 2)
**Owner**: —
**Relates to**: [`001-vessel-control-architecture`](../001-vessel-control-architecture/spec.md) (the port-write control path), [`010-authority-rbac`](../010-authority-rbac/spec.md) (possession + the `authorize` gate this reuses)

> **Rev 2 supersedes rev 1.** Rev 1 was written against `DriveRover → on_drive_rover`, a command + sink that the networking/USD merge **deleted**. It proposed a bespoke local `ControlAuthority` component with a per-frame acquire/expire arbiter. This revision drops all of that. The post-merge substrate already has everything needed — one owner per vessel (`SessionRegistry`), one gate on the control write (`authorize` / `OWNED_CONTROL`), and rhai-authored authorization (`rbac.authorize`). So the design collapses to a single idea:

> **An autopilot is just a user with a specialty.** It is an `AiAgent` session whose "input device" is a rhai script instead of a keyboard. It possesses a vessel through the same path a human does. Possession *is* the authority; the existing control-write gate *is* the arbiter. Nothing is checked every frame — ownership changes only on a possess event.

---

## 1. Problem

A rover jitters when a script and a human drive it at the same time.

Post-merge, control is the **one generic command** `SetPorts` (`crates/lunco-cosim/src/lib.rs:230`): a batch of named input-port writes on a vessel's `FlightSoftware` command surface (`throttle`/`steer`/`brake`). A static `DriveMix` + kernel then projects that surface onto actuator ports in `apply_drive_mix` (`crates/lunco-mobility/src/lib.rs:910`, a `FixedUpdate` system). Two emitters write that same command surface every fixed tick:

- **Human keyboard** — `drive_from_bindings` (`crates/lunco-controller/src/lib.rs:86`) emits one `SetPorts` per fixed tick per `ControllerLink`, writing *every* bound port (0 when idle).
- **Rhai autopilot** — the prelude `drive()` / `nav_to()` verbs (`assets/scripting/prelude/control.rhai`, `nav.rhai`) emit `SetPorts` every `on_tick`.

Both land through `on_set_ports` (`lunco-cosim/src/lib.rs:253`) into the same FSW input ports. `apply_drive_mix` then reads whatever value is currently in those ports. When the two disagree, the last write of the tick wins and the setpoint flips tick-to-tick → the wheels oscillate.

### Why the existing ownership doesn't already fix it

The substrate **does** have single-owner control — but two things stop it from covering the local human-vs-autopilot case:

1. **Ownership is keyed only by network `SessionId`.** `SessionRegistry` (`lunco-core/src/session.rs:181`) maps `vessel_gid → SessionId`; `authorize` (`:752`) rejects a `SetPorts` whose origin session isn't the owner (`SetPorts` is seeded `OWNED_CONTROL`, `:703`). But a local human and a locally-run rhai autopilot share **one** session, so both pass identically. There is no actor identity *below* the session — until we give the autopilot its own session (this spec).
2. **Single-player never reaches `authorize`.** The gate runs host-side in the wire-capture path; in `Standalone` capture no-ops (`session.rs:751`). So on a local sandbox — exactly where the tutorial autopilot + human collide — there is no owner gate at all.

### Prerequisite bug (independent of this spec)

The merge deleted `DriveRover`/`BrakeRover`, but `assets/scripting/prelude/control.rhai` still emitted `cmd("DriveRover", …)` / `cmd("BrakeRover", …)`. Scripted driving was therefore a **no-op**. Fixed alongside this spec: `drive()`/`brake()` now emit `cmd("SetPorts", { target, writes:[["throttle",…],["steer",…],["brake",…]] })`, writing all three command ports every tick (mirroring the keyboard path) so a prior `brake` doesn't stick.

## 2. Goals / Non-goals

**Goals**
1. Exactly one source drives a vessel's command surface per tick — no competing writes, no jitter.
2. Autopilot is a **first-class actor**: an `AiAgent` session you engage/disengage; when engaged it owns the vessel and drives it via a rhai script.
3. Human and autopilot arbitration is **possession** — decided on an event (a possess/claim), never a per-frame comparison, timer, or grace-tick scan.
4. **Who may take control from whom is authored in rhai** and hot-distributed to peers — no compiled priority ladder.
5. No new control taxonomy: reuse `SessionRegistry`, `authorize`, `AuthorityRole::AiAgent`, `rbac.authorize`, and `SetScriptedPolicy`. (Matches the standing "less Rust / more dynamic registries" direction.)

**Non-goals**
- Changing `SetPorts`, `apply_drive_mix`, `DriveMix`, kernels, or the physics/actuator model. Arbitration is entirely at "which session owns the vessel."
- A per-frame arbiter system with holder state / idle grace / expiry (explicitly rejected — that was rev 1).
- Multi-operator cross-session conflict beyond what possession + `authorize` already give.

## 3. Requirements

**FR-1** A vessel has at most one owner (`SessionRegistry`, already true). Only the owner's `SetPorts` reaches the FSW surface.
**FR-2** An **autopilot** is a session with `role = AiAgent` and a bound rhai behaviour + a target vessel. Engaging it makes it `claim` the vessel; disengaging `release`s it.
**FR-3** While an autopilot owns a vessel, the local human's `drive_from_bindings` does **not** emit for that vessel (it yields) — a single `owner_of` lookup, no timer.
**FR-4** A human takes control by issuing `PossessVessel` (the existing possess path). Whether that steal is allowed is decided by the rhai `rbac.authorize` hook (per the chosen "rhai decides stealing"), keyed on `{ role, owns_target }`.
**FR-5** The owner gate holds in **all** modes — including `Standalone` — so the local sandbox is arbitrated, not just networked play.
**NFR-1** Zero added cost on a vessel with a single controller and on headless/CI (no autopilot session ⇒ no owner ⇒ unchanged path).
**NFR-2** Deterministic and network-safe: possession is already host-authoritative and broadcast; the authorization hook is already broadcast via `SetScriptedPolicy` so every peer converges.

## 4. Design

### 4.1 Autopilot = an `AiAgent` session

`AuthorityRole::AiAgent` already exists (`session.rs:290`) and is currently **unused** — a ready-made role slot. An autopilot is registered exactly like a connecting user. It is **not** part of the avatar and has no UI/camera dependency: it lives in its own **headless** crate (`crates/lunco-autopilot`, deps `lunco-core` + `lunco-cosim` only), so a `--no-ui` server runs autopilots identically to the GUI.

- A reserved `SessionId` band (`AUTOPILOT_SESSION_BASE + index`, distinct from `SessionId::LOCAL` and host-minted client ids), inserted into `SessionRbac` as an authenticated, token-bearing `UserSession { role: AiAgent }` so `is_authorized` passes (`session.rs:337`). One session **per autopilot**, so the model is inherently **multi-actor**: many vessels, each owned by a different session (some human, some autopilot).
- A tiny component marking the autonomous driver:

```rust
// crates/lunco-autopilot/src/lib.rs
#[derive(Component, Debug, Clone)]
pub struct Autopilot {
    pub vessel: Entity,       // the vessel it drives
    pub session: SessionId,   // its own AiAgent identity (distinct per actor)
    pub engaged: bool,        // armed?
    pub throttle: f64,        // fallback setpoint when no AutopilotBehavior tree
    pub steer: f64,           //   is attached (see the behaviour-tree note below)
}
```

The autopilot is **structurally a user**. Its only specialty: its setpoint comes from a behaviour, not a keymap.

**The behaviour is a [`lunco-behavior`] tree, authored as data.** The *what to do* is an `AutopilotBehavior` component holding a behaviour tree. The tree STRUCTURE (sequence waypoints, fallbacks, when-to-brake) is the **glue**, authored as DATA (`BehaviorSpec`, an internally-tagged serde enum — so rhai/JSON define it) and compiled by `build_tree`. Its leaves are **Rust** primitives (`nav_setpoint` steering math) — the split the project mandates: *computation in Rust, glue in rhai*. Because the tree is data, it is dynamic: the `SetAutopilotBehavior { vessel, spec_json }` command lets a rhai scenario define or **hot-swap** a vessel's behaviour at runtime — different autopilots, updated on the fly, no rebuild. This is the first consumer of the (previously unwired) `lunco-behavior` kernel; enabling per-entity storage required making the kernel's boxed children `Send + Sync` (a `BoxNode` alias — no new deps). With no `AutopilotBehavior` attached, the autopilot falls back to constant `throttle`/`steer` setpoints.

[`lunco-behavior`]: ../../crates/lunco-behavior

### 4.2 Possession is the arbiter (no per-frame check)

- **Engage** → register the `AiAgent` session + `SessionRegistry::claim(session, vessel_gid)` (on spawn of the component). A refused claim (vessel already owned) leaves it disengaged rather than fighting.
- **Autopilot drives** while engaged **and** it still owns the vessel: `drive_autopilots` (FixedUpdate) emits one `SetPorts` per owned vessel — the single writer that tick.
- **Human drives only what it owns** (FR-1/FR-3): `drive_from_bindings` skips any vessel whose owner is a session `≠ LocalSession`. Whether that other owner is a remote player or an autopilot's `AiAgent` session is irrelevant — you drive what you own. One `SessionRegistry::owner_of` lookup at the existing emit point, **not** a new system, no timer, no holder state. A vessel with no other owner is unaffected (NFR-1).
- **Human takes over** (FR-4): the human `PossessVessel`s the vessel; the claim flips ownership to the human session (see §4.4 for the steal decision). The autopilot's `owns` check then goes false, so it stops writing on the next tick. **Losing ownership IS the disengage signal** — no autopilot-side polling, and the autopilot never needs to know *who* took over or by what command.
- **Disengage** → `release`, dropping the vessel; a human (or nothing) may take it.

Ownership changes only on these events. Between events there is nothing to evaluate. Because the only shared truth is `SessionRegistry` (in `lunco-core`), the autopilot never depends on the avatar/possession crate — it and the human path are decoupled through core.

### 4.3 The one real change — honor possession in all modes

The single gap is FR-5: `Standalone` skips `authorize`, so locally the human and autopilot both write freely. Close it at the **one emit choke** without touching the sink:

- `drive_from_bindings` (`lunco-controller/src/lib.rs`) takes `Option<Res<SessionRegistry>>` + `Option<Res<LocalSession>>`. Before emitting for `link.vessel_entity`, if `owner_of(gid)` is some session `≠ LocalSession`, `continue` (yield). Cheap, mode-independent, additive (a vessel owned by nobody or by us is unaffected), and `Option` so a controller-only test app without the session substrate still runs.
- The autopilot's own emit is gated symmetrically in `drive_autopilots`: it drives only a vessel it owns.

That is the whole enforcement in single-player. In networked play the host `authorize` gate is the airtight backstop (an autopilot on one peer can't write a vessel owned by another session); the local yield just avoids emitting a doomed command and keeps prediction clean.

### 4.4 "Rhai decides stealing"

Who may `PossessVessel` a vessel currently held by an autopilot is **not** hardcoded. It rides the existing scripted-authorization plane:

- A rhai `fn allow(ctx)` registered under `AUTHORIZE_HOOK` (`"rbac.authorize"`, `session.rs:676`) receives `{ session, capability, target, owns_target, role }` and returns a bool. It can only **tighten** (fail-closed), so it never weakens the compiled floor.
- Distribution is `SetScriptedPolicy { kind: Authorize, … }` (`crates/lunco-networking/src/scripted_policy.rs`): host-authoritative, compiled once, **broadcast to every peer on connect and on change**, so late joiners converge.
- Example policy: *"a human `Operator` may steal a vessel from an `AiAgent`; an `AiAgent` may not steal from a human."* Expressed in a few lines of rhai, hot-swappable, no recompile.

Because `PossessVessel` arbitration already runs through `SessionRegistry::claim` + `authorize`, "who controls, decided by scripting" needs **no new mechanism** — only the `allow` snippet.

**The takeover rule is rhai, not Rust — in all modes.** When a `PossessVessel` targets a vessel another session already owns, `record_possession_authority` (`lunco-avatar`) asks a rhai policy — `lunco_core::session::may_take_control` → the `CONTROL_AUTHORITY_HOOK` (`"control.authority.take"`) — whether the takeover is allowed. If yes, it releases the prior owner first so the claim succeeds under `Exclusive` (one vessel per autopilot session, so the release frees exactly that vessel); the released autopilot then loses `owns` and stops. The rule itself lives in **`assets/scripting/policy/control_authority.rhai`**:

```rhai
fn may_take_control(ctx) {           // ctx: #{ taker, taker_role, owner, owner_role, target }
    if ctx.owner_role == "AiAgent" { return ctx.taker_role != "AiAgent"; }  // human may take from autopilot
    false                            // a human-held vessel is not stealable
}
```

It is registered under the hook id at startup by `lunco-scripting` (`register_builtin_policies`, from the embedded `assets/scripting/policy/` dir) and **hot-replaceable** by any later `SetScriptedPolicy { Authorize }` broadcast. The Rust seam (`may_take_control`) only marshals context and **fails closed** (no policy ⇒ no takeover). So the "who may steal from whom" decision is authored data, tunable without a rebuild — the same mechanism networked play uses, now the default everywhere.

### 4.5 Flow

```
engage      : register AiAgent session + claim(autopilot_session, vessel). autopilot owns vessel.
tick N      : drive_autopilots → SetPorts. autopilot owns ⇒ applied.
              human drive_from_bindings: owner ≠ me ⇒ yield (no emit).
human grabs : PossessVessel(vessel). owner is AiAgent ≠ me ⇒ release it (rhai-gated when networked),
              then claim(human_session, vessel). human owns vessel.
tick N+k    : drive_autopilots: !owns ⇒ autopilot stops writing.
              human drive_from_bindings: owner == me ⇒ emit. human drives.
```

One owner ⇒ one writer per tick ⇒ no competing port writes ⇒ no jitter. Different vessels can be owned by different actors at once (some human, some autopilot) with no interference — the model is multi-actor by construction.

## 5. Composition with existing layers

- **Possession + `authorize` (spec 010)** are reused verbatim; this spec only *populates* them with an autopilot session and adds the local-mode yield.
- **Tutor mode** stays orthogonal: it seizes a peer's avatar camera/input, not vessel ownership — unchanged.
- **Networked prediction**: possession is already replicated (host broadcasts the owner table); the authorization hook is broadcast via `SetScriptedPolicy`. Nothing new to replicate.

## 6. Alternatives considered

- **Bespoke `ControlAuthority` component + per-frame acquire/expire arbiter (rev 1)** — a second ownership concept parallel to `SessionRegistry`, plus an always-on expiry system. Rejected: duplicates possession, adds per-frame cost and holder/grace state, and doesn't reuse the rhai-authorization the merge already shipped.
- **`source` tag on `SetPorts` + guard in `on_set_ports`** — makes the sink source-aware. Rejected for now: `SetPorts` is the networked control command with prediction fields; adding an origin tag is invasive, and possession already answers "who may write this vessel." Revisit only if a rogue non-possessor emitter appears.
- **Priority ladder baked in Rust (Owner > Operator > AiAgent > Observer)** — the `AuthorityRole` lattice already encodes the floor; the *stealing* policy on top is deliberately rhai (FR-4) so a deployment tunes it without a rebuild.

## 7. Task breakdown

1. **Prelude fix (prerequisite — DONE):** `control.rhai` `drive()`/`brake()` → `SetPorts`; `nav.rhai` doc. Unblocks scripted driving. (`drive()` writes throttle/steer/brake=0; `brake()` writes brake only, since the mix zeroes throttle/steer under brake.)
2. **Headless `lunco-autopilot` crate (DONE):** `Autopilot` component; `setup_autopilot_session` registers a reserved `AiAgent` `UserSession` + `claim`s the vessel; `drive_autopilots` emits `SetPorts` while `engaged && owns`. Deps `lunco-core` + `lunco-cosim` only. `AutopilotPlugin` added on the control path in `luncosim` + `lunco-sandbox` (so `--no-ui` runs it).
3. **General ownership yield (DONE):** `drive_from_bindings` skips any vessel owned by a session `≠ LocalSession`. The single `lunco-controller` change; `Option`-guarded.
4. **Takeover rule authored in rhai (DONE):** `assets/scripting/policy/control_authority.rhai` (`may_take_control`), registered at startup by `lunco-scripting::register_builtin_policies` under `CONTROL_AUTHORITY_HOOK`; consulted by `record_possession_authority` via `lunco_core::session::may_take_control` (fail-closed). No hardcoded steal rule in Rust; hot-replaceable via `SetScriptedPolicy`.
5. **Behaviour = data-authored `lunco-behavior` tree (DONE):** `BehaviorSpec` (serde, rhai/JSON) → `build_tree` → `AutopilotBehavior` component ticked in `drive_autopilots`; Rust leaves (`nav_setpoint` math); hot-swap via the `SetAutopilotBehavior` command. Made the `lunco-behavior` kernel `Send + Sync` (`BoxNode`) for per-entity ECS storage.

## 8. Testing

- **Headless mechanism (DONE — `crates/lunco-autopilot/tests/authority.rs`):** autopilot engages → registers an `AiAgent` session + owns its vessel + drives it; **stops the instant it loses ownership** (simulated takeover) — the single-writer / no-jitter invariant; two autopilots own distinct vessels and drive only their own (multi-actor non-interference). Runs on `Standalone` with no avatar/UI.
- **Behaviour tree (DONE — `crates/lunco-autopilot/tests/behavior.rs`):** a JSON-authored `BehaviorSpec` (the exact shape rhai emits) compiles to a tree that drives toward a waypoint and **advances the sequence** on arrival; `nav_setpoint` brakes within radius / drives when far; malformed specs error cleanly. Plus `lunco-behavior`'s own 11 kernel tests (now `Send + Sync`).
- **Integration (future):** full `PossessVessel` takeover through `record_possession_authority` + the `lunco-controller` yield, asserting the human emits no `SetPorts` for an autopilot-owned vessel and takes over on possess.
- **Regression (future):** the `first_drive` tutorial (autopilot + possess) drives to the flag without wheel oscillation.

## 9. Open questions

- **Reserved `SessionId` band for autopilots** — pick a range that can't collide with `SessionId::LOCAL` or host-minted client ids. (One local autopilot per vessel to start.)
- **Default `PossessionPolicy` for autopilot vessels** — `Exclusive` (deliberate takeover) vs `LastWins` (grab-to-steal). The stealing `allow` policy composes with either; default likely `LastWins` for the "grab the stick" feel, gated by the rhai policy.
- **Should the human yield be automatic on input, or only on explicit `PossessVessel`?** Rev 2 chooses explicit possess (event-driven, no input-edge detection). Auto-steal-on-first-input is a later nicety layered on the same possess call.
