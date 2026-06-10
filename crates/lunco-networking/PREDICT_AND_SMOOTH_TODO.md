# Predict-and-Smooth — step-by-step TODO checklist

Companion to [`PREDICT_AND_SMOOTH_PLAN.md`](./PREDICT_AND_SMOOTH_PLAN.md) (design +
rationale live THERE; this is the execution order). Tick items as they land.
File:line refs are against the 2026-06-11 working tree (post stash-pop).

Run protocol for every verify item: host + client from **repo root**
(`cargo run -p lunco-client --bin sandbox --features networking -- --host --api 4001 --no-throttle`
/ `-- --connect 127.0.0.1:5888 --api 4002 --no-throttle`), probe via ctx_execute
fetch, build with `-j 2`. See `CONTACT_JITTER_HANDOVER.md` §6 for gotchas (CWD,
disk, Ping≈frametime check).

---

## Step 0 — baseline (DONE 2026-06-11)

- [x] `git stash pop` stash@{0} (Phase B `PredictedDynamic` + §6 `NotPredictable`).
- [x] Strip diagnostic `return;` from `reconcile_owned_prediction` (commands.rs).
- [ ] `cargo check -p lunco-sandbox-edit -j 2` + `cargo test -p lunco-core -j 2`
      green (deferred — fold into Step 1's first build, per no-build-per-edit rule).
- [ ] User: commit baseline + docs (user commits themselves).

## Step 1 — velocity-driven kinematic proxies

**Probe first (throwaway, ~30 min):**
- [x] 1.1 **DONE 2026-06-11** — avian 0.6.1 kinematic semantics CONFIRMED via
      headless real-solver test mod `avian_kinematic_probe` (commands.rs):
      `Position += v·h` per tick (±8 nm substep rounding) ✓; kinematic velocity
      pushes a Dynamic body from a clear gap ✓. Learned: one-tick spawn/prepare lag;
      harness needs AssetPlugin+init_asset::<Mesh>()+DiagnosticsPlugin+finish/cleanup.
      Outcome logged in PLAN §2 "avian facts". Delete probe mod when Step 1 lands.

**Implement (`crates/lunco-sandbox-edit/src/commands.rs` only) — DONE 2026-06-11:**
- [x] 1.2 `ProxyPlaybackClock { t, init }` resource + pure `advance_playback_clock`
      helper (returns `(render_t, snapped)`); `init_resource` registered.
- [x] 1.3 `sample_curve(buf, t) -> Option<(pos, rot, lv, av)>` — cubic Hermite
      position (reduces to exact linear at constant v, verified), slerp rot, starved
      glide + dist cap preserved. `interpolate_proxies` consumes it.
- [x] 1.4 `drive_kinematic_proxies` (FixedUpdate): advances clock once; per RigidBody
      proxy sets `LinearVelocity=(target−pos)/h` (target = curve at `render_t+h`) +
      `AngularVelocity` via `ang_vel_to_track` (shortest-arc q_err→axis·angle/h);
      teleport when `snapped` OR `|pos−curve(render_t)| > PROXY_SNAP_DIST(2.0)`;
      stamps `ReplicatedChassisMotion`. (First-sample handled by `snapped`+dist, no
      per-gid set needed.)
- [x] 1.5 `force_kinematic_proxies`: kept kinematic pinning, DELETED velocity zeroing
      + updated doc.
- [x] 1.6 `interpolate_proxies`: now read-only on the clock, skips `Has<RigidBody>`
      (handles only RigidBody-less proxies); docs updated.
- [x] 1.7 `app.add_systems(FixedUpdate, drive_kinematic_proxies)` (before avian's
      FixedPostUpdate step; reads buffers filled in prior Update — 1-frame latency
      absorbed by INTERP_DELAY).
- [x] 1.8 7 unit tests `step1_curve_tests` (Hermite endpoints/linear/starved/empty;
      ang-vel identity/90°/shortest-arc). **All green: 31/31 lib tests pass.**
      Fixed 2 pre-existing interpolate tests for the external-clock change.

**Verify (build green; runtime gates need live host+client — user verifies in GUI):**
- [ ] V2 settled remote rover: pixel-still, no blink/glide.
- [ ] V1 ram a parked remote rover: clean bounce, no kick (reconcile ON).
- [ ] V3 balloon/cosim body: follows snapshots, no drift.
- [ ] V4 open-ground owned-rover feel unchanged.

## Step 2 — error-offset render smoothing

**Probe first:**
- [ ] 2.1 Determine avian Transform→Position sync behavior: write an offset onto a
      dynamic body's `Transform` in `PostUpdate`, observe whether physics `Position`
      absorbs it next step. Also check whether avian's transform-interpolation
      feature is enabled anywhere (`grep -rn "TransformInterpolation\|interpolation"
      crates/ --include="*.rs" | grep -i avian`). Outcome decides Option A
      (strip-before-step) vs Option B (render-child). Record decision in the plan doc.

**Implement:**
- [ ] 2.2 `lunco-core/src/smoothing.rs` (always-on substrate): component
      `RenderErrorOffset { pos: Vec3, rot: Quat }` + `decay(dt)` (exp ease,
      time-const 0.05 s) + `MAX_VISUAL_OFFSET` snap-to-identity (3 m / 30°).
      Pure unit tests: decay converges, snap branch, compose order.
- [ ] 2.3 Reconcilers accumulate the pop: in `reconcile_owned_prediction` and
      `reconcile_predicted_dynamic`, after seating the body, add
      `offset.pos += old_rendered_pos − new_pos`,
      `offset.rot = old_rot · new_rot⁻¹ · offset.rot` (insert component if missing).
- [ ] 2.4 Apply path per the 2.1 decision:
      - Option A: `apply_render_offset` (PostUpdate, after avian writeback;
        `Transform = Position ⊕ offset`, then `offset.decay(dt)`) +
        `strip_render_offset` (first in FixedUpdate; `Transform = pure Position`).
      - Option B: locate visual root child per body (USD prim child of chassis);
        apply offset to its local Transform; colliders must not be offset.
- [ ] 2.5 No-snap-into-penetration guard: when decision is `Snap` (or a `Correct`
      moving > ~1 m), spatial-check the target pose against nearby non-static
      bodies (`Collisions`/`SpatialQuery`); on overlap degrade to blend. Lives next
      to the reconcilers; reuse `ReconcileParams`.

**Verify:**
- [ ] V5 force corrections (e.g. brief packet drop / drive hard turns): no visible
      pop; body converges; open-ground feel unchanged.
- [ ] V1 again with reconcile ON: no buzz under sustained contact.

## Step 3 — input hardening

- [ ] 3.1 Map the live input path end-to-end first (read, no edits):
      `lunco-controller/src/lib.rs` `emit_vessel_input` → capture seam →
      `lunco-networking/src/wire.rs` → host apply. Write the actual route into the
      plan doc §4 (it currently says "map during implementation").
- [ ] 3.2 New lightyear channel `InputChannel` (UnorderedUnreliable) in
      `lunco-networking/src/protocol.rs`.
- [ ] 3.3 Client send: per fixed tick, per owned vessel, packet = last ≤10 unacked
      `InputFrame`s (seq, tick, forward, steer, brake). Reuse `OwnedInputLog` ring.
- [ ] 3.4 Host ingest: per (session,gid) de-jitter buffer — dedupe by seq, apply in
      seq order at fixed tick, hold-last on gap (few ticks), drop seq ≤ applied.
      Keep `AppliedInputSeq` semantics identical (acks unchanged).
- [ ] 3.5 Remove per-tick `DriveRover` from the reliable CommandBus path (drive
      input ONLY rides InputChannel now); non-input commands stay reliable.
- [ ] 3.6 `SnapshotSample` += applied input (`in_fwd, in_steer, in_brake`, f32 or
      i8-quantized) stamped in `gather_snapshot`; bump/keep wire compat (host+client
      build together — no cross-version concern).
- [ ] 3.7 Use it immediately in `sample_curve` starvation extrapolation? (optional,
      cheap: steer-along-arc; can defer to Step 4.)
- [ ] net_smoke / lunco-core tests still green.

**Verify:**
- [ ] V6 simulated ~5% loss (tc netem on loopback, or lightyear's loss sim if
      exposed): no correction bursts, input never stalls behind a resend.

## Step 4 — predict-all-vehicles (mutual push)

- [ ] 4.1 Marker `PredictedVehicle` in `lunco-core/src/session.rs` (always-on,
      Reflect), doc cross-ref to plan.
- [ ] 4.2 `maintain_predicted_vehicles` (client-only, Update chain near
      `maintain_owned_locally`): every `RoverVessel` + `NetReplicate`,
      `Without<OwnedLocally>`, `Without<NotPredictable>` → insert marker +
      `RigidBody::Dynamic` + seed `Position/Rotation/LinearVelocity/AngularVelocity`
      from latest snapshot **extrapolated to local now**; on exit remove marker
      (force_kinematic_proxies re-pins). Mirror commands.rs:499-511 lesson.
- [ ] 4.3 Exclude `PredictedVehicle` from `force_kinematic_proxies`,
      `drive_kinematic_proxies`, and `interpolate_proxies` filters.
- [ ] 4.4 Held-input drive: feed snapshot `in_*` (3.6) into the same per-vessel
      input seam the controller/`DriveRover` observer writes, every tick, for
      `PredictedVehicle` bodies (do NOT invent a parallel mobility path).
- [ ] 4.5 Widen `reconcile_predicted_dynamic` query to
      `Or<(With<PredictedDynamic>, With<PredictedVehicle>)>`; corrections flow
      through Step-2 offset automatically.
- [ ] 4.6 Possession-flip handling: gaining `OwnedLocally` demotes `PredictedVehicle`
      (single owner of the body's prediction path — same pattern as the
      PredictedDynamic/OwnedLocally demote in `maintain_predicted_dynamic`).

**Verify:**
- [ ] V7 two clients + host: drive into each other's rovers → mutual push both
      directions, convergence to host truth, no buzz.
- [ ] V8 CPU gate: 3–4 predicted rovers, client holds 60 Hz FixedUpdate
      (`scripts/perf/profile.sh`). **If fail → islands fallback** (plan §5.5):
      proximity-trigger + extrapolate-to-now seeding, reuse 4.1-4.5 machinery.

## Wrap-up

- [ ] Update `PREDICT_AND_SMOOTH_PLAN.md` statuses + probe outcomes (Option A/B,
      avian semantics) as they're learned.
- [ ] Update `DESIGN_GAPS.md` Gap G (closed by Step 3) + Gap C note.
- [ ] Update memory (`project_predict_and_smooth.md`) with shipped state.
- [ ] User commits at each green verify gate (steps are independently shippable).
