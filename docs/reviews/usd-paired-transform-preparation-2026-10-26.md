# Shared native transform preparation

`lunco-usd-bevy-stage` prepares both immutable snapshot representations from
one native local-transform composition per active prim. The shared reader owns
ordered-stack validation, the native `Matrix4d` and stage convention. Rendering
decomposes the raw matrix and explicitly narrows before convention conversion;
geometry converts the full `f64` matrix. Single-representation live readers use
the same owner. No dependency source, authored scene or numerical policy changes.

The consumer is `UsdStageProjectionPlan::from_stage`, on the existing asset
preparation worker. No cache, lifetime, scheduling policy or hook is introduced.
Absent/blocked/empty stacks retain their absence, and malformed or unrenderable
transforms reject admission. Inactive prims have neither representation.

## Focused verification

`cargo test --locked -p lunco-usd-bevy-stage --lib
prepared_transform_pair_preserves_native_representations -j 4` passes. The
inline native-schema fixture compares both representations exactly for Y/Z up,
metres/centimetres, billion-unit translations, nonuniform scale, rotation,
inverse pivots, reset stacks and three animation samples. It covers empty,
omitted and blocked stacks, inactive prims, malformed order, zero scale and
translation outside the render representation's finite range.

The ignored `native_projection_snapshot_benchmark` contains 1,000 child prims,
12 scalar attributes each, and separate omitted/authored transform cases.
It verifies the exact `1_000_000_999.125` geometry coordinate. Independent
copied test executables ran in baseline/candidate/candidate/baseline order,
with five measurements per case per process. Their hashes remain distinct
and unchanged. The comparison changes only the snapshot's separate versus
paired reads; both variants use the shared native reader.

Pooled medians, milliseconds:

| Fixture | Separate reads | Paired reads | Reduction |
| --- | ---: | ---: | ---: |
| Omitted stacks, snapshot | 321.369 | 319.751 | 0.5% |
| Omitted stacks, complete recipe | 379.328 | 383.389 | -1.1% |
| Authored stacks, snapshot | 435.189 | 411.983 | 5.3% |
| Authored stacks, complete recipe | 503.993 | 479.841 | 4.8% |

These are owner benchmarks, not Twin loading measurements. A foreign live
application and desktop workloads remain active. The initial unmatched runs
straddled another worktree's build completion and are excluded from the
comparison. Raw logs, executable copies and receipts are ignored under
`.cache/codex-evidence/performance-20261026/paired-transform-*`.

## Production validation

The normal pre-change artifact `24f85e7b` loads Griffin in 7.886 s and reaches
sleeping four-pad handoff. All 271 source hashes remain unchanged; diagnostics
are clear, the rendered landing is inspected, and owned API 5433 closes.
Fixed-tick p50/p95 is 3.756/5.446 ms and frame p50/p95 is 11.861/17.095 ms.
This is a host-qualified baseline; the sub-second load, sub-ms coupled tick
and sub-10-ms graphics targets remain unmet.

The normal candidate artifact `85fb1c61` passes the same complete landing,
source-preservation, diagnostics and visual checks; owned API 5434 closes.
Readiness is 10.196 s, fixed-tick p50/p95 4.540/7.945 ms, and frame p50/p95
11.423/21.493 ms. A new concurrent video encoder consumes roughly 359% CPU
at launch, alongside the pre-existing live application. These mission timings
do not establish an isolated scene-level improvement or regression. Keep the
measured owner gain separate from this contention-affected production check.

The regular `cargo build --locked -p lunco-luncosim -j 4` succeeds without
warnings. Production receipts and images are in the ignored
`griffin-paired-transform-{before,after}-normal/` directories beside the owner
benchmark evidence. No model source, dependency pin or profiling data is staged.
