# Scoped scalar transaction storage

`lunco-port-core::InputWriteBuffer` owns reusable staging for the exclusive
`PortRegistry::input_transaction` boundary. Its duplicate index uses Bevy's
`HashTable<usize>` to refer to the already-owned staged writes. The table hashes
and compares the complete entity/name identity, including collision checks,
without cloning each prepared name into another collection. Commit order stays
the staging-vector order. The index clears before its referenced writes.

This is an internal storage change using an existing Bevy primitive. No API,
dependency, cache owner, policy or hook is added. Live declaration precedence,
metadata, bounds, permission, topology and slot checks remain in the registry.
Independent retained batches still revalidate before committing. No physics
setting, precision or graphical quality changes.

The preceding landed native Tracy write zone contains 234 selected 1 kHz CPU
samples, including reference-count operations and collection/string work.
Some native symbols are unavailable, so those leaf samples alone do not identify
which shared value caused each operation. Source inspection identifies the
duplicate index's additional name ownership; the isolated measurement and
bounded native comparison qualify this specific change.

The ignored low-level diagnostic exercises 512 transactions of 512 resolved
inputs, reusing the buffer and checking the committed samples. Its elapsed time
is 34.264 ms before and 32.359 ms after, about 5.6% less. This is one bounded
measurement, not a whole-scene speed claim. The existing named owner test
`resolved_input_write_preflights_once_and_rejects_invalid_or_stale_contracts`
passes, covering atomic rejection, duplicates, read-only/invalid/stale writes
and buffer reuse. Production `port_owner_collision` passes all twelve checks.

The normal `cargo build -p lunco-luncosim -j 4` completes in 3m17s with sccache.
Its SHA-256 is
`742395ef6d209e786beee6088e7c92ba45d472ce7417825a58c063e75c82de78`.
The existing unrelated `UsdStageProjectionPlan` unused-import warning remains.
Three regular desktop observations require five seconds of stable readiness
and complete terrain admission, followed by ten measured seconds:

| Twin | OpenTwin to ready (s) | Complete tick median/p95 (ms) | Frame median/p95 (ms) |
|---|---:|---:|---:|
| Summer | 3.285 | 3.596 / 3.985 | 5.100 / 9.413 |
| Griffin | 24.139 | 5.107 / 7.547 | 12.675 / 15.487 |
| Tesla | 2.529 | 1.510 / 1.813 | 3.450 / 5.697 |

Each measured cohort retains unchanged authored hashes and clear runtime
diagnostics/script-error checks. Final images are inspected. Summer and Griffin
match the preceding source cohorts; Tesla's `models/SuctionFan.mo` changed
between cohorts, so its before/after timing is not attributed to this patch.
Another worktree's Tesla process on port 4387 remains active. These are bounded,
contention-affected observations rather than complete mission or clean FPS
acceptance. The existing unavailable frame/datum Tesla HUD remains unqualified.

The separate normal Tracy build completes in 2m06s, SHA-256
`7a78556ea0a26c241aa5b0b346f7367c346867fc22cc22a9d1d44af1f2dc8b3c`.
The collector starts before the owned application. Native 0.14.1 export verifies
PID 1165171, 1 kHz sampling, 17,315 compressed blocks, 350,961,342 payload bytes
and 351,030,608 total bytes. The last 240 complete FixedMain instances cover
57,049,863,627 through 60,823,415,078 ns after landing handoff and readiness soak.
Authored hashes match `griffin-celestial-native-landed`; both retain 638 bodies,
20 dynamic bodies, 21 sleeping bodies, 777 colliders, 31 joints, 315 graph pairs
and no orphan pairs. Both final images and diagnostics are checked.

| Inclusive instrumented owner median | Before | After |
|---|---:|---:|
| Port-write transaction | 0.932 ms | 0.860 ms |
| Complete connection propagation | 1.067 ms | 0.991 ms |
| Complete coupled tick, API | 5.208 ms | 4.990 ms |

The write-zone median is about 7.7% lower. Other unchanged owners also vary,
so the complete-tick difference is not attributed solely to this storage change.
Instrumented timings do not establish regular-build acceptance. All owned apps,
collectors and readers exit successfully; API ports 5143–5147 and Tracy port
8419 are released. Data stays ignored under
`.cache/codex-evidence/performance-20261023/port-transaction-table-*`,
`*-port-transaction-table-normal/`, `griffin-port-transaction-table-landed/`
and `scripts/perf/captures/`.

The sub-second loading, sub-millisecond whole coupled tick and sub-10 ms frame
targets remain open. Prepared-solver disk replay and live construction remain
loading leads; native frame/solver and port-admission work remain tick leads.
