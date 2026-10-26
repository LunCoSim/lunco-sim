# Reuse strict compiled Modelica artifacts across processes

Status: cache and mission qualification complete; startup and tick targets remain open, with intermittent exact replay still unresolved.

## Owner and measured cause

`lunco-modelica-worker` owns strict compiled artifacts before its persistent
prepared-FMI cache. Griffin's [pre-change warm baseline](griffin-startup-profile-2026-10-26.md)
spends 1.82 s compiling the combined initializer even with prepared/native hits.
Other component equation compilations add work on the same compiler actor.

The existing `CompiledArtifactCache` owns the extension. The native compiler
actor enables bounded persistent reuse after validating its existing
`ModelicaArtifactDiskLimits`; browser and memory-only test hosts retain their
configured memory cache. No third-party source or dependency change is needed.

## Identity, validity and lifetime

`ModelicaCompiler::admitted_compilation_identity` captures every admitted source
set, its exact contents, admission order and diagnostic URIs. It rejects a
failed latest admission. Unlike the participating-source identity used by
prepared FMI, this conservative pre-compilation identity includes provenance
and unrelated admitted roots; it must be available before compiler execution.
The key also includes the selected model, primary/extras source and URI,
structural binding names and exact f64 bits, clean native compiler revision,
host OS/architecture and the artifact format version. A location or admission
identity change may invalidate a result whose semantic source bytes match.

The worker retains only the SDK's checked DAE and the five experiment defaults
consumed by its result publication. Flat IR and the additional compiler source
map/diagnostics are released after participating-source identity capture. The
persistence boundary serializes this runtime model. Its native DAE deserializer
validates the schema and reconstructs the branded
graph through `Dae::construct`; no application reconstruction or compiler-check
suppression is introduced. The current SDK enables exact float round-tripping.
The stored full input CID is checked before admission, and the original
participating-source CID continues to own prepared-FMI identity.

DAEs with loaded external table data are not persisted: their resource bytes
are not represented by this source-only input closure. The native compiler
continues to own their ordinary compilation. Extending persistence to those
models requires captured resource identities, not a guessed timestamp or path.

The versioned `modelica/compiled-dae-v2` family reuses the existing bounded,
checksummed compression transport, full-consumption decoder, storage
transaction, recency metadata and retention mechanism. The full input CID in
the record validates its bounded filename key. Corrupt, mismatched or
unsupported optional records are diagnosed and do not admit a compiled result.
Only a fresh strict compilation can satisfy a cache miss. Each artifact family
uses the admitted record/byte limits; generated data is not tracked.

Source admission and compilation remain on the same FIFO compiler actor.
Checked DAE data and consumed defaults may be retained; executable arenas, FMI
instances, initial numerical state, scene entities and live sessions are not.
Prepared-FMI validation, native artifact relocation, FMI initialization and
deterministic live publication continue through their existing owners.

Hook review: source identity, graph validity, bounded storage and compiler
reuse are engine mechanisms. No lifecycle selection or Twin policy is added,
so no Rhai hook is appropriate.

## Verification

Three low-level cache checks pass: exact serialized artifact/source identity
round trip with native invalid-schema rejection; exact input/URI/source-root
and signed-zero binding keys; and refusal to encode uncaptured external table
resources. The identity check also passes after adding fresh-context equality,
extra-source changes, changed library bytes and failed-latest-admission cases.
The regular application build and scoped worker Clippy command complete.
Clippy reports 20 existing worker warnings and existing dependency warnings;
none point to the added cache code. This is not a warning-free crate gate.

Two regular production processes on application `0eee529a3` use the same
executable and preserve all 271 source hashes on Lunar Base `78d308e4`, including
the recorded local environment edits. First-fill scene readiness is 13.414 s;
the immediate warm process is ready in 8.683 s. The initializer strict compile
is 2.04 s on first fill; the warm checked compiled-artifact lookup is 146.275 ms.
All 13 compiled models hit in the warm process. Prepared-FMI lookup remains
699.246 / 771.035 ms and live initializer construction remains
2.408 / 2.322 s, respectively. Both constructors record 16,482 native hits,
zero compilations and zero rejected entries. These overlapping owner intervals
must not be added to infer an exclusive scene-cost breakdown.

