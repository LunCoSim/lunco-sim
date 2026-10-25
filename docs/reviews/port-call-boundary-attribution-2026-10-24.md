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

The topology-revision retention experiment is not present in the runtime.
Its paired timing result is excluded: `port-revision-comparison.json` records
the same executable hash for baseline and candidate. The receipt does not
distinguish implementations; rebuilding hard-linked outputs can invalidate such
comparisons. It cannot establish a slowdown or reject
revision retention as an optimization. Numerical verdicts remain valid for the
executed binary. Any renewed comparison requires independent executable copies
with distinct, unchanged hashes.

## Source fan-out attribution

The current binding snapshots contain 1,412 Griffin wires / 750 distinct
causal sources, 142 Tesla wires / 82 sources, and 30 Summer wires / 30 sources.
Source identity includes entity, port name and input/output side. This suggests
repeated reads, but does not establish that reading dominates the transaction.

The retained `settled_diagnostic_publication_benchmark` measures the complete
exchange and diagnostic publication with 128 targets, 6,000 transactions and
fan-outs 128, four and one. Every case requires 768,000 writes and final exact
f64 bits `4000000000000000`. The earlier alternating timing result is excluded:
`source-fanout-comparison/receipt.json` records identical executable hashes for
the two supposed variants. It cannot establish the performance effect of source
interning.

The experiment and its private instrumentation are absent; production
propagation is unchanged. Its focused numerical checks covered canonical order,
missing fan-in, input/output identity, fresh replay samples and topology rebuilds.
The retained diagnostic covers large fan-out, ordinary connections and complete
publication. Source-read sharing remains a performance hypothesis to revisit
with a current owner profile and verified independent binaries. All original
timing targets remain open.

Logs, binary hashes and exact-value receipts stay ignored under
`.cache/codex-evidence/performance-20261023/source-fanout-comparison/`.
The task-created baseline hard link is removed; normal build outputs and shared
caches are preserved.
