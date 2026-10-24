# Scalar port call-boundary attribution

The current application retains its complete live owner, metadata, bounds and
slot validation. Compiler-inlining hints at the transaction boundary did not
establish a gain and are removed. No runtime change is retained in this review.

The exact starting application is `64efb1f19ef58cb0d5d6dadf534d557ce6cdf505`.
All 28 port-core checks pass before and during the experiment, including the
manual diagnostics and negative write cases. Twelve co-simulation port-owner
checks also pass. The existing consumer diagnostic stages and commits 768,000
writes with unit, frame and bounds metadata and requires the final exact f64
bits `40b7700000000000`.

Five alternating pairs of the baseline and candidate consumer binaries measure
187.855 ms and 189.179 ms median respectively. The candidate is 0.7 percent
slower in this bounded diagnostic; there is no evidence for adopting the hints.
No product-speed claim follows from this diagnostic. The two copied baseline
test executables are removed after comparison, recovering 303,936,528 bytes.
Source, normal build outputs and shared caches are preserved.

The live-admission diagnostic had a fixture defect: `PortRegistry::default()`
already puts the generic input owner first. Registering sixteen other backends
after that owner did not measure the sixteen preceding owners named by its
output. The fixture now explicitly constructs that order and asserts the
resolved owner index before timing. This is a low-level registry fixture, with
no new application constructor or selection path.

`cargo test --locked -p lunco-port-core --lib live_resolved_input_admission_benchmark -j 4 -- --nocapture`
passes. The corrected fixture measures 131,072 admissions in 16.151 ms. It has
different inputs from the incorrect fixture and is not a speed comparison.
The application runtime and its existing production qualification remain
unchanged. All original loading, coupled-tick and graphics targets remain open.

Raw gates, alternating-run logs, binary hashes and cleanup receipt remain
ignored under `.cache/codex-evidence/performance-20261023/port-inline-*` and
`port-admission-benchmark-fixture.log`.

The current follow-up measured retaining the global topology revision for
one immutable transaction while preserving every live owner, slot, metadata,
value and duplicate-write check. The same consumer diagnostic passes for both
binaries. Five alternating pairs measure 215.414 ms baseline versus 222.314 ms
candidate median (3.2 percent slower), with identical final f64 bits. The
experiment is removed in full; no revision-retention helper or transaction
field remains. Its baseline test hard link is removed after comparison.
Raw comparison evidence stays ignored in `port-revision-*`.

## Source fan-out attribution

The current binding snapshots contain 1,412 Griffin wires / 750 distinct
causal sources, 142 Tesla wires / 82 sources, and 30 Summer wires / 30 sources.
Source identity includes entity, port name and input/output side. This suggests
repeated reads, but does not establish that reading dominates the transaction.

The retained `settled_diagnostic_publication_benchmark` measures the complete
exchange and diagnostic publication with 128 targets, 6,000 transactions and
fan-outs 128, four and one. Every case requires 768,000 writes and final exact
f64 bits `4000000000000000`. Five alternating baseline/candidate pairs give:

| Fan-out | Baseline median | Candidate median | Change |
| --- | ---: | ---: | ---: |
| 128 | 171.261 ms | 171.040 ms | 0.1% faster |
| 4 | 180.801 ms | 179.250 ms | 0.9% faster |
| 1 | 190.173 ms | 190.933 ms | 0.4% slower |

Interning sources and sampling each once per transaction does not establish a
useful gain in this consumer. The experiment and its private instrumentation
are removed completely; production propagation remains unchanged. Its thirteen
focused mechanism checks passed, including canonical order, missing fan-in,
input/output identity, fresh replay samples and topology rebuilds. The diagnostic
extension remains to prevent accepting a fan-out optimization that penalizes
ordinary connections. No production rebuild or runtime rerun is needed for this
test-only result. All original timing targets remain open.

Logs, binary hashes and exact-value receipts stay ignored under
`.cache/codex-evidence/performance-20261023/source-fanout-comparison/`.
The task-created baseline hard link is removed; normal build outputs and shared
caches are preserved.
