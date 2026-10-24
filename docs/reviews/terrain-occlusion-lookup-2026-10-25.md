# Physical terrain occlusion: lookup attribution and bounded diagnostic

The production algorithm is unchanged. Bevy prehashed keys and separate compact
whole-cell storage did not establish a useful consumer improvement, so neither
experiment remains in the runtime. The retained change is an ignored, bounded
numerical timing diagnostic in `lunco-terrain-surface`.

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

The diagnostic uses a 257-post flat base plus the native analytic Overzoom
modifier, 36 positive-slope moving rays per tick, and 512 ticks per capacity.
Every ray must certify clear. Both capacities produce 2,359,296 admitted
intervals; final occupancy is 256 and 1,558 respectively. It also asserts
FIFO/map cardinality and the configured capacity. The existing ten occlusion
mechanism checks retain negative coverage for ambiguous geometry, unavailable
bounds, exhausted quotas, replacement surfaces and moving-ray certificates.

Regular `cargo test --locked -p lunco-terrain-surface --lib --no-run -j 4`
produces immutable test binaries for alternating comparisons, using the normal
target directory and sccache. Five baseline/prehashed pairs give these medians:

| Capacity | Baseline | Prehashed | Difference |
|---|---:|---:|---:|
| 256 | 11,536.567 ms | 11,538.966 ms | 0.02% slower |
| 65,536 | 56.878 ms | 57.044 ms | 0.29% slower |

Every pair has identical verdicts, interval totals and occupancy. A separate
compact-storage probe gives 11,776.742/56.530 ms; this single probe does not
establish an improvement. Both candidates pass the same ten mechanism checks.
No candidate production build or new scene-performance claim is made. The
current normal production executable remains unchanged.

## Next algorithm boundary

The raster DDA still visits every crossed cell after a ray rises above the
nearby terrain. Certified bounds over larger spatial regions could avoid those
lookups. Such acceleration must conservatively enclose the actual bicubic base
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
and `scripts/perf/captures/`. Temporary executable links are removed. No new
runtime session was launched for these pure numerical experiments. Sub-second
loading, sub-millisecond coupled ticks and the complete graphics target remain
open.
