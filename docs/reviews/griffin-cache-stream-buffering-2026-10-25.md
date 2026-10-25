# Griffin cache compression buffering

Status: focused and production qualification pass; the original performance
targets remain open.

## Owner and cause

The native cache's I/O lane appears in 4,518 selected 1 kHz CPU samples from
the validated shared-assignment startup capture (owned PID 144113, OS thread
144195). Symbolization uses that process's saved maps and ELF index. Leaves
include the Zstd stream call (788), JSON byte-array visitation/parsing, the
stream writer and the cache budget writer. These are sampled CPU attribution,
not wall-clock or non-overlapping phase times.

`worker::cache::encode_record` sends the serializer's small numeric/punctuation
writes directly to Zstd. This repeatedly enters streaming compression even
though the encoded format is a continuous JSON record. Both native artifacts
and checked prepared-FMI records consume that shared codec.

The change uses `std::io::BufWriter` between the decoded-byte budget writer and
Zstd. The outer budget rejects oversized serialization before buffering;
explicit `into_inner` flushes before checksum/frame finalization and propagates
errors. The compressed budget remains below Zstd. This preserves the wire,
cache namespace, atomic publication, cross-process merge, numerical state,
compiler checks and all cache limits. No dependency or policy mechanism changes.
Hook review: bounded streaming and error propagation are cache-owner invariants.
Bounded native load/publication spans expose this I/O lane to future captures.

## Focused checks

The 4,096-entry / 512-byte-per-entry roundtrip diagnostic retains exact keys,
code bytes and entry order. Baseline encoding/decoding observes 92.565/30.585 ms;
the buffered candidate observes 31.361/32.806 ms. The first command runs the
single diagnostic, the second all cache cases; these are isolated codec
observations, not a controlled complete-scene speedup ratio. Compressed size
is 15,625 bytes in both. All 31 cache-filtered checks pass, including compressed
and decoded limits, explicit write failures, corrupted records, ownership,
retention, stale-writer merge and independent invalidation.

Raw logs, CPU samples and captures remain ignored under
`.cache/codex-evidence/performance-20261023/` and `scripts/perf/captures/`.

## Production qualification

The regular optimized development artifact is
`8945aec50a96389832adb1b7a2cdc625766e3c7b6eb0b8ed40dbd7c8de330fb5`.
The production initialization network and retained-receipt/replacement gate pass
all 46 authored checks on owned API 5381. Griffin (PID 207278/API 5382) reaches
physics readiness in 14.152969 s, then four-pad handoff at 35.963744 s and the
five-second clear soak at 41.155163 s. The fresh ten-second / 240-tick window
observes coupled p50/p95 3.475709/4.288407 ms. All 271 source hashes match
before/admitted/after; the body sleeps with finite mass and inertia, unpaused,
with zero registered warnings/errors and no script errors. The inspected
screenshot shows the four-pad handoff HUD. The session closes normally.

The preceding native startup-only control observes 15.381713 s, while the
separately qualified SDK warm run observes 12.775131 s. Host/view differences
prevent an isolated total-loading or graphics ratio. Sub-second loading and
sub-ms coupled ticks remain open.

## Native attribution and artifact restoration

A separate startup-only Tracy capture owns app PID 211593/API 5383 and collector
211548/port 8449. Socket ownership matches both processes. All 271 source hashes
match the normal cohort, and both processes/ports close. The 264,701,917-byte
capture contains 11,927 native blocks and 264,654,203 compressed payload bytes;
framing matches the collector's rounded 252.39 MiB. Native export reads
3,915,982 zones. The native-cache load zone takes 2.110347 s; two publication
zones take 3.168225 and 3.122608 s, on the dedicated I/O thread. Their work
remains asynchronous and these are diagnostic, not critical-path sums.

CPU export verifies a 1 ms period, 97,768 selected samples and no missing stacks.
The cache thread has 4,175 leaf samples. Direct per-fragment Zstd stream entry
no longer leads; buffered budget writes and JSON byte-array parsing do. The
comparison capture has 4,518 cache-thread leaf samples, with a smaller retained
record and different concurrent load, so no isolated production CPU ratio is
asserted. This confirms the next remaining persistence cost: decoding and
encoding native code as individual JSON numbers. No decoder, format or namespace
change is part of this fix.

Instrumented readiness is 17.720854 s; the initializer constructs in 3.436001 s
with 16,414 native hits, 68 compiles and no rejection/eviction. The regular
artifact is restored exactly to the SHA above; its build has no warnings.
