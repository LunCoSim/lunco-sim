# Compact assignment-producer index

Status: published to `LunCoSim/rumoca` as
`41637b1eba187ce1324aa1c511ad55e2f27815f4`, branch
`perf/compact-assignment-producers`. The application pins this revision.

## Profiled baseline

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

## Prepared-cache reconstruction attribution

A separate exact-zone export from the same capture isolates the initializer's
`modelica_solve_preparation_disk_cache_lookup` on OS thread 554983 / CSV thread
22. The zone takes **1.089 s**. Export selects one complete zone, 876 samples,
zero missing stacks and the same 1 ms sampling period. Application addresses
are resolved from the capture's recorded ELF load base and symbol table.
Of these samples, 485 have only one frame, limiting caller attribution.

Inclusive groups overlap and must not be summed:

| Native operation | Samples |
| --- | ---: |
| Structural dependency propagation | 179 |
| Serde, including JSON parsing | 94 |
| Assignment producer-range tree operations | 76 |
| Assignment shape and exact refresh coverage | 55 |
| Jacobian output catalog/seed domain | 35 |
| Zstd/checksum routines | 15 |

The application `PreparedSolveCache::load_disk_at` reads bounded compressed bytes
and invokes Rumoca's canonical `deserialize_fmi_component`. That owner invokes
`deserialize_solve_model`: it validates correlations, derives executable
artifacts, reconstructs reduced charts and validates the model before constructing
the checked FMI component. Thus the cache-lookup label includes reconstruction
and semantic validation, not just disk I/O or parsing. The regular production
run at application `646ea228f` observes 1.188 s for this lookup and 3.604 s for
the subsequent initializer construction; those sequential measurements have
different owners and must not be conflated.

This adds a second measured consumer for the producer index. It does
not justify bypassing native validation or a broad cache-codec replacement.
Structural dependency work needs its own concrete proposal and permission
before dependency changes beyond the producer index. Exact exports and resolved
sample tables are ignored under
`.cache/codex-evidence/performance-20261026/griffin-direct-terms-tracy/`, using
`initializer-cache-*` and `prepared-cache-*` names.

The separate 267 ms `dataset_twin_manifest_scan` zone yields only 25 CPU samples,
all resolving to hardware SHA-256 compression. Source inspection identifies
manifest integrity checking of raw terrain inputs. The small sample count
does not attribute the remaining elapsed time or establish that this concurrent
scan delays scene admission. Integrity checks remain unchanged.

## Authoritative owner and implementation

`rumoca-ir-solve/src/refresh/assignment_shape/producers.rs` owns
`UniqueProgram`: a borrowed operation sequence and an immutable sorted vector
of destination intervals with original operation positions. Its constructor
rejects overlapping destinations and invalid range ends. `ProgramPrefix` shares
the vector and rejects producer positions after the selected prefix.

Construction collects checked starts, ends and original positions, sorts by
start, and rejects overlaps through adjacent intervals. Reads use slice
predecessor search. Storage is proportional to operation count, with no tensor
scalarization. Empty destinations, nonmonotone register numbering, gaps,
exclusive ends through `Reg::MAX + 1`, and prefix boundaries remain covered.
Sorting the index does not reorder the source operations.

This is source-bound immutable indexing, not a numerical or lifecycle policy;
no Rhai hook or application-side replacement is appropriate. No interpreter,
solver tolerance, floating-point operation, artifact schema or public API needs
to change. The native independent checks remain in place.

## Application qualification

The regular build passes 46 initialization/receipt checks, both expected
Modelica terminal-fault fixtures, ten USD reference-edit checks, Summer's 102
physics checks and Roadster's eight manifest verification checks. All ten exact
replay profiles pass against the unchanged reference, SHA-256
`6247db0dfefbd0eaedad3d712c359578f30a75addb6daea4a79584b784a70451`.
This run does not establish a repair for the previously intermittent articulated
tick-11 failure. Every owned process and port closes.

The fresh Griffin baseline is 8.341 s to physics readiness. The candidate's
first fill is 14.568 s because compiler identity `41637b1eba18` invalidates its
prepared/compiled caches. Its warm run is 8.295 s. Both candidate landings
preserve all 271 source hashes and finish at sleeping four-pad handoff with zero
registered warnings/errors and inspected rendering. This pair does not show a
meaningful whole-mission loading gain.

