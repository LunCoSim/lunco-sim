# Griffin startup path derivation

Griffin startup remains above its target. The normal baseline reaches scene
readiness in 14.036 s. Its native USD facts snapshot takes 3.520 s in the
separate startup capture, with 3,488 selected CPU samples.

The unmodified upstream OpenUSD candidate is
`9375048e5e7d75c3701b346fbbea2c0b706443e8`. Its native binding cache replaces
independent ancestor-binding walks within the immutable snapshot. The complete
API migration uses native field, list-operation and schema methods; the duplicate
application field reader is removed. Irreducible native list-operation
composition returns a path/field error from authored-layer transport.

Validation: 18 authoring, 53 stage and 94 document checks pass; the composed-read
integration target passes 19 checks. Its pre-existing ignored repeated-prim-spec
parser case still fails when explicitly exercised. The production reference-edit
gate passes ten authored checks on owned API 5196/PID 472825 and exits normally.
Griffin lands with zero diagnostics on API 5195/PID 465532, unchanged source
revision `78d308e4e4fa97ce3d0b07d3b65437f3c95a4de5`, and an inspected image.

That normal candidate reaches first readiness in 27.814 s and landed eligibility
in 65.624 s. It is not a speedup: active swapping and concurrent simulator/browser
workloads affect the run. Even unchanged Modelica compilation and prepared-cache
lookup grow substantially. The startup-only candidate Tracy capture is valid,
310,722,780 bytes with 13,714 blocks and 310,667,918 compressed payload bytes;
collector and native exporter exit zero. Its USD-facts zone is 3.509 s, with
3,245 CPU samples and no missing stacks. About 44 percent of the selected samples
are string/path iteration or identifier validation. This does not establish an
end-to-end loading improvement.

An application handle-retention experiment did not establish a reliable gain.
The inline 1,001-prim/12,000-value diagnostic measured a baseline median
399.496 ms, an initial candidate at 383.032 ms, and a broader candidate at
509.892 ms. The experiment is removed. Its native add/edit/remove checks passed,
but correctness alone does not justify additional application cache ownership.
The explicit diagnostic benchmark remains available for the native change.

## Native path owner

The user approved modifying and publishing the existing `../openusd` /
`LunCoSim/openusd` fork. Its `fix/shared-prim-path-derivation` branch starts at
upstream `9375048e5e7d75c3701b346fbbea2c0b706443e8`.

