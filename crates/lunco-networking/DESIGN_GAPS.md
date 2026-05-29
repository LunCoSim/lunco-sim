# What we're missing — gap analysis vs. how others do it

The transport/replication/auth docs cover *plumbing*. They are nearly silent on
the problems that actually make networked physics hard: **time sync, the
reconciliation model against a non-deterministic solver, large-world coordinates,
and the cosim coupling**. This doc names those gaps, with how the reference
engines/games handle each, and what we specifically need.

---

## 0. First, pin the model (and rule out the wrong ones)

| Model | Used by | Fits us? |
|---|---|---|
| **Deterministic lockstep** (send only inputs, everyone simulates identically) | Factorio, RTS, GGPO | ❌ — avian is **not** cross-platform deterministic (desktop host vs wasm client: different float/SIMD), and Modelica runs async on a background thread. Lockstep is impossible here. |
| **State replication + client prediction** (server authoritative, predict local pawn, interpolate the rest) | Source, Overwatch, **Unreal**, Unity NGO, lightyear | ✅ — this is us. |
| **Full physics rollback** (deterministic physics + rollback everything) | Rocket League | ⚠️ partial — we can predict-and-correct, but **not** roll back the whole avian world (global solver, non-determinism). |

So we're squarely in the **Unreal model**. Adopt its vocabulary as our per-entity
replication *role* — this is the missing organizing concept:

- **Authority** — the server. Ground truth.
- **AutonomousProxy** — the entity *this* client controls → **predicted** locally.
- **SimulatedProxy** — everything else → **interpolated** from snapshots.

lightyear exposes exactly this (`Predicted` / `Interpolated`). Our possession
system decides, per client, which rover is AutonomousProxy.

---

## CRITICAL gaps (project-specific — no library solves these for us)

### A. Large-world coordinates (`big_space`) replication  ← biggest gap
We planned to replicate `Transform`. **That's wrong here.** Position is
`CellCoord` (grid cell) **+** `Transform` (offset within cell), and **each client
re-bases its floating origin independently** around its own camera. A raw
`Transform` means nothing without its cell, and cells differ per client.

- **How others handle it:** Star Citizen (zone-relative 64-bit positions
  replicated as zone id + local offset), KSP multiplayer mods (Krakensbane /
  reference-frame rebasing), Dual Universe (planet-relative construct coords).
- **What we need:** the authoritative pose is `(CellCoord, Transform)`; replicate
  **both**. Each client maps server cell+offset into *its own* local origin on
  receipt. Bonus: the within-cell offset is **bounded**, so position quantization
  gets *easier and cheaper* than the README's generic scheme (quantize a known
  small range; send the cell as ints).

### B. Stable identity must be **deterministic from USD**, not random
The README's plan is "replicate state, reconstruct topology from USD" — both sides
load the same scene and **don't** re-spawn replicated entities. But `GlobalEntityId`
is currently minted from `make_id_53()` (time/random). Two processes loading the
same prim get **different ids** → the replication layer can't match them → double
entities or orphaned state.

- **How others handle it:** Unreal net-stable names for level actors; any
  asset-instanced netcode keys by a content path / GUID, not a runtime handle.
- **What we need:** for USD-spawned entities, derive `GlobalEntityId` **from the
  USD prim path** (stable hash), so server and client independently converge on
  the same id with zero coordination. Reserve random ids for *runtime-spawned*
  entities, which the **server alone** spawns and replicates. Decide the rule
  explicitly: *USD-instanced = deterministic id, locally spawned; runtime = server
  spawns + replicates.*

### C. Cosim coupling decides prediction eligibility
Prediction works only if the client can compute the same forces the server does.
- A **driven rover**: motion is dominated by wheel drive/suspension = **local
  avian forces** the client *can* recompute. Thermal cosim doesn't move it. →
  **Predictable.**
- The **balloon / buoyancy / thrust** entities: motion comes from **Modelica
  forces computed server-side only**. The client has no way to predict them. →
  **Must be SimulatedProxy (interpolated), never predicted.**

- **How others handle it:** Unreal predicts only the autonomous pawn's *movement
  component*; anything driven by server-only logic is a simulated proxy. Nobody
  predicts a value they can't locally compute.
- **What we need:** make "is this entity's motion locally computable?" a
  **first-class input** to the predict/interpolate decision — not just "do I own
  it." A rover I own but whose motion is cosim-driven is still interpolated.

### D. Simulation clock + time-warp must be replicated
This is a *simulation*, not just a game. It has sim-time, an output interval, and
`TimeWarpState` (pause / speed). If the host pauses or warps, clients must agree —
otherwise every position desyncs.
- **How others handle it:** KSP **forbids** time-warp in multiplayer; Factorio ties
  game speed to the lockstep and votes on it. Sim tools make the clock
  server-authoritative.
- **What we need:** server owns sim-time + warp state; replicate it; **decide the
  MP policy** (simplest for MVP: warp/pause allowed only by host, applied to all;
  or disabled in MP entirely). Inputs/snapshots are stamped in sim-ticks, not wall
  time.