Both processes reach authored landing handoff with a sleeping lander, clear
readiness and zero registered errors/warnings. Each retains a five-second
readiness soak followed by at least ten seconds and 240 fresh ticks. The warm
final screenshot was inspected. Tick p50/p95 is 3.563/6.047 ms on first fill and
4.031/6.670 ms warm; frame p50/p95 is 10.341/15.658 and 11.038/20.925 ms.
Concurrent foreign Roadster/browser workloads and differing camera conditions
qualify these timings; they do not establish a clean graphics acceptance or
an isolated whole-scene speedup ratio. The original timing targets remain open.

Current receipts and logs are ignored under
`.cache/codex-evidence/performance-20261026/griffin-compiled-dae-lean-{first,warm}-normal/`.
Unlike the earlier baseline's externally deleted files, these receipts were
captured after cleanup stopped. The reusable Linux production driver is
`scripts/perf/twin_startup.py`; it rejects source changes and failed runtime,
render or authored-script boundaries before admitting its measurements.

The production initialization/receipt gate passes 46 assertions; source-target
passes 20; both terminal-fault scenes report their expected initialization/step
fault kind; Twin retirement/duplicate lifecycle passes 7 + 14. Summer's physics
scene passes 102 assertions and Roadster's runner passes eight, with their
authored source hashes unchanged. Owned sessions exit and release their ports.

The first unchanged exact-replay matrix passes nine of ten profiles. The serial
four-rover case fails its articulated tick-11 row, matching the boundary in the
[open first-step investigation](physics-first-step-joint-replay-2026-10-25.md).
One independent unchanged repeat passes that profile exactly. This provides a
passing observation for each profile but does not make the matrix reliably
green or resolve the intermittent failure. The reference, zero tolerance and
all authored assertions remain unchanged. Logs and per-run receipts live in
the ignored `compiled-dae-production/` evidence directory.

## Fresh native attribution

The regular pre-profile process after a machine restart reaches readiness in
7.643 s, with all 13 compiled hits and a 2.155 s initializer construction. It
retains all sources, reaches sleeping landing handoff and clears diagnostics.
Its tick p50/p95 is 3.296/4.148 ms and frame p50/p95 is 9.307/12.151 ms.
The changed desktop workload makes this a separate observation. The preceding
preparation process was interrupted by the restart and supplies no completed
mission or performance acceptance.

The qualified Tracy 0.14.1 run uses application PID 40098/API 5431 and collector
PID 40010/port 8453, with socket ownership verified. All sources match before,
at admission and after the complete landing/measurement window. The screenshot
is inspected, diagnostics are clear, and both owned processes/ports close.
Native framing verifies 319,323,538 bytes, 14,562 blocks and 319,265,284 payload
bytes, matching the collector's rounded 304.48 MiB. Native CSV and CPU exports
complete; the measured sampling period is 1,000,000 ns.

The largest compiled-artifact lookup is 88.188 ms with 65 selected CPU samples.
The initializer constructor is 3.260 s with 2,838 samples; USD facts are 1.829 s
with 1,784 samples. Each selection uses the exact zone's thread and interval,
with no missing stacks. Constructor leaves include symbolic term interning/key
construction, IR clones, operand validation and native cache hashing. USD leaves
include character/path iterators and identifier validation. Some function
addresses remain unresolved; this is not a complete exclusive cost breakdown.
The profile still records 13 compiled hits and no initializer native compiles.

An earlier valid startup capture (PID 36977/API 5430) was rejected for settled
performance after a qualification guard changed. It is excluded from settled
acceptance. The reusable driver now retains the exact readiness, body, terrain
and handoff facts for that guard, plus terminal process/port and source status.
The complete repeat uses the same thresholds. Raw captures and exports remain
ignored; the regular production build is restored through Cargo after profiling.

## Integration

Optimization fast-forwards to local main `3553f2618`, preserving its terrain
shader and solar-system asset changes and all unrelated review files. The
integrated regular binary `24f85e7b` passes the complete Griffin run on owned
API 5432, with 271 unchanged source hashes, sleeping landing handoff, clear
diagnostics and inspected rendering. Its readiness is 18.474 s, initializer
construction 4.427 s and compiled lookup 129.512 ms. Separate `usd` worktree
Rust compilation consumes several cores throughout the recorded interval;
this is functional integration evidence, not a clean timing comparison.
The app exits through its API and the port closes. Generated profiling data
remains untracked. The exact replay reference is unchanged.