The C++ SDK uses immutable shared prefix-tree nodes ([OpenUSD path API](https://github.com/PixarAnimationStudios/OpenUSD/blob/release/pxr/usd/sdf/path.h#L255-L265)).

Owner: OpenUSD `sdf::Path` and its immutable shared text. Consumer: native
composition/property readers, including Griffin's snapshot. Each path retains
its owning-prim node. Prefix nodes share the source string allocation, and their
hash, equality, ordering, display and serde use their exact text view. Construction
and release walk the prefix chain iteratively. Map keys stay immutable. Input
parsing and derived-prefix validation retain their checks. This is pure path
structure with the same lifetime as the shared native text; no Rhai hook applies.

The derivation-sharing and concurrent-sharing regressions fail on the unmodified
base. The property-owner sharing regression also fails before its constructor
retains the supplied prim. The published head is
`601f034db12d75f334248d09c0d62645ee4f2f9d` on
`fix/shared-prim-path-derivation`; the remote SHA matches and the fork is clean.
The native unit gate passes 2,053 checks, with its timing diagnostic exercised
separately. All-target/all-feature Clippy passes with warnings denied. Coverage
includes prefix-source sharing, hash/ordering, serde, eight-thread reads and
release of a 512-variant chain on a 64 KiB thread stack. No lint exemptions are
introduced.

Native property construction retains its supplied owning-prim node.
The explicit native derivation diagnostic measures 545.352 -> 16.871 ms. The
application consumer uses the regular repository test profile and the same
inline 1,001-prim/12,000-value fixture in both binaries. Three alternating pairs
retain all fifteen observations per version and include both snapshot-only and
complete parse/composition/plan timings. The pooled snapshot median is 455.280 -> 393.870 ms (13.5 percent faster);
complete preparation is 509.029 -> 534.505 ms (5 percent slower). The complete
preparation measure prevents accepting work merely moved to construction.
All 53 application stage checks and 19 composed-read checks pass on the pin;
the explicit timing check remains separate, and the known repeated-prim-spec
parser case stays ignored. The regular production Griffin reaches observed scene readiness in 12.996 s
(previous normal observation 14.036 s). Requested-Twin/native-physics readback is
observed at 13.302 s. Landed eligibility is 35.210 s; the inspected image shows
the lander, terrain and shadow, with zero diagnostics and unchanged authored
hashes. Owned PID 923871/API 5199 exits and releases its port. This remains a
contention-affected observation, not sub-second loading or causal timing
acceptance. Complete coupled tick median/p95 is 4.004/4.836 ms; graphics
11.549/14.624 ms. Coupled compilation takes 1.85 s, prepared-cache lookup 0.680 s,
and session construction 2.891 s; the native attempt/hit/compile counts remain
16,482/16,414/68. Modelica initialization remains the main next owner. The final-pin production
reference-edit gate also passes all ten authored checks on owned API 5201;
the app exits and releases the port.

The confirmed normal SDK-only run reaches readiness in 22.455 s and landed
eligibility in 54.595 s on API 5198/PID 569998, then exits cleanly. Its source
hashes are unchanged, diagnostics are zero, and its image is inspected. Recorded
memory/IO pressure and concurrent workloads qualify these timings. The SDK-only
cohorts do not establish a loading gain. The original sub-second loading,
sub-millisecond coupled tick and graphics targets remain open.

All traces, sampled stacks, process maps, timing receipts and images are ignored
under `.cache/codex-evidence/performance-20261023/` and `scripts/perf/captures/`.

## Current constructor attribution

The separate final-pin startup capture uses owned PID 971514/API 5202 and
Tracy collector 8428. Both exit normally, authored hashes remain unchanged and
the inspected image shows the in-flight lander and terrain. The capture is
319,878,375 bytes: 14,607 native blocks and 319,819,941 compressed payload bytes.
Native aggregate and exact-zone exports succeed; CPU sampling is 1,000,000 ns.
This startup-only run is not a landing or settled frame-time acceptance gate.

Observed native USD facts time is 1.788 s, versus 3.509 s in the SDK-only
capture. Different concurrent pressure prevents treating that as a controlled
speed ratio. Requested-Twin/native-physics readiness is 17.929 s with Tracy.
The initializer constructor is 3.495 s and has 3,010 selected CPU samples,
with zero missing stacks. The native output lookup fixed earlier appears in
only one selected sample. Shared-value construction and proof functions appear
in 505 sampled stacks; these inclusive counts overlap and are not self times.

The next proposed native change belongs to Rumoca's exact refresh schedule
construction. `InterpretedSchedules::construct` calls `shared_segments`,
materializes a checked scalar block and retains its interpreter. The native
backend later calls `compile_exact_assignment_schedule`, which calls
`shared_segments` again for the same immutable schedule. Both derivations run
the symbolic equivalence proof and recreate assignment operations.

Proposal: retain one proven `SharedAssignmentSchedule` per issued runtime
sequence and let the interpreter and native backend consume that typed result.
The source/owner validation and full equivalence proof remain at construction;
mutable solver values and output scratch remain instance-owned. Update the
backend contract and all consumers together, remove the redundant derivation
entry point, and avoid a global or hash-keyed proof cache. No lifecycle policy
or Rhai hook belongs in this pure IR construction.

This extends the earlier typed-key optimization into shared proof ownership
and requires explicit dependency-source/publication approval. Before adopting
it: cover ordered writes, repeated targets, opaque programs, invalid schedule
ownership and separate runtime scratch; run the native schedule checks and
20-model MSL canary, then compare the complete initializer's f64 observations
and regular Griffin readiness/landing. Profiling alone does not establish the
size of the potential end-to-end improvement.

The initializer's prepared-cache lookup is 1.129 s in that capture, with
654 selected CPU samples and no missing stacks. Its leading leaves are native
dependency walks, assignment-shape construction and coverage validation; JSON
string parsing accounts for ten samples and zstd decoding for nine. Changing
cache codecs is not supported as the next optimization by this attribution.
Retain native checked deserialization.
