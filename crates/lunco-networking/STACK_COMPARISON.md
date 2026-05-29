# Rover Networking — Stack Comparison & MVP Shape

Decision input (2026-05-29): **compare stacks before implementing**, target a
**thin vertical slice + cosim-value replication**. Topology: **native (or cloud)
host + browser clients over WebTransport** — see §2.5. (Originally "listen-server,
host-is-a-player"; refined because a browser tab cannot be the host.)

This doc compares the two viable Bevy-0.18 networking stacks *against what this
codebase already has*, then defines the MVP. It supersedes the transport choice
in `README.md` (which predates the typed `#[Command]` system and the
`Mutation<P>` envelope).

---

## 0. What's already built (don't re-derive)

| Asset | Where | Relevance |
|---|---|---|
| `GlobalEntityId(u64)` on **every** entity, time-sorted | `lunco-core` | The cross-process key. README assumed it's added only on `Replicated`; reality is simpler — it's unconditional. |
| `Mutation<P>` / `OpId` / `SessionId` / `Replication{Local,Authoritative,Ephemeral}` | `lunco-core::commands` | The server-authority routing envelope, already typed. `Ephemeral` = rover throttle; `Authoritative` = scene edits. |
| `#[Command]` → `Event + Reflect`, reflection dispatch, `ApiEntityRegistry` (GlobalEntityId↔Entity) | `lunco-api`, `lunco-command-macro` | The RPC layer. `DriveRover`, `BrakeRover`, `PossessVessel` already exist as typed commands. |
| Input already decoupled from physics | `lunco-controller` | `ControllerLink → VesselIntent → DriveRover/BrakeRover`. Prediction-friendly: inputs are already discrete, replayable commands. |
| avian3d 0.6 (f64), fixed 60 Hz, 33 ms clamp | `lunco-client` | Rover motion = avian forces from raycast-wheel suspension (`lunco-mobility`). |

**The two-clock split that drives everything:**

- **Chassis motion (avian)** → fast, latency-sensitive → **client-side prediction**.
- **Cosim outputs (Modelica thermal/forces, FSW ports)** → slow, computed on a
  background OS thread, **non-deterministic to replay client-side** →
  **server-authoritative state replication only, no prediction**.

Predict the chassis; replicate the rest. The MVP does both.

---

## 1. The two stacks

### Option A — lightyear (latest 0.26.x, updated for Bevy 0.18)

- **Built-in** client-side prediction + rollback + snapshot interpolation
  (`lightyear_prediction`, `lightyear_replication`, `lightyear_sync`).
- WebTransport (QUIC) on **both native and WASM** — one transport, both targets.
- Demonstrated **deterministic replication with Avian** (the exact physics we use).
- First-class **host-server / listen-server** mode (host is also a client).

**Cost for us:**
- Replaces the README's renet2+replicon commitment (auth/CCSDS bridge design
  would need re-pinning, but those are post-MVP anyway).
- Heavier, faster-moving API; its `Message`/`Channel`/`Replicate` model doesn't
  line up 1:1 with our `#[Command]` reflection layer — we'd bridge them.
- Host-client replication has had rough edges (community fix forks exist) —
  **verify listen-server replication on a spike before committing.**

### Option B — bevy_replicon (0.30.x) + renet2 via bevy_replicon_renet2

- Server-authoritative **replication only**. Prediction/reconciliation is **DIY**
  (there's a separate snapshot-interpolation crate; prediction you build on the
  `Mutation`/`OpId` envelope you already have).
