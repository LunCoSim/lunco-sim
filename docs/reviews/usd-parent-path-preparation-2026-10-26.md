# Native parent-path preparation proposal

Status: dependency change proposed, not implemented. The current OpenUSD pin
and clean adjacent checkout are `601f034db12d75f334248d09c0d62645ee4f2f9d`.

## Evidence

The owned Griffin capture on application `d78ee3c70`, API 5435 / PID 201648 uses
the regular development profile with Tracy, 1 kHz CPU sampling and
projection-group spans enabled.
Collector PID 201438 connects on 8454 before launch. The complete landing window
passes readiness, unchanged 271 source hashes, sleeping four-pad handoff,
diagnostics and visual inspection. Both processes and ports close normally.
These are diagnostic timings, not product acceptance.

The largest `usd_projection_facts` interval starts at 3,769,010,326 ns and lasts
1,732,441,531 ns on CSV thread 14. It prepares 2,736 prims. Direct group times:

| Group | Total ms |
| --- | ---: |
| Attribute metadata | 414.954 |
| Prim identity/visibility | 240.525 |
| Connections | 199.647 |
| Attribute values | 174.872 |
| Time samples | 171.144 |
| Traversal | 164.967 |
| Collections | 97.545 |
| Relationships | 69.052 |
| Transforms | 48.602 |
| Material bindings | 46.140 |
| Property names | 39.879 |
| Asset sources | 6.979 |

Collection expansion (94.569 ms) is nested inside collections, and surface
terminal resolution (4.155 ms) inside bindings; do not add those children again.
The remaining interval includes uninstrumented loop/index work and tracing.

Exact metadata zones contain 392 selected CPU samples with no missing stacks.
After resolving application addresses against its recorded ELF mapping and
symbol index, leading leaves are `CharIndices::next` (76),
`PathComponents::next` (50), and `validate_identifier` (30). These leaves do not
establish that every sample originates in parent derivation.

Source inspection identifies a concrete repeated operation:
`Stage::query_at` calls `IndexCache::population_reaches`, which calls
`Path::parent` before probing the population memo. The native owner already
records this as a performance TODO in `pcp/instancing.rs`. `Path::parent`
recreates and debug-validates prefix paths on each call; its result depends
only on immutable path text. Owning-prim derivation already shares immutable
nodes, but parent derivation does not. Existing application APIs do not bypass
this population gate, and its semantics must remain intact.

An application-only experiment reordering inspector-hint lookup produced no
measurable benefit in alternating snapshot benchmarks and was removed. Its
matched logs remain ignored for audit; there is no alternate reader to maintain.

## Proposed native change and acceptance

Extend the native immutable path owner to share parent derivation across clones,
retaining all path grammar, equality, hash, ordering and serialization semantics.
Keep external-input validation and development assertions. Reuse shared prefix
storage; avoid eager quadratic ancestor storage, ownership cycles or recursive
destruction on deep paths. Do not cache population or composition answers in a
path: stage revisions continue to own those answers.

Before publishing, validate empty/root/relative paths, prim/property/variant
paths, nested relationship targets, malformed inputs, clone/concurrent reads,
serialization and deep-path destruction. Compare allocation/time costs for
repeated parents and ancestors, including the one-read construction case.
Exercise native mask/active-state and edit invalidation tests. Then benchmark
the same application snapshot fixtures, qualify Griffin's production landing
and reference-edit behavior, and inspect a matched native capture. Retain the
change only if the complete preparation cost improves without a material
construction or memory regression. Publishing and pinning require explicit
dependency-patch authorization under `AGENTS.md`.

The capture is `scripts/perf/captures/griffin-usd-attribution-20261026.tracy`:
316,710,534 bytes, 14,691 blocks, 316,651,764 payload bytes (301.9827 MiB), matching
the collector's 301.98 MB report. Native zone and CPU exports pass. Raw logs,
maps, symbols, receipts and exports are ignored under
`.cache/codex-evidence/performance-20261026/griffin-usd-attribution-tracy/`.

Local main's terrain integration `6e7a9db35` was fast-forwarded afterward.
The capture does not qualify that later rendering change; it remains evidence
for the recorded application revision and unchanged USD/port owners.
