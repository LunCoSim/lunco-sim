# Collider frame preparation and performance qualification

`lunco-usd-avian-core` owns relative collider transforms below the canonical
BigSpace root. Native Avian propagation requires a root `Transform`, which the
BigSpace root does not have. The existing preparation consumer composes the
relative transform before native scale and mass-property consumers.

Removing `ChildOf` must invalidate that composition even when the local
`Transform` is unchanged. The consumer now independently drains transform,
rigid-body and parent removal streams and recomposes once on any removal.
The regression fails before the fix: removing a scale-two parent leaves the
child's relative translation at two rather than one. The fixed owner passes all
five `collider_` tests, including invalid hierarchy/rotation admission and native
detach/retirement seams. There is no new cache, API, policy hook or dependency
patch. The regular development profile optimizes this hot owning package at
level three while retaining development checks, precision and physics settings.

## Production evidence

`cargo test --locked -p lunco-usd-avian-core --lib collider_ -j 4 -- --nocapture`
passes five tests. The normal `cargo build --locked -p lunco-luncosim -j 4`
passes in 1m35s, SHA-256
`38349d5ded4cc194cf82caa081df4d1fdeac11627cebfdc7076b5d0645b2cf2d`.
Production initial-velocity, contact-impact-pause and Griffin nominal-contact
gates pass five, ten and five checks respectively. The nominal gate reaches
touchdown and flight handoff, then passes at tick 1369 without a false pause.
The existing unrelated `UsdStageProjectionPlan` import warning remains.

The three normal desktop observations use complete terrain admission, a
five-second readiness soak and ten measured seconds. Each complete-tick result
contains 240 samples; unchanged source hashes, clear diagnostics, script-error
checks and inspected final images are retained.

| Twin | Qualified milestone (s) | Complete tick median/p95 (ms) | Frame median/p95 (ms) |
|---|---:|---:|---:|
| Summer | 3.371 | 3.543 / 4.121 | 5.616 / 10.419 |
| Griffin | 39.571 after landing | 3.952 / 4.712 | 11.103 / 15.425 |
| Tesla | 3.339 | 2.751 / 4.901 | 9.114 / 14.448 |

Griffin's qualified milestone includes the mission's descent; it is not loading
latency. Its first post-load ready observation is 17.305 s. The Summer milestone
also includes complete terrain preparation. Another Tesla application, PID
1559015/API 4497, is active during this cohort. These are contention-affected
observations, not clean acceptance or a causal whole-scene speed comparison.
Tesla's frame/datum-unavailable HUD remains unqualified.

## Native attribution

The separate Tracy build completes in 1m29s, SHA-256
`6e4e105bdb6460b2bddbaadf5d380cb2f31e58064a4538d7f650ec6e9ef8d291`.
Collector PID 1613925 starts before owned application PID 1613958/API 5171,
using the matching adjacent Tracy 0.14.1 tools and 1 kHz sampling on port 8421.
Native export succeeds. File framing verifies 16,830 blocks, 345,604,655
compressed payload bytes and 345,671,981 total bytes, plus the exact app PID and
sampling period. The last 240 complete FixedMain instances cover
54,900,605,511 through 60,839,873,030 ns, after landing and the readiness soak.

Griffin authored hashes match `griffin-proof-hasher-landed`. Both retain 638
bodies, 20 dynamic bodies, 21 sleeping bodies, 777 colliders, 31 joints, 315
contact-graph pairs and no orphan pairs. Current inclusive medians are 0.125 ms
for each fixed pose-to-position pass, 0.034 ms for position-to-pose, and 0.253 ms
for rootless collider propagation. The preceding capture reports 0.232, 0.055
and 0.349 ms respectively. Unchanged port writes and native physics also vary
substantially between captures, so these measurements do not isolate the
compiler-profile contribution. The correctness fix is independently reproduced.

Port writes remain a 1.108 ms inclusive instrumented median, and complete
connection propagation is 1.326 ms. Saved native CPU selection covers 240 write
zones and 352 samples, with no missing stacks, but almost all symbols are
unresolved; these stacks do not establish a specific function-level bottleneck.
The generic binding inventory records 1,412 bound wires with unique targets;
1,197 feed 57 exhaust visual entities. This makes actual shader-port admission
and write consumers a next measurement target. No wire is skipped or moved to
another clock on the strength of that inventory.

The native GPU instance-builder experiment remains rejected: its faster cohort
has rectangular terrain holes that persist with shadow maps disabled. The
validated CPU instance builder remains active. A scan of 600 cached terrain
meshes finds no morph targets outside their position bounds; it does not justify
adding guessed bounds expansion. Neither experiment establishes a rendering fix.

All owned apps and collectors close through their normal boundary. Profiling
data stays ignored under `.cache/codex-evidence/performance-20261023/` and
`scripts/perf/captures/`; derived unwrapped CSV is removed after extracting the
native summaries and samples. All three original performance targets remain
open.
