# Rumoca pure-call result storage relocation

Status: native fix `5febdef6d489538e26960d9528c352a257431046` is tested and
published to LunCoSim/rumoca branch `fix/pure-call-data-relocation`.
The application pins that published revision; production qualification and its
remaining replay boundary are recorded below.

## Owner and current contract

Rumoca's native typed-program emitter allocates zero-initialized writable result
cells through Cranelift `DataDescription` and `define_data`. Helpers reference
module data symbolically through the supported global-value API, with full-width
address materialization rather than code/data colocation assumptions. The
existing artifact adapter rebinds these data relocations to each current JIT
module. Each primal/directional helper owns separate fresh state; the table
remains non-Sync and drops its code/data arena after callable addresses.

The complete-input bit comparisons, directional seed, publish-after-success,
assertion predicates, function ABI and numerical checks remain unchanged.
Heap-cell retention and its per-table result-buffer vector are removed.
Application code adds no adapter, policy or alternate path. Native execution
and storage lifetime are engine invariants, not a hook decision.

## Reproduced fault and native proof

The previous emitter embeds result-storage addresses as integer constants in
function stencils. The held-two-table regression observes zero native hits for
two equivalent helpers on the second construction. The fixed table reuses every
artifact without compilation, while interleaved primal/directional calls retain
independent storage and exact finite/signed-zero bits. Changed bodies still miss;
dropping one table leaves the other valid.

All 114 native owner checks pass, including the explicitly enabled x86-64 and
AArch64 code-generation test for full-width `Abs8` data relocations. That is not
AArch64/macOS execution acceptance. Scoped all-target/all-feature Clippy passes
with warnings denied. The fixed 20-model MSL 4.1.0/OpenModelica 1.27.1 canary has
12 strict-high comparisons, zero deviations, missing/skipped traces or timeouts.
Six ToDAE refusals and two EL005 lowering refusals remain visible. All 20
phase/error rows and 12 channel-band rows match retained snapshot `d47a5bf3`;
the documented predecessor counts are unchanged. Its raw intermediate directory
is absent, so no bitwise trace delta or runtime delta for refused models is
asserted. The partial run skips the full-cohort gate; no Tier 2 claim is made.

The DCO-signed-off native commit is pushed and its remote SHA verified. Native
[source/verification review](https://github.com/LunCoSim/rumoca/blob/5febdef6d489538e26960d9528c352a257431046/docs/reviews/luncosim-pure-call-data-relocation.md)
records the owner, tests and canary boundary. No cache-key normalization,
compiler-check suppression or precision change is introduced.

## Application admission

The native family moves together to the exact published Git revision. The
lockfile changes source identities only, preserving the complete current graph,
including independently integrated mobility and UI changes. The ordinary
optimization target and four-job/sccache build remain authoritative.

Before the pin, the unchanged application exact-reference gate fails at
articulated tick 11 for `/MultiRoverStress/Rovers/Rover01` in profile
`jitter-0.25-seed-1234567890123456789`. Expected position begins
`[-7.499999989098277, 1.5772187814000063, ...]`; observed begins
`[-7.500000000324079, 1.577218776481459, ...]`. This is a pre-pin finding;
no reference or tolerance is changed and no causal attribution is made from
that failure alone. The owned baseline API 5388 exits. This finding remains
separate from native artifact-reuse qualification and the passing candidate run.

All ten unchanged candidate exact-replay profiles pass 360 authored checks on
owned API 5397, covering the maintained jitter/seed and roster/thread matrix.
The reference SHA-256 remains
`6247db0dfefbd0eaedad3d712c359578f30a75addb6daea4a79584b784a70451`.
No reference, tolerance or behavioral assertion is weakened. This bounded pass
does not resolve the separately documented
[intermittent first-step articulated finding](physics-first-step-joint-replay-2026-10-25.md).

The regular locked application build passes with four jobs and sccache. Its
SHA-256 is `288648c323e8251648c9eb96379a869e5cdb1b653ea1654651f6831250b6b029`.
Production initialization/receipt retirement passes 46 assertions, source-target
passes 20, and lifecycle passes 21. Both negative scenes report their expected
terminal initialization/step faults with a passing production verdict. The
43-skill catalogue and both MCP skill-reader checks pass.

The bounded Summer Lunokhod physics gate passes 102 assertions and Roadster's
launch/runner verification passes eight requirements. Both production verdicts
and process exits pass, with unchanged authored source hashes and released owned
API ports 5398/5399. These gates do not assert complete mission/trick acceptance.

Griffin's first process and warm restart preserve all 271 source hashes, reach
four-pad handoff and sleeping unpaused lander state, retain clear registered
diagnostics and script logs, and complete a five-second readiness soak followed
by at least ten seconds and 240 fresh simulation ticks. Screenshots are inspected;
the owned API sessions 5395 and 5396 exit and release their ports.

The first process after the pin constructs the initializer in 6.453 s with
16,482 attempts, 7,958 hits and 8,524 compilations. This uses the existing
bounded native store, not an empty cache namespace. The warm restart records
**16,482 attempts, 16,482 hits and zero compilations**, with zero rejection or
eviction. This removes all 68 recurring warm misses observed before the pin,
consistent with the checked wire's 34 pure-call owners and their two forms.
Warm construction still takes 3.378 s: native IR/proof preparation and hashing
are not eliminated by code reuse.

First/warm physics-ready observations are 20.563/17.229 s. Warm fixed-tick service
p50/p95 is 7.715/10.985 ms in its final 240-sample window. Desktop/video-player
workloads differ across runs, so these are bounded observations, not an isolated
loading or tick speedup. Sub-second loading and sub-ms coupled ticks remain open.
The separate native-cache I/O Tracy capture also remains open; this patch's
artifact-reuse claim comes from native regression and production counters.

Raw logs, references, source snapshots and captures remain ignored under
`.cache/codex-evidence/performance-20261023/rumoca-pure-call-relocation-*`,
`pure-call-relocation-*`, `griffin-pure-call-relocation-*` and
`scripts/perf/captures/`. No raw profiling data is tracked. All owned production
sessions are closed; the ordinary production artifact remains available.
