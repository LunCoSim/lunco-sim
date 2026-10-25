# Physical terrain occlusion: lookup attribution and bounded diagnostic

The production cache retains Bevy's hash map. Neither the prehashed-key nor the
compact whole-cell experiment remains in the runtime. Their paired timing
comparison is excluded by the artifact-identity audit below. The reusable
diagnostic belongs to `lunco-terrain-surface`.
The retained mathematical repair and current production qualification are in
[the native enclosure review](terrain-procedural-enclosures-2026-10-25.md).

## Attribution

The qualified Summer CPU-builder Tracy capture
`summer-native-cpu-builder-profile` contains 240 measured
`publish_sun_visibility` instances: 0.961090 ms median and 1.223365 ms p95.
This is instrumented attribution, not product-performance acceptance. The
[native-builder review](native-instance-builder-2026-10-25.md) owns the capture's
source, readiness, framing, workload and normal-build qualifications.

Exact-zone CPU selection covers PID 2088183 at a verified 1,000,000 ns sampling
period: 237 samples, zero missing stacks and 240 matched zones. Resolving leaf
addresses with that executable's saved PIE mappings and symbol table identifies
96 samples in Hashbrown's `RawTable::find` for `CellBoundsKey`, 37 in
`refine_ray_enclosure`, 17 in the ray-classification closure and only four in
`foldhash::hash_bytes_long`. Sixty-nine leaves point to the key's derive line;
that source location alone does not identify hash calculation. Table lookup
and its inlined key equality are the stronger lead.

## Consumer evidence

The diagnostic uses flat bases with one- and two-metre raster cells plus the
native analytic Overzoom modifier. It queries 36 positive-slope moving rays per
tick: 32 ticks at capacity 256, and 512 ticks at capacity 65,536. Every ray must
certify clear. It reports interval admission, occupancy and elapsed time, and
asserts FIFO/map cardinality and the configured capacity. The existing occlusion
mechanism checks retain negative coverage for ambiguous geometry, unavailable
bounds, exhausted quotas, replacement surfaces and moving-ray certificates.

## Artifact identity

The earlier cache-layout comparison used hard links to Cargo's test output.
Rebuilding overwrote the linked snapshots together; identical timings from those
paths do not compare different implementations. That paired result is withdrawn.
The independently run numerical checks remain valid, and no cache-layout change
was committed. Use independent copies, verify different device/inode identities,
and check recorded hashes after both builds and after the comparison.

The normal CPU/GPU renderer comparison is unaffected: its session receipts have
distinct hashes `31d869123...` and `d227f3cec...` for the respective builds.
The saved native Tracy captures are immutable data from their owned live PIDs.
Do not infer that a hard-link snapshot remained immutable merely because a
different artifact family happened to survive a rebuild.

## Next algorithm boundary

The raster DDA visits every crossed cell after a ray rises above nearby terrain.
First inspect the procedural modifier's conservative certificates: an overly
broad amplitude interval can force repeated refinement and lookup even over
clear ground. A larger-block traversal experiment was removed without a verified
consumer improvement; it did not establish a solution to that certificate
problem. Any future spatial acceleration must conservatively enclose the actual bicubic base
and all analytic modifiers, including extrapolated boundary posts and overshoot.
Sampled visual LOD height ranges and collider triangles do not certify that
surface. Retain fresh f64 rays, exact surface identity, bounded work and visible
failure for unavailable or ambiguous geometry.

The authoritative owners are `HeightGrid::cell_height_bounds` for base geometry,
`SurfaceOracle` for modifier composition, and terrain preparation for immutable
derived artifacts. Native restamping already runs off thread, but web worker
replies compose the oracle on the page thread; an eager full-grid index in the
oracle constructor would introduce a new UI hitch. Any larger-region index must
use the existing asynchronous preparation and admission boundaries on both
platforms, including invalidation after raster or analytic edits. Its preparation
cost and memory must also be measured against the loading target.

Raw captures, symbols, logs and paired results remain ignored under `.cache/`
and `scripts/perf/captures/`. No runtime session was launched for these numerical
experiments. Sub-second loading, sub-millisecond coupled ticks and the complete graphics target remain
open.
