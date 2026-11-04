# TODO: network control correctness

Deferred by user request on 2026-11-04. No networking fixes are included in the
third simulation-core repair pass. Reviewed at `d6379130f`; verify current source
before implementation. These are source-traced defects except SC45, which was
reproduced in the production two-peer harness.

- [ ] **SC37 — P1: preserve physical-tick grouping.**
  `lunco-cosim/src/control_frames.rs:281` appends one frame per command;
  `lunco-networking-core/src/prediction.rs:1043,1360` consumes/steps once per
  frame. Define one shared grouping for local execution, host consumption and
  rollback. Acceptance: two partial writes in one tick have the same effective
  timing and trajectory on host/client/replay. Rollback coverage must explicitly
  enable `LUNCO_ROLLBACK=1`.
- [ ] **SC38 — P1: retire queued actions with their endpoint.**
  `lunco-networking-core/src/prediction.rs:1063` faults if a queued target has
  been deleted; `lunco-cosim/src/lib.rs:196` retires holds/cache but leaves
  `BufferedClientInputs`. Acceptance: queue authenticated input, delete the
  exact endpoint before consumption, then verify retirement, no global fault,
  no stale action on a replacement, and continued control of another endpoint.
- [ ] **SC39 — P1: fence old possession packets.**
  `lunco-core-session/src/lib.rs:91` and `session.rs:2013` clear sequence history
  across release/reclaim. A delayed frame from the same producer's previous
  possession can pass current ownership checks. Retain producer high watermarks
  or validate an explicit possession lease. Acceptance: release across a fixed
  boundary, reclaim, deliver an old packet and verify rejection; a different
  producer must still start and receive correct ACKs.
- [ ] **SC44 — P2: publish generic client authority loss.**
  `lunco-networking-sync/src/sync.rs:1541` replaces the ownership table without
  the `ControlAuthorityChanged` lifecycle used by co-simulation cleanup.
  Acceptance: generic `ClaimControl`/`SetPorts` without an avatar `ControlLink`
  must retire local holds after release or takeover while unrelated holds remain.
- [ ] **SC45 — P2: record committed remote controls.**
  `lunco-cosim/src/lib.rs:1410` and the fixed consumer in
  `lunco-networking-core/src/prediction.rs:1043` bypass the session-input
  recorder. A host capture completed with zero records while G2 moved to X=9.05
  and release was acknowledged. Feed successful commits into the existing typed
  ordered recorder. Acceptance: host capture contains remote writes/releases,
  reproduces their effect, and excludes rejected actions. Do not add a second
  recorder. Evidence: local ignored directory
  `.cache/codex-evidence/simulation-core-third-review-20261104/`.
- [ ] **SC46 — P2: share explicit-sequence admission.**
  `lunco-cosim/src/control_frames.rs:245` checks only monotonicity while
  `lunco-core-session/src/session.rs:1972` enforces `MAX_SEQ_JUMP`. After seq1,
  seq1026 commits locally but is rejected by the host. Acceptance: both reject
  before mutation/logging, preserve the next valid sequence and accept the
  boundary value consistently.

Fix grouping and possession/endpoint lifetimes first, then authority-loss and
sequence admission, then capture. Extend the production two-peer gate with
negative cases; a passing smoke test does not cover these triggers. Remove
competing rules in each phase rather than keeping compatibility paths.