Initializer construction is 2.380 s in the baseline, 2.196 s in the first fill
and 2.609 s in the warm candidate; every run has 16,482 native hits and zero
native compilation. Prepared-cache lookup is 0.681 s baseline and 0.649 s warm
candidate; first fill lowers/prepares for 3.253 s. These measured owners overlap
other startup work and do not explain the full readiness interval. The warm
candidate's tick/frame p95 also worsens (6.311/21.029 ms versus 4.194/12.080 ms).
The isolated index gain is established; a whole-mission performance improvement
is not. Requested loading, physics and graphics targets remain open.

An initial baseline command named the wrong body path and was closed through
its owned API. An initial Roadster command omitted the required scene and was
rejected before execution. Both invalid invocations are retained in ignored
logs; the corrected runs provide the qualification above.

Baseline trace data stays ignored under
`.cache/codex-evidence/performance-20261026/griffin-direct-terms-tracy/` and
`scripts/perf/captures/griffin-direct-terms-20261026.tracy`. Capture integrity is
317,604,864 bytes: six header bytes, 14,499 four-byte block lengths and
317,546,862 compressed payload bytes (302.836 MiB), matching the collector's
302.84 MB report. Native zone and CPU exports exit successfully.

## Native qualification

The published commit passes 424 IR tests (two diagnostics ignored), focused owner
Clippy, and the fixed twenty-model MSL canary. The canary preserves every model's
phase, state/algebraic counts and refusal codes: six DAE refusals, two solve
refusals and twelve high-agreement traces. Its numerical error and initial-value
summaries match the preceding canary. State-selection comparison is unavailable
in this run (zero compared); it is not claimed as fresh coverage.

Twenty alternating independent test-binary runs compare checked construction and
prefix queries, including exact agreement with a linear reference. Combined
median time improves 68.4%, 64.9%, 78.2% and 88.5% for 4, 64, 1,024 and 8,192
operations respectively. Construction-only improves 75.5–90.1%. These native
fixture gains do not establish a whole-scene speedup. Raw data is ignored in
`../rumoca/.cache/codex-evidence/producer-index-20261026/`.

## Cache-churn lead

The subsequent Tracy run follows the numerical test matrix and has a different
cache state. Its initializer prepared lookup misses, lowering/preparation takes
3.955 s, and construction takes 7.721 s with 7,964 native hits, 8,518 compilations
and 8,518 evictions at the 32,768-entry bound. This is not comparable to the warm
constructor timings above. Exact-zone export selects one interval and 6,755 CPU
samples with zero missing stacks and a verified 1 ms sampling period. Cranelift
verification and register allocation lead its resolved leaves.

The shared native cache already has an application owner,
`lunco-modelica-worker::worker::native_code_cache`, and typed
`ModelicaCacheCapacities`. Investigate workload retention and existing budgets
there before proposing another dependency API or bypassing native checks.
No cache capacity, solver policy or numerical tolerance changes in this patch.

## Portable pin build

`cargo build --locked -p lunco-luncosim -j 4` passes with the published Git
revisions, normal sccache and the regular checkout target. Binary SHA-256 is
`599893cb6a9f85022a377386493a710e78552c7c5dbe5cd02cda6ef79da27178`.
The behavioral gates above exercised the identical dependency commits through
local paths; the final loading passes below exercise the portable pins.

| Twin | API / PID | Physics readiness | Settled window |
| --- | --- | ---: | --- |
| Summer | 5497 / 1136523 | 2.803 s | Fails when terrain has one pending tile |
| Roadster | 5498 / 1136772 | 2.984 s | Passes, clear diagnostics |
| Griffin | 5499 / 1137335 | 9.120 s | Passes, clear diagnostics and sleeping four-pad handoff |

Every source snapshot is unchanged (72/137/271 files), and every owned process
and port closes. Summer remains ready, unfaulted and unpaused when its terrain
window fails; no passing repeat replaces that failure. Final Roadster and Griffin
screenshots are inspected. Their views differ from the initial baseline, so
frame timings are observations, not a matched graphics comparison.

Roadster's coupled tick p50/p95 is 1.679/2.409 ms and application frames are
6.706/12.741 ms. Griffin's values are 3.691/4.263 ms and 10.102/12.905 ms. Its
initializer is fully warm (16,482 hits, zero compilations), with 2.487 s live
construction and 0.744 s prepared-cache lookup. The published pin has verified
functional behavior and isolated owner gains; neither sub-second loading nor a
whole-mission speedup is established.

Current application receipts, test logs and binaries remain ignored under
`.cache/codex-evidence/performance-20261026/native-loading-fixes/`.