- renet2 transports: UDP, **in-memory channel**, WebTransport, WebSocket.
  In-memory transport is ideal for a listen-server (host runs server+client in
  one process, zero serialization for the host's own view).
- WASM clients via `wt_client`/`ws_client`.
- Aligns with the existing README, skeleton feature flags, and the
  auth/roles/CCSDS-bridge vision.

**Cost for us:**
- You write the prediction loop: store unconfirmed inputs, re-simulate avian on
  correction. That's exactly what the README's "Client-Side Prediction" section
  sketches — but it's real work, and avian rollback by hand is the hard part.

---

## 2. Scorecard for *our* requirement (server-authoritative + client prediction, Avian, WASM, listen-server)

| Criterion | lightyear | replicon + renet2 |
|---|---|---|
| Client prediction + rollback | ✅ built-in | ❌ DIY |
| Avian integration proven | ✅ demonstrated | 🟡 manual |
| Listen-server (host-is-player) | ✅ host-server mode (verify) | ✅ in-memory transport |
| WASM | ✅ WebTransport native+wasm | ✅ wt/ws client |
| Bevy 0.18 | ✅ | ✅ |
| Maps to existing `#[Command]`/`Mutation` layer | 🟡 bridge needed | ✅ closer fit |
| Aligns with existing README/auth/CCSDS plan | ❌ re-pin | ✅ |
| API churn / risk | 🟡 higher | 🟢 lower |

**Recommendation: lightyear**, *unless* the auth/CCSDS/space-standards roadmap in
`README.md` is a near-term hard requirement. Reason: your single most specific
ask — "client-side prediction" — is precisely the thing lightyear gives for free
and replicon makes you hand-roll against f64 avian. The `Mutation`/`OpId`
envelope still earns its keep as lightyear's input message payload.

**De-risking step before locking it in:** a 1-day spike — lightyear host-server,
one predicted Avian cube driven by `DriveRover`, a second client interpolating
it. If host-client replication misbehaves, fall back to replicon+renet2 (the
envelope work carries over either way).

---

## 2.4 What "the backend" is (scope of this decision)

The backend is the library that supplies the wire mechanisms we chose **not** to
hand-write: **M2** (state replication + Predicted/Interpolated roles), **M4** (input
plumbing for prediction), **M6** (distributed clock/tick-sync — client runs ahead,
RTT offset estimation), and transport/connection lifecycle. We build M1 (identity)
and M3 (op-log via `#[Command]`/`Mutation`) ourselves regardless.

**DECIDED 2026-05-29: lightyear (see `DECISIONS.md` › D1).** lightyear ships
M2-roles + M6 first-class; replicon is M2-state only (M4/M6 DIY). Evidence was
lopsided (gap E tick-sync, gaps C/F roles, ROS2 `/clock` — all M6/prediction). The
Ph0 spike is **no longer an open A/B** — it's narrowed to verifying lightyear's one
real risk (host-client robustness under latency); if that fails, replicon+renet2 is
the manual-but-stable fallback and the `Mutation`/`OpId` envelope carries over. The
facade keeps the choice contained.

**Not a candidate:** Copper (cu29) is a deterministic robotics *runtime*. Even with
v1.0's *distributed* determinism it's the **lockstep family** — assumes every node
runs Copper, native/MCU only, **no browser/wasm**. It solves deterministic
execution+replay of a cooperating robot system, not clock-sync+prediction for
untrusted browser clients. It does not fill this gap (see `ROS2_BRIDGE.md §6b`); it
belongs on the robotics-runtime edge alongside ROS2.

## 2.5 Browser constraint & topology (decided)

**A browser tab cannot accept incoming connections and cannot do raw UDP.** It can
only make *outbound* connections over **WebTransport** (QUIC/TLS, preferred) or
**WebSocket** (TCP fallback). Therefore a browser cannot be the host.

**Decision: native (or cloud headless) host + browser clients over WebTransport.**
- Host is a native process — also where the Modelica worker already wants to live
  (background OS thread; avoids the wasm-main-thread freeze, see
  `[[project-wasm-asynccompute-main-thread]]`).
- Browser players join via WebTransport, get predicted rovers + replicated cosim telemetry.
- The host may itself be a player (native window) or fully headless — both work.

Implications:
- **WebTransport needs TLS.** Local dev requires a self-signed cert the browser
  trusts (or `serverCertificateHashes` for ephemeral certs). Plan a cert step in
  the spike. WebSocket-over-TLS is the fallback if WebTransport setup blocks.
- lightyear and renet2 both run a **native WebTransport server**; only the client
  compiles to wasm. Reuse the existing `lunco_client_web` wasm build path.
- **Not chosen:** fully in-browser P2P (WebRTC via matchbox + bevy_ggrs). That
  needs deterministic lockstep, which reintroduces the cosim-determinism problem
  we deliberately avoided. Revisit only if serverless deployment becomes required.

---

## 3. MVP scope — slice + cosim values, native host + browser clients

**Goal:** Host runs the sandbox and drives a rover. A second client joins, sees
the host's rover move (interpolated) and its own rover move (predicted), and sees
the rover's Modelica/cosim telemetry (thermal/forces) streamed read-only.

### Replication set (server → clients)
- `Transform` (or `Position`/`Rotation`) of vessels — predicted for owner, interpolated for others.
- Drive state (`DifferentialDrive`/`AckermannSteer` setpoints) — for prediction inputs.
- `RoverVessel`/`Vessel` markers + `GlobalEntityId` — identity.
- **Cosim outputs** (`AvianSim.outputs`, `SimComponent.outputs`) — server-authoritative telemetry, no prediction.

### Stays local (never replicated)
- Cameras (`SpringArmCamera`, `FreeFlightCamera`), `ControllerLink`, selection/gizmo state.
- `Wire.source/target`, `port_map`, `PendingWheelWiring` — rebuilt from the same USD on both sides.
- The Modelica background worker — **runs on the host/server only**; clients receive values, never run rumoca.

### Control flow
1. Client possesses a rover → `PossessVessel` command to server → server sets ownership.
2. Owner client: input → `DriveRover` (the `Ephemeral` `Mutation`) applied **locally immediately** (prediction) **and** sent to server with a sequence number.
3. Server applies authoritative avian step, replicates `Transform`.
4. Owner reconciles (snap + replay unconfirmed inputs); non-owners interpolate.

### Explicitly deferred (post-MVP, per README)
Auth/roles/ACL, interest management, compression stack, edit-log collab + Yjs,
CCSDS/XTCE/YAMCS bridge.

### Milestones
1. **Spike** — lightyear native WebTransport server + 1 predicted Avian body via `DriveRover`, one **browser** client connecting (incl. local TLS cert). Decide stack.
2. **Replicate transforms** — host + joining client see each other's rovers (interpolated).
3. **Predict owner rover** — local input prediction + server reconciliation.
4. **Replicate cosim values** — stream `AvianSim`/`SimComponent` outputs read-only; show in a panel.
5. **Possession over network** — `PossessVessel`/`ReleaseVessel` set replicated ownership.

---

## Sources
- [lightyear (GitHub)](https://github.com/cBournhonesque/lightyear) · [docs.rs](https://docs.rs/lightyear/latest/lightyear/) · [releases](https://github.com/cBournhonesque/lightyear/releases)
- [lightyear host-client replication fix fork](https://github.com/Ploruto/lightyear-fix-host-client-replication)
- [bevy_replicon (GitHub)](https://github.com/projectharmonia/bevy_replicon) · [crates.io](https://crates.io/crates/bevy_replicon)
- [bevy_replicon_renet2 (docs.rs)](https://docs.rs/bevy_replicon_renet2) · [renet2 (GitHub)](https://github.com/UkoeHB/renet2)
