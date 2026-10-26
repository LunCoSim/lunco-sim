# Input transaction measurement and Griffin qualification

Status: diagnostic benchmark retained; runtime remains unchanged.

The shared port owner now has an ignored `multi_port_transaction_diagnostic`.
It measures 2,048 complete transactions on 32 entities at widths 1, 8 and 32.
Five earlier backends query a registered component absent on these entities;
the fixture asserts that backend five wins. Each transaction uses the normal
live validation, duplicate checks and atomic commit. Every final scalar must
have exactly the expected f64 bits. This complements the existing single-owner
transaction diagnostic without changing runtime behavior or physics policy.

Run with the normal development profile, repository target and sccache:

```sh
cargo test --locked -p lunco-port-core -j 4 --lib multi_port_transaction_diagnostic -- --ignored --nocapture
```

## Decision evidence

An entity-grouped live-discovery experiment passed its low-level owner-change,
metadata, unsampled-input and atomic-rejection checks. It was removed after
production qualification failed to demonstrate a benefit. There is no retained
alternate API, grouped write implementation or dependency patch.

Twenty independent processes in four ABBA blocks compare the same full fixture.
Snapshot binaries have distinct inodes and SHA-256 values, verified unchanged
after the comparison. All final-value assertions pass. Results for the final
experiment, before removal:

| Inputs per entity | Baseline median ms | Experiment median ms | Improvement |
| --- | ---: | ---: | ---: |
| 1 | 10.758 | 11.531 | -7.2% |
| 8 | 86.054 | 76.684 | 10.9% |
| 32 | 343.848 | 299.708 | 12.8% |

These are complete fixture durations, not per-tick or scene gains. They do not
model every native backend's declaration cost or the scene's input distribution.

## Regular production qualification

Both builds use `cargo build --locked -p lunco-luncosim -j 4`, without Tracy.
The unchanged baseline is application `5096c6b51`; both cohorts use model
`2b3c0a926b656162be80fa60053976b7f3ff2312`. The existing
`scripts/perf/twin_startup.py` owns each process and records source hashes,
readiness, terrain, handoff, diagnostics, body state and frame/tick windows.

| Measurement | Baseline PID 829156 / API 5469 | Experiment PID 845433 / API 5470 |
| --- | ---: | ---: |
| Physics readiness, seconds | 8.634 | 8.769 |
| Coupled tick p50 / p95, ms | 3.404 / 4.276 | 3.827 / 4.521 |
| Application frame p50 / p95, ms | 9.198 / 13.160 | 11.133 / 14.044 |
| Initializer prepared-cache lookup, seconds | 0.717 | 0.738 |
| Initializer live construction, seconds | 2.373 | 2.465 |

Both landers reach four-pad handoff and sleep without an object pause. Each
cohort preserves all 271 source hashes and ends with zero diagnostic warnings
or errors. The inspected screenshots show the landed vehicle, lunar terrain,
shadow and handoff controls. Both processes and ports close. This establishes
landing-handoff behavior, not the subsequent ramp/rover mission or exact replay.
Desktop/browser workloads remain present; neither run overlaps a Cargo build
or another simulator. This one pair does not isolate the cause of the slower
experiment, but supplies no evidence for keeping its added complexity.

The initializer records 16,482 native cache hits and zero compilations in both
runs. Its 2.373-second baseline construction alone exceeds the loading target.
The cache lookup and live construction are separately measured native-owner
intervals; the remaining readiness elapsed time is not an attributed subsystem
cost. The pending [Rumoca producer index](rumoca-assignment-producer-index-2026-10-26.md)
and [OpenUSD parent-path proposal](usd-parent-path-preparation-2026-10-26.md)
remain distinct dependency changes requiring approval. No further native scope
is modified by this investigation.

Raw binaries, timings, screenshots, receipts and the removed experiment patch
remain ignored under `.cache/codex-evidence/performance-20261026/input-owner-batching/`.
Only the reusable diagnostic and this interpretation are tracked. The loading,
coupled-tick and graphics targets, and previously reported articulated replay
failures, remain open.

## Retained change validation

The named diagnostic passes all three widths through the command above.
The skills validator passes all 43 entries. The regular production rebuild
after removing the experiment completes without warnings; SHA-256 is
`eed8b9cff36e1bc1021b18b73ad677f1fe2cd89756d98fcbaea828ddff6535bd`.
Only ignored test code and documentation differ from the baseline runtime.
The existing production behavior evidence remains applicable; the removed
experiment does not require retaining its expanded API or extra scene tests.
