# Startup failure ownership and Griffin cache audit

`lunco-luncosim-services` scopes the explicit CLI startup failure guard to its
canonical Twin root and complete root-relative scene path. The workspace's
`TwinClosed` observer retires that guard before late asset results are admitted.
Another mount or a different directory containing the same basename cannot
acquire fatal startup ownership. Active startup failures retain a nonzero exit.

The observed failure occurred on PID 2910268/API 5317: a valid `OpenTwin` replaced
the bootstrap Twin while its native USD document was preparing. The retired
document correctly rejected its changed input authority. The application guard
still matched its basename, requested process exit, and left the controller
receiving HTTP 500. This is a failure-lease bug, not a physics or cache failure.

The existing workspace/asset owners supply the close edge and mounted-root
identity. No dependency, public API, cache budget or numerical setting changes.
Hook review: this repair enforces the lifetime of an existing application
failure decision; it adds no authored selection or lifecycle policy. Rust owns
the lease invariant and its generic close application.

## Verification

The focused lifecycle seam passes:

`cargo test --locked -p lunco-luncosim-services --lib startup_failure_lease_matches_the_exact_live_owner -j 4 -- --nocapture`

It admits native asset messages that the public API cannot inject. Foreign
mounts, different relative paths and failures after closure leave the process
running; closing an unrelated Twin preserves the guard. The active owner's
own failure issues `AppExit::error` and removes the guard.

The regular production build passes in 1m32s with sccache, `target/` and four
jobs. SHA-256:
`fbd78233deebe3dcfa8905319aad448450034054074a3235c7fd2e30696b08f0`.

The production replacement gate on API 5319 passes four authored Rhai checks:
current workspace ownership, clear readiness, clear runtime diagnostics and
projected replacement root. It observes a startup load in flight, replaces it,
inspects the screenshot, and closes through the owned API. That bounded run
does not reproduce the late native authority rejection; the deterministic
message-admission test covers that race, and the original failure log remains
its production reproduction. The replacement fixture intentionally has an
empty viewport. An invalid startup root on API 5320 separately exits 1 with
its owner diagnostic and no panic. Both ports are released.

The first attempt at the replacement gate misread the diagnostics count as an
array. It is excluded; the Rhai assertion uses the authoritative numeric count.
The skill catalogue check passes. No broad physics suite is repeated because
this repair changes only startup failure ownership.

## Griffin observations

The preceding regular run on API 5316 reaches sleeping four-pad handoff with
finite mass/inertia and clear diagnostics. First requested-scene physics
readiness is 21.569 s. Its initializer's prepared solver is a disk hit in
735.446 ms, but native construction takes 8.280 s: 16,482 attempts, 7,896 hits
and 8,586 compilations. Its ten-second settled window measures 3.333/4.116 ms
coupled and 9.019/11.409 ms frames, median/p95.

The source-matched repeat on API 5321 has the same initializer source key,
solver and Rumoca identity. Prepared lookup takes 731.692 ms. Construction
takes 3.162 s with 16,414 hits and 68 compilations in 16,482 attempts. The
different native-cache condition explains a large component of construction
cost; this is not a speedup attributed to the startup guard repair.

The repeat's launch controller exits 143 while the app remains live.
Its total loading timing is excluded. A recovery controller verifies the exact
PID 2977501, checkout, executable hash and API 5321, then collects a fresh
five-second readiness soak and ten-second window with at least 240 fresh ticks.
Griffin is sleeping after four-pad handoff, diagnostics are clear and all 271
source hashes match the preceding run. The inspected image retains the lander,
terrain and shadow. Coupled median/p95 is 3.325/3.973 ms; frames are
9.233/12.233 ms. The app exits through its API and releases the port.

Recorded workloads show no other live simulator or Cargo build during these
normal windows. Host costs still vary, and these are bounded landing windows,
not complete mission acceptance. All original loading, coupled-tick and
graphics targets remain open. The current cache keys match the earlier warm
cohort; increasing retention or changing codecs is not supported as a fix.
Recurring native constructor work remains a measured startup lead.

Raw logs, receipts and images remain ignored under
`.cache/codex-evidence/performance-20261023/griffin-startup-cache-*` and
`startup-lease-*`. No profiling data is committed.
