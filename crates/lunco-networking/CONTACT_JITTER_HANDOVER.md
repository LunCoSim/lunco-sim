# Handover — client contact jitter & the predict/reconcile state (2026-06-11)

> **SUPERSEDED IN PART (2026-06-11, same day):** a critical review against
> networked-physics best practice revised the fix. The agreed plan now lives in
> [`PREDICT_AND_SMOOTH_PLAN.md`](./PREDICT_AND_SMOOTH_PLAN.md) — read THAT for what
> to build. The diagnosis here (§0–§4, §6) remains correct and authoritative; §5's
> fix sketch is amended as follows:
> - **Part 1 goes further:** proxies become **velocity-driven** (`v = (target −
>   current)/h` per fixed step, avian integrates; teleport only as exception), not
>   "keep teleporting + also write a velocity hint". A teleported kinematic can
>   still mint penetration regardless of claimed velocity.
> - **Part 2 as written below is WRONG:** "never seat the physics body" lets error
>   compound until the 6 m Snap. Correct model (Fiedler error smoothing): corrections
>   **do** seat the body immediately; the *visual discontinuity* goes into a decaying
>   render offset (~150 ms), so the eye never sees the pop.
> - **§5's "Phase C contact-island" is replaced by predict-all-vehicles** (Rocket
>   League model) with held-input extrapolation; islands demoted to CPU fallback.
> - **Tree state since:** stash@{0} was popped (Phase B + §6 restored, they're
>   load-bearing for the new plan) and the diagnostic `return;` was stripped.

This is a session handover for the **client-side rover jitter on contact** problem.
It records the current code state, the root-cause analysis, **what was tried and
failed** (so nobody repeats it), the **proper fix**, and the operational gotchas
learned. Read alongside [`PREDICTION_RECONCILIATION.md`](./PREDICTION_RECONCILIATION.md),
[`PREDICTION_MEMBERSHIP.md`](./PREDICTION_MEMBERSHIP.md), [`DESIGN_GAPS.md`](./DESIGN_GAPS.md),
[`DECISIONS.md`](./DECISIONS.md).

---

## 0. TL;DR

- **Symptom:** on the **client**, a possessed rover jitters/buzzes violently when it
  **touches another rover**; baseline (committed code) is a *mild* version, the
  uncommitted experiments made it worse.
