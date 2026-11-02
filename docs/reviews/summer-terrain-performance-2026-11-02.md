# Summer terrain correctness and performance qualification

The terrain/contact implementation is recorded in
[the terrain review](terrain-contact-and-camera-cover-2026-11-02.md).
Commits `7f51409bc` and `525f10b98` are integrated into local main. They share
canonical physical triangles across contact, camera detail and terrain-bound
route/trail presentation, and publish resident material morph pins before an
idle stream can complete. Camera distance does not lower physical detail.

## Verification ownership

The production API driver exercises six phases with 11 authored assertions each:
initial terrain, a driven tile crossing, constrained discretionary quality,
finer geometry, far camera and close camera. Each cover and its mesh handles
are read within one World hook. Missing or retired handles fail visibly.
The driver records contiguous frame sample identifiers, their full history,
mean/median/p95/maximum frame time, and optional companion process identity and
CPU activity. It verifies the owned binary/cwd and releases its API port.
A disappearing rover records document topology and runtime diagnostics before
failing; it is not silently rebound to a replacement entity.

`scripts/api/test_physical_presentation_time.py --windowed` runs the existing
13-check authored clock contract through the native host. Its default mode
uses the production scene-test host. Pause, manual steps, fractional and
accelerated rates, fixed quanta, epoch mapping and celestial clocks retain
one authoritative test. The native deadline allows paused unfocused frames;
it is a liveness bound rather than a frame-time gate.

## Tracy diagnosis

Diagnostic tools are the adjacent `../tracy` release 0.14.1 collector and native
CSV exporter. Collection starts before the owned app, with profiler port 8092,
API port 4794 and 1000 Hz CPU samples. Instrumented numbers are attribution
only. Native exports and payload/framing checks pass for both captures below.
Raw evidence stays under ignored `target/perf/summer-10ms/`.

`summer-before.tracy` contains 1,269 frames and 5,340,230 zones over 34.89 s.
Its compressed payload (227,802,527 bytes) plus framing (42,362 bytes) matches
the 227,844,889-byte file. In a settled 8–24 s window, 711 native render
receipts take median 7.037 ms and p95 8.938 ms. Schedule self time also includes
worker waits and instrumentation; it does not establish scheduler overhead.

The separate native scheduling candidate capture `summer-coherent.tracy`
contains 572 frames and 2,851,532 zones over 37.52 s.
Its payload (218,211,502 bytes) plus framing (39,470 bytes) matches the
218,250,972-byte file. All 66 terrain assertions pass. In the selected drive
window, 200 native receipts take median 20.663 ms, p95 42.329 ms. GPU shadow
and opaque stage totals are about 6.6 ms per frame combined; this does not
include every GPU stage. The concurrent media workload and instrumentation
prevent treating this as product frame-time acceptance.
That candidate is not included in the final implementation.

## Performance boundary

The requested target is 10 ms with two ordinary interactive simulators.
That target is not qualified. The companion on API 4473 is owned by another
task and was never controlled. Its Griffin media scene uses a 2560 by 1440
window. Read-only GPU observations show it can saturate the GPU even after
recording ends. Such a workload is not a controlled two-interactive acceptance
cohort. Graphics quality, physics quanta and substeps remain unchanged.

The uninstrumented `current-interactive/` drive passes all 66 checks over
10.881 m, with 148 contiguous frames: mean 101.505 ms, median 115.422 ms,
p95 119.637 ms and maximum 150.734 ms. After the owned app exits, the companion
alone still uses 100 percent GPU. This result documents contention; it does
not establish a native scheduling regression or a successful optimization.
No unqualified native scheduling or worker-budget change is included.

## Final production validation

The final binary SHA-256 is
`62ece605df86ff16fe0bd49838180cfb6aeb556cbe324204c882f1110d286541`.
Its runtime source is `525f10b98`, with Summer Twin commit
`ee2e5b9f7590fd32633fe9214d8f09998847cd24`. Remaining changes are test drivers
and documentation. `final-main-terrain/` passes all 66 authored checks with
zero runtime errors/warnings, clean native GPU logs and released port 4794.
The rover travels 10.036 m. Worst render/contact error is 0.000005952 m.
The far camera selects depth 4 at 116.161 m clearance while physical support
retains depth 9; the close camera selects depth 10 at 2.994 m clearance.
Screenshots were inspected for terrain detail and route projection.

This final drive records 138 contiguous frame samples: mean 102.167 ms,
median 118.483 ms, p95 128.389 ms and maximum 144.033 ms, with the same
GPU-saturating companion. It is not a passing 10 ms result.


`final-main-native-clock/` passes all 13 authored presentation-clock checks
through the normal native host, and releases port 4796. The production build,
Python syntax checks, skill catalogue validation and `git diff --check` pass.
No application or Tracy collector owned by this task remains running.