---

## IMPORTANT gaps (standard netcode — the backend helps, we still must design)

### E. Tick/clock synchronization (the foundation we hand-waved as "sequence numbers")
Server-auth + prediction needs a **shared fixed tick**, with the **client running
~RTT/2 + jitter buffer *ahead*** so its inputs land at the server just before that
tick is simulated. The offset is adjusted continuously (speed the client clock up
/ slow it down) as RTT drifts.
- **How others:** Overwatch "command frames", Source `cl_cmdrate`/interp, GGPO
  frame timing.
- **What we need:** a tick clock + offset estimator. **lightyear ships this
  (`lightyear_sync`)** — a strong reason to lean lightyear. With replicon we build it.

### F. Reconciliation strategy: smooth-correct, not full rollback
On a server snapshot, the predicted rover is corrected. Full rollback re-simulates
avian from the confirmed state replaying pending inputs — but avian's solver is
**global** (contacts couple bodies), so rolling back one rover needs the whole
contact island at that tick. Static terrain is fine; other dynamic bodies are not.
- **How others:** FPS reconcile only the local movement component; racing games
  with full determinism roll back everything.
- **What we need (MVP):** predict the rover kinematically and **error-correct
  toward the server state** (position/velocity blend, "projective velocity
  blending") rather than full physics rollback. Accept small corrections on
  rover-rover contact. Revisit true rollback only if corrections feel bad.

### G. Input redundancy + server-side jitter buffer
Inputs go on an **unreliable** channel (loss happens). A dropped input = a hitch.
- **How others:** Quake/Source resend the last N commands every packet; servers
  buffer inputs and consume one per tick.
- **What we need:** each input packet carries the last *N* unacked inputs (cheap,
  they're tiny); server keeps a small per-client input buffer to absorb jitter.

### H. Interpolation buffer for simulated proxies
Remote rovers render **in the past** (`now − interp_delay`, ~2 snapshots) to hide
jitter and cover gaps.
- **What we need:** a snapshot buffer + interp delay; **reconcile with the
  *existing* `TranslationInterpolation`/`RotationInterpolation` components** (used
  today for 60Hz→render smoothing) so we don't double-interpolate. lightyear's
  `Interpolated` would replace them for networked entities.

### I. Late-join baseline sync
A client joining mid-session needs: which **USD scene** (handshake → load by id
from shared assets, don't stream geometry) + a **full snapshot** of dynamic entity
states and current cosim values + the sim clock.
- **What we need:** a join handshake (`scene_id`, sim-tick, warp state) then the
  backend's initial full-state replication for dynamic entities. Ties directly to
  gap B (ids must match the locally-loaded USD).

---

## DEFER (acknowledge, don't build yet)
- **Rover↔rover collision under prediction** — two predicted/authoritative rovers
  touching causes corrections. MVP: accept the snap, or disable inter-player rover
  collision. (Source/Overwatch simply don't predict pawn-vs-pawn physics.)
- **Lag compensation** (server rewind) — only needed if precise rover-rover or
  tool interaction matters. No shooting → low priority.
- **Input validation / anti-cheat** — LAN co-op; clamp inputs server-side, defer
  real validation.
- **Network condition simulation** (latency/jitter/loss injection) — dev tooling;
  both backends have link conditioners. Add early for testing E/F.

---

## So, what were we missing? (summary)

1. **A coordinate model** — `big_space` cell+offset replication and per-client
   origin rebasing. (Gap A — the biggest, and unique to us.)
2. **A deterministic identity rule** — USD-derived ids vs server-spawned ids, to
   reconcile "both load USD" with "server replicates spawns". (Gap B.)
3. **Prediction eligibility from cosim coupling** — predict avian-driven motion,
   interpolate cosim-driven motion, even for entities you own. (Gap C.)
4. **A replicated sim clock + warp policy.** (Gap D.)
5. **A real time-sync/tick model** instead of bare "sequence numbers." (Gap E.)
6. **A concrete reconciliation choice** — smooth-correct, not full avian rollback,
   given the global solver. (Gap F.)
7. The standard supporting pieces: input redundancy + jitter buffer, interpolation
   buffer (folded into existing interpolation), late-join baseline. (G–I.)

These reshape the plan: the hard work isn't the transport (mostly the backend's
job) — it's **A, B, C, D**, which no networking library solves out of the box.

---

## Open decisions these gaps surface — **RESOLVED 2026-05-29 (see `DECISIONS.md`)**
1. **Reconciliation:** smooth error-correction; full avian rollback ruled out by construction. → **D2**
2. **Time-warp in MP:** host-only, applied to all; forbidden when ROS owns a vessel. → **D5**
3. **Spawn authority:** confirmed — content = deterministic id + local spawn; runtime = server spawns + replicates. → **D4**
4. **Backend:** lightyear committed; spike narrowed to host-client robustness only. → **D1**