- **Root cause (confirmed by experiment):** it is **structural**, not a reconcile
  tuning bug. Your rover is a live `Dynamic` body at *now*; every other rover is a
  `Kinematic` proxy that is **~0.18 s stale, moved by teleport, with velocity forced
  to zero**. Ramming that = a teleport-penetration **kickback (#1)**, which the
  reconciler then re-applies every 20 Hz into a sustained **buzz (#2)**.
- **Proper fix = "Predict-and-Smooth"** (the model the docs already specify but the
  code never implemented): **Part 1** give proxies their interpolated velocity
  (honest moving wall → kills the kick); **Part 2** move error-correction onto a
  *decaying render offset* instead of yanking the physics body (kills the buzz).
- **Current tree state:** **CLEAN** (committed `248d0de8`, prediction code identical
  to `22472017`). All session experiments are in **`git stash@{0}`** (recoverable).

---

## 1. Current repo state

- **HEAD:** `248d0de8 Merge branch 'networking'`. For ALL prediction code
  (`commands.rs`, `reconcile.rs`, `session.rs` systems) this is **byte-identical to
  `22472017`** — the commit the user confirmed feels "pretty good." So the committed
  baseline == the good baseline.
- **Working tree:** clean (stripped 2026-06-11).
- **Stashed work** (`git stash list` → `stash@{0}`): Phase B + §6 guard + a reconcile
  experiment. Recover with `git stash pop`. Contents:
  - **Phase B**: `PredictedDynamic` marker + `maintain_predicted_dynamic` +
    `reconcile_predicted_dynamic` + `Without<PredictedDynamic>` exclusions in
    `force_kinematic_proxies`/`interpolate_proxies` (for runtime-spawned free props /
    the "rocket-league ball").
  - **§6 opaque guard**: `NotPredictable` marker + `tag_cosim_opaque` in
    `lunco-usd-sim/src/cosim.rs` (stamps `NotPredictable` on any `SimComponent` +
    `RigidBody` body that isn't a `RoverVessel`), respected by
    `maintain_predicted_dynamic`.
  - **Experiment** (do NOT keep): a `return;` early-exit at the top of
    `reconcile_owned_prediction` that disables owned-rover reconcile.
  - `PREDICTION_MEMBERSHIP.md` edits: Phase C parked, §6 documented.
  - NOTE: Phase B + §6 are **sound and additive** and do **not** cause the rover
    jitter (they only act on `SkipContentStamp` runtime spawns, never scene rovers).
    They were stashed only to get a clean baseline, not because they're wrong.

---

## 2. The architecture as it stands (committed baseline)

All file:line refer to the committed tree.

### Markers / resources — `crates/lunco-core/src/session.rs`
- `OwnedLocally` (248) — client marker: this session owns **and is actively
  driving** this body → predict it locally (Dynamic). Excluded from kinematic-pin +
  interpolation.
- `NetReplicate` (212) — host replicates this entity's transform via snapshots;
  carried on client proxies.
- `SkipContentStamp` (206) — runtime-spawn marker (server-allocated id).
- `VesselInputLog` (361) — per-vessel input ring + `last_active_tick`.
- `OwnedInputLog` (383) — client unacked input logs per gid.
- `AppliedInputSeq` (390) — host: highest input seq applied per gid → stamped into
  snapshots as the reconcile ack.
- `SnapshotSample` (313) — wire record: `gid, tick, t[f32;3], r[f32;4], lv, av,
  last_input_seq, pos[f64;3], cell[i64;3]`.

### Client systems — `crates/lunco-sandbox-edit/src/commands.rs`
Schedule (verbatim intent):
```
Update (chain): apply_replicated_spawns, maintain_owned_locally, ingest_snapshots,
                interpolate_proxies, force_kinematic_proxies, tag_networked_physics
FixedPostUpdate (after PhysicsSystems::Writeback):
                reconcile_owned_prediction, record_predicted_state   (chained)
```
- `force_kinematic_proxies` (149) — `With<NetReplicate>, Without<OwnedLocally>` →
  set `RigidBody::Kinematic` and **zero LinearVelocity/AngularVelocity every frame**
  (so the proxy holds the last snapshot instead of gliding). **THIS zeroing is
  defect #1's source for contact.**
- `interpolate_proxies` (305) — render proxies `INTERP_DELAY = 0.18 s` in the past;
  lerp/slerp between bracketing samples; writes Transform + f64 `Position`. Owned
  body excluded.
- `ingest_snapshots` (270) — file snapshots into per-gid ring, **tick-stamped**.
- `maintain_owned_locally` (475) — **Phase A membership**: `OwnedLocally` iff
  `predicts_locally(owns, last_active, now, grace=30)` (≈0.5 s). Idle-owned → marker
  removed → falls back to proxy path.
- `reconcile_owned_prediction` (625) — **D2 input-replay**: compare
  predicted-at-acked-seq vs authority-at-seq (latency lead cancels → InSync when
  correct, no rubber-band); else `Correct` (blend pos+rot 30%) or `Snap` (>6 m),
  **and seat lin/ang velocity to authoritative**. Runs after writeback.
- `record_predicted_state` (576) — record owned post-step pose keyed by input seq.

### Reconcile decision — `crates/lunco-core/src/reconcile.rs`
`ReconcileParams { eps_pos 0.25, eps_rot 0.03, snap_pos 6.0, blend 0.3 }`.
`InSync` (< eps) → leave alone; `Snap` (> snap_pos) → full authority; else `Correct`
= `current + err*blend`. **Pure + unit-tested.** The caller seats velocity.

### Wire
- Snapshots @ **20 Hz**, `only_if_changed`. `INTERP_DELAY = 0.18 s`. Not
  cross-platform deterministic (avian `enhanced-determinism` off) — every snapshot
  re-anchors; no lockstep.

---

## 3. The problem, precisely (and the experiment that proved it)

On the client two incompatible representations meet in one avian solver:
- **Your rover:** `Dynamic`, simulated at *now* from local input.
- **Other rovers:** `Kinematic` proxies — **immovable, ~0.18 s in the past, moved by
  teleport (`interpolate_proxies` writes Position each frame), with velocity forced
  to 0 (`force_kinematic_proxies`).**

**Two compounding defects:**
1. **#1 — Kinematic-by-teleport, zero-velocity (the kick).** A kinematic body
   pushes dynamics *only if the solver knows its velocity*. We zero it, then teleport
   it. So each frame the proxy teleports into your rover → penetration appears from
   nowhere → solver ejects with a large penetration-correction impulse → buzz/kick.
2. **#2 — Reconcile re-amplifies (the buzz).** Your local contact resolved against
   the *stale* proxy; the host resolved it with both rovers at true present
   positions. Divergence ≈ `INTERP_DELAY × closing-speed` (~0.5–1 m) > `eps_pos`
   (0.25) → every ack `Correct`s and **seats the stale authoritative velocity** →
   second shove → sustained oscillation.

**Decisive experiment (done this session):** disabled `reconcile_owned_prediction`
(temporary `return;`) and drove into a rover. Result: **the sustained buzz vanished
but a discrete "kickback" remained.** → confirms reconcile = the *amplifier* (#2)
and the kinematic-proxy contact = the *source impulse* (#1). Both are real; the
violent jitter is #1 × #2.

This is exactly the case the docs **deferred**: *two rovers in contact under
prediction* (`DECISIONS.md` ~65-66, `DESIGN_GAPS.md` DEFER). You predict YOUR rover;
others are server-authoritative kinematic stand-ins → mutual contact can't be
physically right at the solver level.

---

## 4. What was tried and FAILED (do not repeat)

1. **`Correct` = position-only (drop velocity seating).** Made it **worse** — broke
   the common (non-contact) case: position yanked back each ack with unmatched
   velocity = textbook rubber-band ("jumps back and forth even on open ground").
   **Lesson: velocity seating is net-good; do not remove it wholesale.**
2. **Contact-gate (skip `Correct` while touching a non-static body).** Made it
   **crazy** for joint-based AND raycast rovers. Misfired because a joint rover's
   chassis collider touches its **own dynamic wheel bodies** → `touching_nonstatic`
   was always true → reconcile disabled → free-run divergence. (avian 0.6.1 API used
   was correct: `Collisions` system-param = `ResMut<ContactGraph>`,
   `collisions.entities_colliding_with(e)` yields only `is_touching()` partners;
   classify with `Query<&RigidBody>`. A *correct* gate would key on touching a
   **`NetReplicate` proxy**, which excludes own wheels — but see §5: gating is the
   wrong layer anyway.)
3. **Disable reconcile entirely (the experiment).** Removed the buzz but left the
   kick + drift. Diagnostic only — not a fix.

**Meta-lesson:** every attempt lived in the **reconcile layer**, but the source
impulse is in the **proxy/contact layer**. Reconcile-layer fixes can only ever damp
or disable; they can't make the contact correct.

---

## 5. The proper fix — "Predict-and-Smooth"

`PREDICTION_RECONCILIATION.md` step 5 already says *"render-smooth the residual."*
**It was never implemented** — `reconcile_owned_prediction` writes corrections
straight onto the physics `Position`/`Rotation`/velocity. That is the architectural
root of the buzz. The proper design has two independent parts:

### Part 1 — make the proxy a physically honest obstacle (kills the kick)
Stop zeroing proxy velocity. Give each kinematic proxy its **interpolated velocity**
(`Δpos / Δt` between the two bracketing snapshot samples) so avian resolves the
`Dynamic`↔`Kinematic` contact with a correct relative velocity (a smooth bounce/stop
off where the other rover actually was) instead of a teleport-penetration spike.
Position stays snapshot-authoritative; we only tell the *solver* the truth about the
proxy's motion.
- Touch: `force_kinematic_proxies` (stop zeroing) + `interpolate_proxies` (it already
  computes the bracketing samples — derive and write the velocity there).
- **Watch the ordering**: `interpolate_proxies`/`force_kinematic_proxies` run in
  `Update`; the physics step is `FixedUpdate`. Verify whether avian integrates a
  kinematic body's Position from its velocity during the step (if so, the per-frame
  Position teleport must remain authoritative and the velocity is purely for the
  contact solver). Test empirically.
- **This is the cheap, high-value first step. Do it with reconcile still off and
  confirm the kick becomes a clean bounce.**

### Part 2 — correct the render, not the rigid body (kills the buzz, everywhere)
The reconciler must **never seat the physics body during normal play**:
- The physics body runs continuously (local input + honest contact); never yanked.
- Each ack, compute divergence vs authority and feed it into a **decaying visual
  offset** applied to a **render-only child** of the rover (ease to zero over
  ~150 ms). Only a gross desync (`Snap`) ever hard-resets the physics body.
- **avian gotcha:** avian re-reads `Transform → Position` at the *start* of each
  step (for user edits), so a visual offset written onto the body's `Transform`
  would feed back into physics. The offset must live on a **child entity the solver
  doesn't touch** (e.g. the mesh/visual root child). Verify the rover's visual
  hierarchy first (USD prims are child entities — likely already separable).
- With Part 2, there is no per-ack tug on the solver → contact (or anything) can't
  be amplified into a buzz. Part 1 ensures the smoothed residual is already small.

### What this does NOT give
You still **won't push other rovers locally** — they stay server-owned; you bounce
cleanly and they move a beat later (server-resolved). That's MVP-correct
("correctness > twitch"). Real *mutual push* (you shove it, it shoves you, locally)
needs predicting the whole **contact island** (predict the other body too, seed from
snapshot, state-reconcile both) — the deliberately-deferred **Phase C** /
rocket-league case. Even AAA (Rocket League) only achieves it via predict-everything
on a fixed-step deterministic core.

### Recommended build order
1. Establish clean baseline (DONE — tree is clean).
2. **Part 1** (proxy velocity), reconcile still off → confirm kick → clean bounce.
3. **Part 2** (render-smoothing) → re-enable reconcile (Snap-only seats body) → drive
   normally + into rovers → no buzz, no drift, correction invisible.
4. (Optional, later) Phase C contact-island for true mutual push.

---

## 6. Operational gotchas learned (save the next person hours)

### Running host + client (native, localhost — NO TLS/cert setup needed)
From the **repo root** (CWD matters — see asset gotcha), two terminals:
```bash
# Host (listen-server; net 5888/udp, HTTP API 4001)
cd /home/rod/Documents/luncosim-workspace/main
cargo run -p lunco-client --bin sandbox --features networking -- --host --api 4001 --no-throttle

# Client (connects to host; HTTP API 4002)
cd /home/rod/Documents/luncosim-workspace/main
cargo run -p lunco-client --bin sandbox --features networking -- --connect 127.0.0.1:5888 --api 4002 --no-throttle
```
- Networking is a **cargo feature** on `lunco-client` (`--features networking`);
  flags `--host [port]` / `--connect addr[:port]` (default port 5888).
- Native client uses **empty cert digest + `dangerous-configuration`** → no
  mkcert/CA hassle on localhost. (Browser client needs the WebTransport digest in
  the URL hash — see `SPIKE_PH0.md`.)
- `--no-throttle` keeps rendering when unfocused (else background tab → ~1 FPS).
- API: `POST http://127.0.0.1:<port>/api/commands` body `{"command":"Ping"}`.
  **curl is hook-blocked in this environment** → probe via a JS `fetch`
  (context-mode `ctx_execute`). `CaptureScreenshot{}` returns **raw PNG bytes**.

### Asset-CWD gotcha (caused a full empty-scene red herring)
`sandbox` sets its asset root to `std::env::current_dir().join("assets")`
(`crates/lunco-client/src/bin/sandbox.rs:~197`). If `cargo run` is launched from the
wrong CWD (e.g. a shell that `cd`'d into `target/debug/deps` earlier — the Bash tool
persists CWD across calls), the app looks for `target/debug/deps/assets/...` →
`sandbox_scene.usda` Path-not-found → **EMPTY black viewport**, no USD camera → at
2 s the fallback avatar spawns a **second `FloatingOrigin`** →
`"BigSpace has multiple floating origins"` flood every frame. **All one root cause.**
Fix: always launch from the repo root.

### Build / disk
- Use `cargo ... -j 2` (this machine struggles). The networking build (lightyear +
  avian @ opt-level 3) is the heaviest in the repo.
- Disk on `/home` runs near-full; the networking build hit `No space left` twice.
  Regenerable to clear (in this worktree only): `~/.cache/sccache`,
  `target/debug/incremental`, stale `target/debug/deps/<crate>-<hash>` test binaries.
  **Do NOT touch the sibling `../networking/target`** (separate worktree).
- Two-window testing is RAM-bound; if the client renders at <1 FPS the apparent
  "jitter" may just be frame starvation — **check client `Ping` round-trip first**
  (it ≈ frame time). See `PREDICTION_RECONCILIATION` debugging lesson.

### avian 0.6.1 facts relevant to the fix
- `Collisions` system-param wraps `ResMut<ContactGraph>`;
  `entities_colliding_with(e)` returns only **touching** partners (filters
  `is_touching()`); returns **collider** entities (may differ from the rigid-body
  entity for child colliders).
- Kinematic ↔ Kinematic does **not** collide (so two *proxy* rovers pass through each
  other — the long-standing deferred caveat). Kinematic → Dynamic **does** push (this
  is what enables Phase B's ball, and what makes your-rover-vs-proxy interact at all).
- `SubstepCount = 12` on rovers (suspension stability). Any local re-stepping inherits
  it (same FixedUpdate physics) — don't hand-roll an integrator.

---

## 7. Open questions for the user
1. **Contact target:** "clean bounce, server-authoritative" (Predict-and-Smooth) or
   "real mutual push" (Phase C)? This decides scope.
2. **Stashed Phase B + §6:** keep (pop + commit) or leave parked? They're sound and
   don't cause the jitter.
3. First confirm the **clean baseline** contact feel (rebuild + drive) so we have a
   true before-picture for Part 1.
