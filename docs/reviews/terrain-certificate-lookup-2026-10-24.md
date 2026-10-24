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
