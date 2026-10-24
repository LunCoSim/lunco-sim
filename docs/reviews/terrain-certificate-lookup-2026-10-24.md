# Terrain certificate lookup — 2026-10-24

`lunco-terrain-surface::occlusion` owns certified bounds of the immutable f64
surface; physical solar visibility consumes them. Whole-cell certificates use
integer cell identity, while refined rectangles retain their exact coordinate
bits. The shared FIFO, complete surface fence, query budgets and conservative
height calculations retain their contracts. This is cache storage in a continuous
math hot path; it adds no policy, public API, approximation or dependency patch.
The normal development profile optimizes this package at level 3 with assertions
enabled, as it does the native physics integration.

The focused occlusion run passed eleven checks, including the temporary query
benchmark. The ten retained mechanism checks cover grazing and narrow blockers,
moving/crossing rays, surface replacement, bounded storage and cache-independent
quota failures. The temporary benchmark was removed after measurement. Its
64-tick, 16-ray cohort uses identical interval and retained-certificate counts;
isolated time changes from 9.93/10.17 ms to 7.09/7.03 ms for resolutions 510/513.
This is a pure query measurement, not whole-tick acceptance.

The regular production Tracy build passes the eight solar-visibility checks,
two night checks and expected terminal invalid-solar-admission fault. No solar
sampling, graphics quality, physics timestep, substeps or Twin sources changed.

Fresh Summer captures use the same admitted source hashes, High quality, fully
resident 256-tile terrain, completed derived maps and zero retained diagnostics.
Both final images were inspected. Matching Tracy 0.14.1 readback verifies owned
PIDs 432192/446816, 1 kHz sampling, compressed file framing and native export.
Each comparison window contains the last 240 complete FixedMain instances and
2,160 terrain-ray traversals:

| Instrumented owner | Before | After |
| --- | ---: | ---: |
| Solar publication median | 1.392 ms | 1.248 ms |
| Solar publication total | 342.971 ms | 323.800 ms |
| Coarse traversal total | 313.129 ms | 294.376 ms |
| Whole coupled tick median, API | 7.133 ms | 7.008 ms |

Solar p95 remains approximately 1.96 ms. Other simulator PID 8277 remained
active; profiler and concurrent workload effects apply. These observations do
not establish clean FPS, sub-millisecond coupled ticks or sub-second loading.
All three requested Twin performance targets remain open.

Receipts, logs, source hashes, native CSV and captures remain ignored under
`.cache/codex-evidence/performance-20261023/` and `scripts/perf/captures/`.
Prefixes are `summer-cell-key-{baseline,candidate}-current`,
`native-cache-audit/solar-cell-key-` and `solar-cell-key-gates/`.
Both applications, collectors, native readers and all three gate processes
terminated successfully; owned API/profiler ports were released.

## Regular build across the three Twins

The production build without Tracy completes in 2m02s. Binary SHA-256 is
`04c7dd3040b923c64ba9d3179bdcbf5766dbd631fbd7a66271c5726fe858f5ed`.
Each owned graphical session opens its default Twin from the same empty
replacement scene. Qualification requires the intended physical body, native
integration, complete terrain residency and derived maps, followed by five
seconds of sustained readiness. A separate ten-second window records complete
coupled ticks and unique frame samples before the final screenshot. Source
hashes remain stable, diagnostics are clear, and final images were inspected.
These are startup/settled observations, not completed-mission verdicts.

| Twin | Observed fully-ready time | Coupled tick p50 / p95 | Frame p50 / p95 |
| --- | ---: | ---: | ---: |
| Summer Space School | 4.162 s | 5.284 / 6.313 ms | 13.103 / 16.582 ms |
| Griffin / lunar-base-model | 36.932 s | 8.195 / 10.467 ms | 22.445 / 32.184 ms |
| Tesla Roadster | 2.943 s | 2.542 / 3.450 ms | 7.171 / 10.427 ms |

The other Tesla session remains active, so these are contention-affected
observations. All requested thresholds remain unqualified. The exact source
cohorts, native timings, frame counts and lifecycle receipts are in ignored
`{summer,griffin,tesla}-cell-key-normal-settled/` and
`normal-three-twin-current.json`. Owned PIDs 468419, 469355 and 470245 exit, and
API ports 5111–5113 are released.

Griffin needs broader attribution. Its native Avian median is 2.415 ms, with
20 dynamic bodies, 638 total bodies, 777 colliders, 31 joints and 315 retained
contact pairs at the last observation. Its main-propulsion Modelica participant
reports 7.898 ms mean worker service across 1468 samples and 4.612 ms on its
last step. During loading, the coupled initializer's cache lookup takes 0.963 s
and live construction 4.598 s despite predominantly native-code cache hits.
These owner timings identify investigation targets; they do not establish
which internal operation or pair category is responsible.
