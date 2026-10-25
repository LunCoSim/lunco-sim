# Griffin native compiler cache publication

Status: application-owned cache repair passes focused and production qualification on
main's Rumoca `b825929b` dependency. The separate `fbfbcdc9` upgrade remains
published and held for its exact-replay investigation.

## Measured loading path

With `fbfbcdc9`, a normal repeat reaches first physics readiness in 19.737 s;
the coupled initializer constructor takes 7.321 s, with 16,482 native attempts,
3,974 hits and 12,508 compilations. The prepared FMI component is a disk hit;
its checked load costs 0.701 s. Landing eligibility is a later mission milestone,
not loading time. All 271 source hashes match the preceding normal cohort.

An immediate key diagnostic observes 37,069 attempts across thirteen components,
36,929 hits, 140 misses and 28,314 distinct requested keys. Of those keys, 28,174
are already in its initial cache; no initially present key is evicted before a
miss. The initializer hits 16,414 times and compiles 68 kernels, constructing in
3.064 s. Trace timings are diagnostic. This does not justify increasing the
32,768-entry / 64 MiB budgets.

The fresh Tracy startup capture uses owned PID 3815759/API 5349 and collector
3815693/port 8445, with 1 kHz sampling. Its 344,329,301-byte file contains 15,455
native blocks and 344,267,475 payload bytes; framing and native zone/sample
exports pass. The selected initializer zone has 7,549 samples on its own worker,
with zero missing stacks. Owned ELF maps/symbols resolve 3,419 frame addresses;
leading leaves are native Cranelift verification, register allocation and
optimization. That constructor again has 12,508 compilations and takes 8.694 s
under instrumentation. No numerical or compiler checks are suppressed.
The capture is startup-only: a terrain request appears during descent and
invalidates its later settled window. Its process/collector/ports close and
all 271 source hashes match. No settled or graphics acceptance is claimed.

## Confirmed owner fault

The independent Roadster session publishes a cache containing 32,768 entries /
41,872,998 bytes at 14:34:53 UTC. Griffin loads exactly those counts at 14:35:18.
The Roadster process later publishes its retained snapshot again after only four
new compilations. The application publisher writes its whole private snapshot
to a machine-shared file, discarding entries another process has added.

The new regression creates two independent owners from the same seed. One
publishes a new kernel; the stale owner reads a retained kernel and publishes
another. The unchanged publisher drops the first owner's new kernel. This is a
cache ownership defect, not evidence of unstable native stencil identity.

## Repair and contract

`lunco-modelica-worker::worker::native_code_cache` remains the sole cache owner.
It tracks local accesses and writes, snapshots only entries touched since its
last successful publication, and merges that delta with the current disk record
under `lunco-storage`'s cross-process directory transaction. Native file storage
owns atomic replacement. The common bounded LRU and both admitted limits apply
to the merged record. Cache loading treats current runtime entries as newer;
pure cache hits do not request full-file rewrites. Failed publications retain
the pending delta for a future modification; errors remain explicit warnings.
Immutable code bytes stay shared during snapshot/serialization.

Protocol version two has its own namespace, so existing binaries cannot bypass
its transaction contract. Its first fill is measured separately from warm reuse.
No simulation values, executable FMI state, solver settings, precision, checks,
substeps, worker admission or deterministic commit boundaries change. Hook
review: artifact retention and persistence invariants belong to the cache/storage
owners; no scenario or lifecycle policy hook is involved.

## Qualification

The stale-writer regression fails on the unchanged publisher. Four focused
native-cache tests then pass, including entry/byte budgets, codec rejection,
fresh-value/recency preservation during load, stale-owner merging and recovery
after a failed publication. The same four tests pass again on main's integrated Rumoca pin. Scoped Clippy
with warnings denied reports 26 diagnostics in unchanged worker/bridge files;
none points into the changed cache module. No lint waiver or unrelated repair is
introduced. The production initialization network and retained-receipt/scene-replacement
gate pass all 46 authored checks on owned API 5353, which closes normally.
Raw logs/captures stay ignored; no profiling data is tracked.

## Qualified-main normal baseline

The ordinary main-dependency build `87b9d92d` runs in owned PID 3859310/API 5350
and exits normally, with all 271 source hashes unchanged. First physics
readiness is 23.331 s. The initializer takes 7.411 s with the same 16,482
attempts, 3,974 hits and 12,508 compilations. Griffin reaches sleeping four-pad
handoff with clear diagnostics and finite mass/inertia. Landed eligibility is
45.366 s, followed by the five-second soak and ten-second / 240-tick window.
Coupled median/p95 is 6.434/9.030 ms; frame observations are 13.476/29.828 ms.
Camera input and concurrent work qualify these frame/load observations; no
matched-view graphics ratio is asserted.

