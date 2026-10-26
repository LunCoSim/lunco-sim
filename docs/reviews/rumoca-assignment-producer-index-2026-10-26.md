# Compact assignment-producer index proposal

Status: native change proposed, not implemented; explicit dependency approval required.

## Current evidence

Application `ce8b553ec` with Rumoca `aae2bbdd` is captured through adjacent
Tracy 0.14.1 on API 5459, PID 554939, collector PID 554833 / port 8455.
The regular development profile has only the opt-in Tracy feature, with 1 kHz
sampling, projection-group spans and executor bookkeeping spans suppressed.
All 271 source hashes are unchanged; both processes and ports close. The normal
production artifact is restored with SHA-256
`84cb317d5cab9df5f75b68d8a28c84a3e193db4d967f0ed88aa4ca27672ce794`.

The capture contains complete startup and four-pad handoff, but does **not**
qualify the requested settled window. At that boundary application readiness
is true and unfaulted; terrain reports one pending tile. The driver exits 1 and
retains the exact qualification facts. No repeat replaces that failure. Video
encoding and another simulator contend with this diagnostic. Neither its frame
times nor its 13.225 s physics readiness are clean performance acceptance.

The initializer's complete `modelica_live_session_construct` zone spans
11,750,080,682–15,335,163,760 ns on native thread 554988 / CSV thread 52:
**3.585 s**. It records 16,482 native attempts/hits and zero compilations.
Native exact-zone export selects one zone, 3,034 CPU samples, zero missing
stacks and a verified 1,000,000 ns sampling period. Unknown application addresses
are resolved against this process's recorded ELF, maps and symbol index.

Inclusive sample groups (overlapping, not additive self times):

| Native owner or operation | Samples |
| --- | ---: |
| Shared-value derivation/checking | 530 |
| Cranelift work and SHA-256 | 442 |
| Assignment producer-range tree operations | 193 |
| Tensor-affine projection/rule preparation | 158 |
| LinearOp cloning, including vector clone frames | 179 |

2,226 samples have only one native stack frame. Although source inspection finds
a whole-model clone for clock anchoring followed by another for runtime
ownership, this capture does not attribute the clone samples to those calls.
It therefore does not justify treating whole-model copying as the dominant cost.
Executable cache hits still build/hash Cranelift IR and prepare checked native
structures; zero compilation is not zero construction work.

## Confirmed owner and proposed repair

`rumoca-ir-solve/src/refresh/assignment_shape/producers.rs` owns
`UniqueProgram`: a borrowed operation sequence and a `BTreeMap<Reg,
(end, position)>`. Its constructor rejects overlapping destinations and invalid
range ends. After construction the table is immutable. Every producer lookup
uses a predecessor tree search; `ProgramPrefix` shares the table and rejects
producer positions after the selected prefix. The sampled tree's exact key/value
type matches this owner, and its predecessor/range operations remain visible
after the direct symbolic-term optimization.

Replace that private immutable tree with one sorted compact vector of destination
intervals. Collect checked starts/ends/original operation positions, sort by
start, reject overlaps through adjacent intervals, and use slice predecessor
search for reads. Keep range storage proportional to operation count, never to
the maximum register number or tensor extent. Preserve empty destinations,
nonmonotone register numbering, gaps, exclusive ends through `Reg::MAX + 1`,
prefix boundaries and all original rejection behavior. Output/certificate
ordering remains program-owned; sorting this lookup does not reorder operations.

This is source-bound immutable indexing, not a numerical or lifecycle policy;
no Rhai hook or application-side replacement is appropriate. No interpreter,
solver tolerance, floating-point operation, artifact schema or public API needs
to change. The native independent checks remain in place.

## Acceptance before publishing

Use the existing producer overlap/overflow, sparse/nonmonotone, full-register
range and earlier-output/later-overwrite tests, plus a focused differential
fixture against an independent linear reference. Benchmark construction and
repeated prefix queries together, with small and large sparse/nonmonotone
programs; reject a material construction regression. Run the affected native IR
checks and owner Clippy, the fixed twenty-model MSL canary, and the unchanged
production initialization, negative-fault and exact-replay gates. Keep the
existing intermittent articulated tick-11 issue explicit. Qualify Griffin's
source-preserved normal landing and compare its constructor under the same
cache conditions; an isolated lookup gain is not sub-second scene acceptance.
Publish to the existing LunCoSim Rumoca fork and pin only a tested improvement.

Raw data stays ignored under
`.cache/codex-evidence/performance-20261026/griffin-direct-terms-tracy/` and
`scripts/perf/captures/griffin-direct-terms-20261026.tracy`. Capture integrity is
317,604,864 bytes: six header bytes, 14,499 four-byte block lengths and
317,546,862 compressed payload bytes (302.836 MiB), matching the collector's
302.84 MB report. Native zone and CPU exports exit successfully.
