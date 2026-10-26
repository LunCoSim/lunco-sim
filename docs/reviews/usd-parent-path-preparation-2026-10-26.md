# Shared native parent-path derivation

Status: published to `LunCoSim/openusd` as
`38fd797eadfdb5e8759cb060b7733b7fc962747e`, branch
`perf/shared-parent-paths`. The application pins this revision.

## Profiled baseline

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

## Qualification scope

Parent derivation retains path grammar, equality, hashing, ordering and
serialization. External-input validation and development assertions remain
active. Prefix storage is shared without ownership cycles or recursive
construction/destruction. Stage revisions continue to own population and
composition answers. The user explicitly authorized this dependency patch and
its publication.

The capture is `scripts/perf/captures/griffin-usd-attribution-20261026.tracy`:
316,710,534 bytes, 14,691 blocks, 316,651,764 payload bytes (301.9827 MiB), matching
the collector's 301.98 MB report. Native zone and CPU exports pass. Raw logs,
maps, symbols, receipts and exports are ignored under
`.cache/codex-evidence/performance-20261026/griffin-usd-attribution-tracy/`.

Local main's terrain integration `6e7a9db35` was fast-forwarded afterward.
The capture does not qualify that later rendering change; it remains evidence
for the recorded application revision and unchanged USD/port owners.

## Native implementation and qualification

`Path` shares a lazily derived parent through its existing immutable text node.
Derived nodes share prefix storage and retain owning-prim semantics. The node
releases both owning-prim and parent edges iteratively; deep paths do not recurse
during destruction. External parsing and development validation remain active.
Hashing, ordering and serialization depend only on immutable text. The precise
Clippy key-type exemption records that invariant, with map lookup checked across
parent publication. No population or stage-revision answer is cached here.

All 2,056 native library tests pass, including path grammar, concurrent parent
reads, serialization, low-stack destruction, population and edit tests. Focused
all-target/all-feature Clippy passes. Twelve alternating independent binaries
measure construction plus parent reads. Eight parent reads improve 77.7%; eight
ancestor walks improve 82.4%. Zero/one-read cases differ by roughly 1%.
Each shared text node adds a 16-byte lazy parent slot on this 64-bit target;
queried prefixes retain shared source storage. The complete preparation fixture
and its process peak memory are measured below; mission peak memory is unmeasured.

Raw data stays ignored in
`../openusd/.cache/codex-evidence/parent-path-20261026/`.

## Complete projection benchmark

Eight alternating independent application test binaries run the unchanged
1,001-prim, twelve-attributes-per-component snapshot fixture with and without
authored transforms. Native snapshot medians improve 19.5% and 17.7%; complete
recipe construction plus projection improves 16.9% and 15.8%. All exact-value
and transform assertions pass. Median process peak RSS is 77,382 KiB before
and 80,576 KiB after, a 3.1 MiB increase (4.1%). This is the measured memory
tradeoff in that bounded fixture, not a whole-mission memory measurement.
The source-owned stage and its revision/invalidation rules remain unchanged.
Raw binaries and logs stay ignored under
`.cache/codex-evidence/performance-20261026/native-loading-fixes/`.

## Production and native attribution

The regular candidate passes the ten-check production reference-edit scenario;
its authored fixture remains unchanged. Griffin's source-preserved sleeping
landing, initialization/fault gates and the three-model numerical coverage are
recorded in the [Rumoca qualification](rumoca-assignment-producer-index-2026-10-26.md).

The new Tracy capture belongs to API 5496 / PID 1090867, collector PID 1090826
on port 8458, with 1 kHz samples and projection-group spans. Its largest facts
zone spans 4,472,201,529–5,875,835,208 ns: 1.404 s for 2,736 prims. Metadata totals
346.2 ms; prim identity 177.4 ms, attribute values 153.8 ms, traversal 145.3 ms,
time samples 142.2 ms and connections 129.6 ms. These are diagnostic owner
measurements. Collection expansion and surface-terminal children overlap their
parent groups and are not added again.

Exact metadata export selects 2,736 complete zones and 327 CPU samples with
zero missing stacks. Resolved leading leaves remain character iteration (51),
path components (23) and identifier validation (16). Parent sharing removes
repeated derivation; these samples do not prove that the remaining parsing has
the same caller. A new parsing change requires its own owner attribution.

Startup and the four-pad landing are captured, but the driver exits 1 when one
pending terrain work item invalidates the settled window. Application readiness stays
true and unfaulted; the body is asleep. All 271 source hashes remain unchanged.
This trace does not qualify steady-state graphics or physics timing. Both owned
processes and ports close; the independent regular binary is restored.

Native CSV and exact-zone CPU exports succeed. File framing is 393,293,439 bytes:
six header bytes, 17,810 four-byte lengths and 393,222,193 compressed payload
bytes, matching the collector's 375.01 MiB payload. The ignored capture is
`scripts/perf/captures/griffin-native-loading-20261026.tracy`; maps, symbols,
exports and receipts are under the existing `native-loading-fixes/griffin-tracy/`
evidence directory.