## Normal candidate: first fill and warm restart

The regular candidate `4cb74412` retains main's dependency pins. Its first fill
starts with the new namespace absent; no old cache is deleted or preseeded.
Owned PID 3967377/API 5351 reaches first physics readiness in 20.548 s. Its
initializer takes 7.363 s with 3,958 hits and 12,524 compilations.

The subsequent process, PID 3970604/API 5352, loads 28,314 native artifacts /
38,250,071 retained bytes. First physics readiness is **14.243 s**, compared
with the baseline's 23.331 s. The initializer takes **3.267 s**, with 16,414 hits,
68 compilations, zero rejected entries and zero evictions. These are observed
normal-run differences (39.0% less loading time and 55.9% less constructor time),
with concurrent workloads retained; they are not an isolated CPU benchmark.
The bounded stale-writer regression supplies the direct causal evidence.

Both candidate runs preserve all 271 before/admitted/after source hashes, reach
four-pad handoff, finish sleeping with finite mass/inertia, remain unpaused,
and retain zero registered warnings/errors and no script errors. Each includes
the five-second landed soak and a ten-second window with at least 240 fresh
ticks. Screenshots are inspected; the warm view shows Griffin and the four-pad
handoff HUD. Both processes and API ports close. Warm coupled median/p95 is
3.247/3.929 ms and frame observations are 9.082/12.018 ms; camera/contention
prevent a matched graphics comparison. The sub-second loading and sub-ms tick
targets remain open.

The repair removes repeated native compilation caused by stale publishers.
Remaining loading time includes USD admission, prepared-IR decoding, live
component construction and the coupled initial-condition solve. The warm
initializer alone still costs 3.267 s. No part of the descent is counted as loading.

## Warm-cache native attribution

A separate Tracy 0.14.1 / 1 kHz startup-only capture uses owned app
4009688/API 5354 and collector 4009612/port 8446. The socket receipt matches
both PIDs. Its 268,057,599 bytes contain 12,102 native blocks and 268,009,185
compressed payload bytes, matching the collector's rounded 255.59 MiB.
Native export reads all 4,311,857 zones. Source hashes match the normal cohort;
both processes and ports close. The instrumented readiness observation is
20.303 s and supplies no product timing comparison.

The initializer's exact constructor zone is 4.158 s on CSV thread 51. Its
3,114 selected samples have zero missing stacks and exactly one matching zone;
the owned maps and ELF symbol index resolve 1,596 addresses. Leading leaves
include SHA-256 (106 samples), `LinearOp::clone` (99), symbolic operation-key
lookup (85), `Terms::key` (68), `SymbolicSlots::run` (60), shared-value builder
append (52), and `ExactRefreshAssignmentSchedule::shared_segments` (51).
Cranelift verification remains visible (78). Kernel reuse therefore leaves
native IR construction, proof preparation, hashing and allocation costs.
The published, held schedule-sharing upgrade addresses an observed owner;
its integration still requires resolving the exact-replay failure.

The initializer's disk-lookup zone is 1.520 s. A separate exact-zone selection
has 664 samples, zero missing stacks and one matching zone. Dependency-state
destruction (58), structural dependency walking (36), canonical assignment
shape derivation (22) and exact coverage marking (15) appear alongside JSON
string decoding (15 and 14). This boundary includes native validation of the
decoded FMI component; its inclusive duration is not raw disk latency. Those
checks remain enabled.

The main, attitude-propulsion and attitude-actuation constructors take
1.543/1.342/1.074 s under instrumentation and run consecutively. The source
owner uses one ordered queue for prepared IR and live construction completions;
a ready preparation can wait behind an earlier live construction. Further
parallel preparation must retain deterministic admission, lane assignment,
failure delivery and publication rather than consume worker completion order.
No queue, scheduling or numerical change is made in this cache repair.

The regular optimized development build is restored after capture, SHA-256
`ffb5e39dd2bdbd4ff868f10881c2f8a6b19c5039bb49dd16edfdbc0aa0be82a3`.
The difference from the measured candidate is final source formatting/line
metadata; no behavior changed after the production gates. All raw profiling
artifacts remain ignored.
