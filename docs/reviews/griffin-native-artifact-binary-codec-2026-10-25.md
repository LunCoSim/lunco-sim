# Griffin native artifact binary persistence

Status: focused checks and bounded production behavior pass. An isolated
loading/tick performance gain remains unqualified under concurrent workloads.

## Authoritative contract

`lunco-modelica-worker::worker::native_code_cache` owns the version-three native
artifact record in `modelica/native-code-v3/stencils.bin.zst`. Its private record
uses the existing workspace Bincode dependency's typed `Encode`/`Decode` derives,
including supported direct `Arc<[u8]>` encoding. Code buffers remain shared
through publication. No custom parser or encoding is added.

The shared cache transport retains bounded storage reads, decoded/encoded byte
limits, the Zstd window limit, streaming buffering, checksum and explicit flush
failure handling. Bincode additionally bounds allocation at the named 256 MiB
codec ceiling; native decoding requires complete record consumption before
version and duplicate/empty-key-value validation. Cross-process transactions,
LRU/byte limits, delta merge and atomic replacement retain their existing owner.

Prepared FMI records use Rumoca's checked structured deserializer through the
same bounded compression transport. Each record has one codec; the native cache
has one reader/writer in its new namespace. No native JSON reader or conversion
path remains. The ordinary compiler fills a missing optional cache. No source,
FMI state, tolerance, integrator or numerical check changes. Hook review: bounded
artifact persistence and validation are cache-owner invariants, not policy.

## Attribution and focused qualification

The preceding buffered-JSON capture attributes 4,175 cache-thread leaf samples
to loading/publication, with byte-array number parsing and serialization still
leading. Its native load zone is 2.110347 s; two publication zones are 3.168225
and 3.122608 s. Those asynchronous, instrumented durations are not a serial
critical-path sum. See the [stream attribution](griffin-cache-stream-buffering-2026-10-25.md).

All 32 cache-filtered cases pass, including exact native bytes/order, compressed
and decoded byte limits, allocation-size refusal, truncation, trailing bytes,
checksum corruption, duplicates, invalid versions, stale-writer merge,
load-overlap recency, and failed-publication recovery. The 4,096-entry diagnostic
observes 2.880 ms encoding and 5.596 ms decoding, versus the preceding buffered
JSON diagnostic's 31.361/32.806 ms; compressed size is 9,766 versus 15,625 bytes.
These are scoped codec observations, not complete loading or tick gains.
The lockfile adds only the worker's edge to existing Bincode 2.0.1.

All raw evidence remains ignored under
`.cache/codex-evidence/performance-20261023/` and `scripts/perf/captures/`.

## Production admission and first fill

The regular optimized development artifact is
`40a46f6bcb163b3004b851aaf31687d199c6a3c6d1d85e9c97a81c7ca1536bf4`;
its locked four-job/sccache build has no warnings. The new namespace is absent
before first launch; no cache is deleted, converted or preseeded. Griffin first
fill (owned PID 221758/API 5384) reaches physics readiness in 28.270599 s.
Initializer construction is 10.414887 s with 3,958 hits / 12,524 compilations.
It reaches sleeping four-pad handoff at 55.361473 s and finishes the clear soak
at 60.400909 s before the ten-second / 240-fresh-tick window. Coupled p50/p95 is
5.361380/6.575657 ms in that contended window.

The production initialization network and retained-receipt/replacement scenario
then pass all 46 authored checks on owned API 5385. A subsequent Griffin restart
(PID 223158/API 5386) reaches readiness in 25.434811 s; initializer construction
is 6.126777 s with 16,414 hits / 68 compiles. Both initializer cohorts have zero
rejection or eviction. The warm run reaches sleeping four-pad handoff at
76.738166 s, followed by the clear soak and ten-second / 240-tick window.
Coupled p50/p95 is 10.868833/16.832248 ms. All 271 before/admitted/after source
hashes match in both processes; mass/inertia remain finite, neither simulation
object pauses, registered warnings/errors and script errors are zero. The warm
screenshot is inspected. All three owned API ports and processes close.

A foreign Roadster process consumes approximately 3.3 CPU cores throughout these
runs. Separate builds in `tutorials` and `optimization` overlap the warm window,
including rustc work visible in its captured workload receipt. These conditions
invalidate a causal scene-speed comparison against the earlier 12.775 s SDK or
14.153 s buffered-JSON warm observations. The new artifact's codec observations
are qualified; no isolated overall loading, physics or graphics gain is claimed.
Sub-second loading and sub-ms coupled ticks remain open.

## Integration boundary

An unrelated `lunco-mobility/src/raycast_travel.rs` edit appears after the regular
artifact above was built, and a separate regular build starts in this checkout.
It is not part of this cache change or these runtime receipts. No further Cargo
or instrumented production build is launched across that concurrent edit. The
new-format Tracy I/O comparison therefore remains open; the preceding native
capture supplies attribution, and the exact-record owner diagnostic supplies
the scoped codec measurement. The tested regular artifact is not replaced by
this task after the production gates. No foreign process, port, source, staged
change or build is controlled.
