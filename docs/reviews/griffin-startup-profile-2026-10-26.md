# Griffin warm startup attribution

Status: measured baseline and rejected snapshot experiment; original timing targets remain open.

The baseline uses application `4064dc006` and Lunar Base source
`78d308e4e4fa97ce3d0b07d3b65437f3c95a4de5`, with all 271 authored source hashes
preserved. Ordinary warm physics readiness is 12.457 s; initializer construction
is 2.672 s with 16,482 native hits and zero compilations. The first process has
prepared/native cache misses and takes 24.588 s. These cache conditions differ.
The warm initializer still compiles its equations in 1.82 s and validates its
prepared FMI cache entry in 0.695 s before live construction.

The separate warm Tracy 0.14.1 run uses app PID 972256/API 5402 and collector
port 8450, with verified socket ownership and 1 ms CPU sampling. The capture has
389,499,090 bytes: 17,930 compressed blocks, 389,427,364 payload bytes and native
framing. The payload matches the collector's rounded 371.39 MiB. Native exports
succeed. The aggregate uses tabs because native zone names can contain unquoted
commas.

The USD facts zone is 1.756 s, with 1,621 selected samples and no missing
stacks. Leading leaves include `CharIndices::next` (276), native
`PathComponents::next` (185) and `validate_identifier` (117). Character iterator
counts do not exclusively attribute every such sample to path validation.
The initializer constructor is 2.643 s, with 2,181 selected samples and no
missing stacks. Leading leaves include symbolic term keys/interning, SHA-256,
tree operations and IR construction. Some function addresses remain unresolved;
no complete function-level percentage split is asserted.

A bounded native-handle-sharing experiment reduces the existing 1,000-prim
snapshot fixture median from 325.253 to 313.794 ms, while full recipe preparation
is essentially unchanged (382.523 / 381.284 ms). Griffin's facts zone is
1.756 / 1.791 s across those diagnostic cohorts. This is not a material loading
improvement, so the production experiment and its helper are removed. The
15 projection checks, 36 exact replay checks, Summer's 102 physics assertions
and Roadster's eight runner requirements passed during the experiment; they do
not qualify subsequent Modelica cache changes.

Both native captures pass framing and native export checks; the second has
361,286,273 bytes, 16,714 blocks and 361,219,411 compressed payload bytes. It
selects 1,509 USD-facts CPU samples with no missing stacks. Both reach sleeping
four-pad handoff, retain source hashes and clear registered diagnostics, and
close their owned applications and collectors. The ordinary candidate run also
lands, but a newly busy foreign Roadster process and differing camera views
invalidate whole-scene timing comparisons. Foreign sessions are untouched.

The next application-owned target is repeated strict Modelica compilation.
See [the typed compiled-artifact cache review](griffin-compiled-dae-cache-2026-10-26.md).
Native IR/proof construction and checked FMI decoding remain separate costs.
Sub-second loading, sub-ms complete physics/coupling service and sub-10-ms
graphics remain open across Griffin, Summer and Roadster.

The two captures and their raw receipts described above were validated before
an external cleanup deleted this worktree's build and profiling directories.
Their measurements remain historical observations; those raw files are no
longer available. The cleanup has stopped. Subsequent compiled-DAE qualification
uses new receipts under ignored `.cache/codex-evidence/performance-20261026/`
and a fresh capture under ignored `scripts/perf/captures/`.
