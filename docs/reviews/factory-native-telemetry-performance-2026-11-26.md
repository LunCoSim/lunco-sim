# Factory fixed-tick telemetry performance

The Factory Site Layout's remaining frame tail was attributed from the native
Tracy 0.14.1 capture after the animation optimization (`de1c9c204`). Across
1,550 complete update intervals from capture-relative 12–31 seconds, the
slowest 78 averaged 22.79 ms versus 11.70 ms for the other 1,472. The main-thread
`FixedLast` contribution was 9.82 versus 0.30 ms; `retain_physics_telemetry`
accounted for 9.62 versus 0.18 ms. Seventy-five of those 78 slow frames spent
more than 5 ms in the recorder. Render-schedule and PostUpdate timings were
nearly equal between the groups. These are instrumented diagnostic timings.

A regular native uninstrumented A/B/A on the same owned Factory session
(1600×1000, RTX 5060 Laptop GPU, standard quality and uncapped presentation)
confirmed the recorder's contribution: master telemetry was enabled, temporarily
disabled for diagnosis, then restored. Existing VS Code activity varied during
this bracket; it is not clean FPS acceptance.

| Diagnostic cohort | Mean FPS | p95 ms | p99 ms | Fixed ticks / 20 s |
| --- | ---: | ---: | ---: | ---: |
| Enabled A | 146.17 | 11.48 | 18.42 | 1213 |
| Disabled B (diagnostic only) | 165.91 | 8.14 | 10.32 | 1206 |
| Enabled A2 | 142.90 | 9.89 | 18.68 | 1214 |

The production solution keeps recording enabled and preserves the configured
5 Hz sample cadence, exact simulation timestamps, channel names, metadata,
cap, rejection, history depth and sample values. The physics recorder uses a
single joined ECS body query, one static descriptor inventory of 51 channel
IDs, a reused numeric sample buffer, and one dense slot table per live source.
The shared signal registry supplies exact lease-checked recording capabilities.
Stable recording slots avoid per-sample name resolution and catalog activation;
refused channels recheck admission only on a membership/cap revision. Lease
validation and history append share one mutable history borrow. Chunk growth
and immutable snapshot ownership remain in the same signal registry. No
frame-spread sampling, deadband, coalescing or dropped sample policy is added.
Hot retention and history-growth Tracy names have no varying span fields.

The existing release `settled_retention_benchmark` records 2,304,000 samples,
384 channels and the identical retained-history digest `d9716d51526de325`.
It took 332.32 ms before and 43.54 ms after (7.63×); these are isolated producer
and registry timings, not whole-frame FPS. The two changed owners passed 26
unit tests (21 signal, five physics telemetry, excluding two manual benchmarks),
including removed/reused leases, rate/history/retention, chunk snapshots,
admission caps and body retirement.

The production Factory Site Layout (`3efedb4a0dacdabbc905f7a6bfa495fca9c7b0ad`)
retained all 4,306 consecutive frames in 20 seconds, **215.07 FPS**, 4.65 ms
mean, 6.28 ms p95, and 1,203 fixed ticks. Its `RuntimeDiagnostics` had no
findings. The existing configured channel cap still reports its warning.
The 150 FPS mean and p95 target is 6.67 ms. The actual Factory session passed
all 18 authored `telemetry_recording_history.rhai` checks with `position.x`
(m) and `warehouseKg` at 5 Hz, including steady values, strictly increasing
simulation timestamps, metadata, retention and missing-channel rejection.
The generic sandbox could not serve this gate in the non-Python production
build because two unrelated optional Python programs held readiness. The
qualified Factory session had no such hold. Captures, per-frame receipts and
logs are in ignored `target/perf/factory-20261125/` and all owned sessions exit.

A later matched native bracket used the saved previous executable with identical
camera, scene source and telemetry settings. In the same quieter desktop cohort,
old versus new mean FPS was 209.95 versus 215.07, p95 6.39 versus 6.28 ms,
p99 12.75 versus 7.63 ms, and frames over 10 ms fell from 101/4205 to 2/4306.
Both advanced 1,204 and 1,203 fixed ticks, respectively, in 20 seconds. Four
physics channel groups (`linear_velocity.x`, `position.x`, `orientation.quat.w`,
`contact`) had exactly equal sets of USD group path, units, provenance and
activity across the runs, with 268, 268, 268 and 267 channels respectively.
The before run retained 90 samples per selected channel versus 89 after; the
windows have different startup phase/elapsed time, not a reduced sample rate.
The authored history gate verifies the unchanged rate contract.
