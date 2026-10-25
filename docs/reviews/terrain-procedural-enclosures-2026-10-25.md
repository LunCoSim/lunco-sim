# Physical solar: native procedural enclosures

Owner: `lunco-terrain-core::Overzoom::height_bounds`, consumed by
`SurfaceOracle::occludes_ray` for initialization and fixed-tick solar probes.
The retained change tightens the global crater-band enclosure at its mathematical
owner. No spatial index, worker protocol, dependency patch, cache-layout change,
sampling reduction or new runtime policy is introduced.

## Mathematical boundary

For positive crater depth d, the bowl lies between -d and zero. Rim plus apron,
and the subtracted reach tail, each lie between zero and a*d, where a is derived
from the generator's maximum rim ratio and the profile's apron fraction. The
complete signed profile therefore lies in [-(1+a)*d, a*d]. Negative depths
reverse those endpoints. The enclosure covers both depth signs, every admitted
radius and up to nine cells of the generator's bounded crater population.
Regional bounds intersect that global enclosure; the existing f64 padding,
Nyquist gate, warp handling and noise enclosure remain in place.

The generator and bound share their rim-ratio and count constants. Pointwise
terrain equations and their operation order remain unchanged. The cache still
uses exact coordinate bits, complete surface invalidation and bounded FIFO
admission; the DDA and canonical cuts remain the same algorithm.

## Numerical and consumer evidence

Two core enclosure checks cover broad and regional rectangles, positive,
inverted and mixed depths, and four detail gates. The existing ten terrain
occlusion checks pass. A new bounded-refinement check certifies clear procedural
rays with a 200-interval limit and rejects a one-interval limit. The baseline
returns `terrain occlusion interval budget exceeded` on its positive case;
the candidate passes both cases.

Five alternating comparisons use independent executable copies. Baseline hash
`436951ff...` and candidate `bee7f0ef...` remain distinct and unchanged before and
after all invocations. The same rays must certify clear in every run.

| Raster cells | Capacity | Baseline median | Candidate median | Change |
|---|---:|---:|---:|---:|
| 2 m | 256 | 7,125.616 ms | 1,748.206 ms | 75.47% less |
| 2 m | 65,536 | 18,149.538 ms | 4,515.267 ms | 75.12% less |
| 1 m | 256 | 1,181.865 ms | 1,196.410 ms | 1.23% more |
| 1 m | 65,536 | 98.444 ms | 110.303 ms | 12.05% more |

The thrashing cases query 1,152 rays over 32 ticks; resident cases query 18,432
rays over 512 ticks. Two-metre resident work drops from 4,592,727 to 2,164,368
admitted intervals and final occupancy from 65,536 to 61,346. One-metre work and
occupancy remain identical. This is a bounded consumer comparison, not a
universal speedup or a complete coupled-tick timing.

The [artifact-identity audit](terrain-occlusion-lookup-2026-10-25.md) withdraws
earlier cache-layout timing pairs whose snapshots shared an inode. The unrelated
topology-revision and source-fan-out receipts also contain identical hashes and
cannot reject those optimization hypotheses. Native CPU/GPU rendering,
Celestial operand sharing and ordered-observation receipts have distinct
variant hashes and remain qualified by their recorded runtime limits.

## Production qualification

The regular `cargo build --locked -p lunco-luncosim -j 4` passes in 3m22s;
SHA-256 `94e77e108fbd452168877cd1019117f8ff4dc585340f4365ee2c6112c0663988`.
All four production terrain-solar gates pass: visibility, night, invalid initial
sampling and invalid live admission. The two negative gates observe the expected
terminal `solar-sample-unavailable` fault. Owned ports 5260–5263 close.

Summer's normal baseline/candidate/baseline windows retain the same 72 admitted
source hashes, full terrain readiness, a five-second soak, at least ten measured
seconds and 240 fresh coupled ticks. Registered diagnostics are clear and each
app exits. Coupled medians are 5.604/4.888/5.752 ms; frame medians are
16.424/15.539/16.619 ms. Another Tesla session remains active. These current
workload-qualified numbers do not meet the requested targets or establish a
loading improvement. The candidate and baseline images retain the same terrain
and route presentation.

Griffin reaches four-pad handoff and settles asleep, with residual linear speeds
below 0.0001 m/s and clear diagnostics. Its current first physics readiness is
30.336 s; coupled median/p95 is 6.669/8.052 ms and frame median/p95 is
22.312/26.880 ms. Its source revision remains `78d308e4...`. The inspected image
retains the lander, terrain and shadow. This is nominal landing evidence, not
qualification of the separate high-energy flight/impact reports.

## Native profiler qualification

The matching opt-in Tracy build passes in 2m32s; binary SHA-256 is
`b204efe7389b3b7a75db81ad051684051e09041021877639bd6c3c687f859b1e`.
Collector PID 2335140 starts before app PID 2335196. Live `ss` output confirms
that app owns API 5259 and Tracy 8438, with the collector connected to that app.
The saved capture retains the normal candidate's exact source hashes, full
terrain readiness, clear diagnostics and normal shutdown. Both processes exit
zero and release their ports.

Native framing is valid: 9,878 blocks, 220,357,763 compressed payload bytes and
220,397,281 total bytes, matching the collector's rounded 210.15 MiB plus native
framing. Native CSV exports succeed. The final 240 measured FixedMain instances
exclude 22 screenshot/shutdown ticks and match fourteen API duration-counter
checks. Complete FixedMain median is 6.940 ms. Physical solar visibility is
0.740/1.053 ms median/p95 across 240 calls. The older capture's 0.961/1.223 ms
belongs to different host pressure; the controlled normal runs and independent
consumer pairs own the speed comparison.

Exact-zone CPU selection verifies the owned PID, 1,000,000 ns sampling period,
180 selected samples, no missing stacks and 240 matched zones. Address resolution
identifies 63 leaves in native certificate-table lookup, 26 in classification,
21 in canonical enclosure refinement and six in the procedural bounds method.
Remaining first-party physics leads include both fixed-step pose bridge passes
and rootless collider propagation. Their inclusive/native schedule placement
must be checked before adding timings or changing ownership. The port-read
sharing hypothesis also remains open after its previous comparison was excluded.

Raw captures, executable snapshots and per-run receipts remain ignored. The
tracked change contains the mathematical repair, its checks, the diagnostic
and documentation.
