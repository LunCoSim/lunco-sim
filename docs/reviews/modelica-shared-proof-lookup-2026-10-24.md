# Modelica shared-value proof lookup

The authoritative owner is Rumoca's `rumoca-ir-solve::linear_op::shared_values`,
consumed by native and interpreted schedule construction. The application pins
the published LunCoSim fork revision
`0025c074a8fed2769681d88ae4b4711144d82740`. Private proof tables use the existing
workspace `rustc-hash` implementation. Full structural equality resolves hash
collisions; complete typed operation encoding preserves floating-point bits.
Program traversal assigns IDs and execution order. The independent symbolic
equivalence check, effects, register limits and final-slot checks remain intact.
There is no new public API, cache, policy, solver setting or precision change.
All non-Rumoca lockfile package records remain unchanged.

The preceding native Griffin capture selects 3,463 CPU samples inside the
4.369 s coupled-initializer constructor. Available frames include shared-value
interning and standard SipHash; some symbols are unavailable. This motivates
the lookup change, rather than proving its production benefit.

## Owner qualification

The adjacent `../rumoca` checkout passes:

- `cargo test -p rumoca-ir-solve --lib shared_value -j4 -- --nocapture`:
  eighteen tests, including changed operands/output selection, effects, cap
  boundaries, signed-zero and NaN-payload rejection.
- `cargo xtask verify msl-parity --sim-targets-file infra/verification/msl-canary-20.json`:
  the same twenty phase/error and channel-band rows as the preceding canary.
  MSL 4.1.0 and OpenModelica 1.27.1~2-g6db4671 retain twenty parse/flatten,
  fourteen DAE/solve and twelve initialized/simulated high-band traces. Six DAE
  and two solve refusals remain visible; no timeouts occur.
- `cargo clippy -p rumoca-ir-solve --all-targets --all-features --no-deps -j4 -- -D warnings`.

Builds use four jobs; canary and Clippy invocations set four test threads.
No full workspace, coverage or 566-model
cohort qualification is claimed. Separate development diagnostics measure scalar
proof derivation/checking at 5.521 ms before and 3.782 ms after, and wide proofs
at 4.246 ms before and 1.721 ms after. These are isolated observations.

## Production checks

The normal `cargo build --locked -p lunco-luncosim -j 4` completes in 5m17s after
the checkout's build directory disappeared. It uses regular sccache/profile
settings. Binary SHA-256:
`e51f3185c20ec3d5bb4a9818f8d272b3337d96c3a8d61b46dff282e728e529f8`.
The existing `UsdStageProjectionPlan` unused-import warning remains.

Production authored gates pass: initial velocity (5), coupled impact pause (10),
Griffin excessive impact (7), nominal Griffin touchdown (5), and collision
enablement (8). The impact case still pauses twenty connected bodies and their
coupled models with a warning; nominal contact keeps the system active.

The reported packaging fixture, SHA-256
`6022e609c35114f7523a9665f88efa434cab669444c7e7d3294d85d762462c71`,
admits its exact root as static and advances from tick 1 to 185 headlessly with
zero runtime diagnostics. The reported island panic does not reproduce. This
is a bounded runtime observation, not the fixture's visual/mission verdict.
The probe waits for the exact body's admission, then reads the typed
`SimulationTimingProfile`; bootstrap readiness alone does not qualify it.
An earlier prematurely closed window reports GPU device loss and is excluded
from graphics evidence. Failed probe requests are retained separately.

The three normal desktop observations require complete terrain admission,
five seconds of stable readiness and ten measured seconds:

| Twin | OpenTwin to ready (s) | Complete tick median/p95 (ms) | Frame median/p95 (ms) |
|---|---:|---:|---:|
| Summer | 4.354 | 3.626 / 4.250 | 5.634 / 10.632 |
| Griffin | 37.351 | 7.826 / 9.554 | 18.765 / 31.066 |
| Tesla | 2.438 | 1.565 / 1.959 | 3.590 / 6.078 |

Authored hashes match the preceding transaction-storage cohorts and remain
unchanged during measurement. Runtime diagnostics and script-error checks are
clear; final images are inspected. The first sequence stops after Griffin;
Tesla completes separately. Completed sessions exit through their API.
These first runs with the new compiler identity include prepared-cache misses,
so their loading times are not a matched warm-cache comparison. Native-cache
fingerprints, prepared-record manifests and replay logs are retained. Another
worktree's Tesla is active during these regular cohorts. Tesla's unavailable frame/datum HUD remains
outside qualification. These observations do not establish a production speedup.

## Native constructor attribution

The regular Tracy build completes in 3m14s, SHA-256
`111736eff0acde532014490d455e132367f6cf89c2415ec63bf052fe9a8a041d`.
Collector PID 1435279 starts before application PID 1435318/API 5157, using
Tracy port 8420 and 1 kHz CPU sampling. Native 0.14.1 export validates the saved
capture: 17,070 blocks, 352,571,979 compressed payload bytes and 352,640,265
total bytes. Authored hashes match the preceding landed capture. Both retain
638 bodies, twenty dynamic bodies, 21 sleeping bodies, 777 colliders, 31 joints,
315 contact-graph pairs and zero orphan pairs. These are graph pairs, not a
count of solved contacts. The final image and runtime diagnostics are checked.

The last 240 complete FixedMain instances cover 65,559,162,988 through
72,031,856,141 ns. Native FixedMain median is 8.888 ms; the API's retained window
reports 8.680 ms. Port-write median is 1.615 ms and connection propagation
1.904 ms. These instrumented timings do not establish regular-build acceptance.

The constructor comparison has different native-cache conditions:

| Coupled initializer | Preceding capture | Current capture |
|---|---:|---:|
| Constructor wall time | 4.369 s | 8.620 s |
| Native artifact attempts | 16,482 | 16,482 |
| Hits | 16,414 | 5,501 |
| Compilations / evictions | 68 / 68 | 10,981 / 10,981 |

The current key-level trace reconstructs every lookup, insertion and recorded
entry/byte count. It loads 32,768 entries before the first lookup, then observes
28,314 distinct keys occupying 38,250,071 bytes. That working set fits the existing
32,768-entry/64 MiB limits. The coupled initializer records 16,414 hits in 16,482
attempts, 68 cold keys and no duplicate pending compilation. Its constructor is
4.356 s. Increasing cache limits is not justified by this trace; the cause of the
earlier capture's different startup cache contents remains unproven.
The trace's graphics window loses terrain qualification and is excluded from
performance acceptance. Its completed constructor and exact cache replay remain
valid diagnostic evidence under `griffin-native-cache-current-keys/`.

The matching-version saved-sample reader selects 7,288 constructor CPU samples,
versus 3,463 previously, with no missing stacks. Identified SipHash leaf samples
fall from 224 to 26. Stacks identifying shared-value proof code contain 251
and 250 samples respectively, while identified Cranelift frames increase.
These sampled categories are incomplete where symbols are unavailable. The
observed hash-cost reduction does not establish a faster complete constructor
under the changed compilation workload.

All owned applications, collectors and readers exit; their ports are released.
The ordinary binary is restored and its SHA-256 exactly matches the production
gates above. Three regenerable zone exports totaling over 9 GB are removed;
their native captures, summaries and constructor samples remain ignored.

The sub-second loading, sub-millisecond complete tick and sub-10 ms frame
targets remain open, as does the unconfirmed lunar-base-model mid-flight reload.
Raw receipts remain ignored under
`.cache/codex-evidence/performance-20261023/` and `scripts/perf/captures/`.
