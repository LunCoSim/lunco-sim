# Factory native animation performance

Factory Twin source `3efedb4a0dacdabbc905f7a6bfa495fca9c7b0ad`, application
baseline `b695fb698`, native Wayland desktop, NVIDIA RTX 5060 Laptop GPU
(Vulkan), 1600×1000, standard rendering, uncapped presentation. Each owned
session opened the exact authored scene with the production executable, used
`/World/Avatar` in View, settled before retaining 20 seconds of consecutive
frame-history samples, then closed. The private Twin preserves the authored
scene and model sources; the factory startup scripts may update its UI settings.
The original Twin remained untouched.

| Scene / cohort | FPS | Mean frame ms | p95 frame ms |
| --- | ---: | ---: | ---: |
| Site Layout, before, Tracy | 55.08 | 18.18 | 24.98 |
| Site Layout, retained transform queries, Tracy | 81.74 | 12.23 | 20.03 |
| Site Layout, retained transform queries, normal release | 184.85 | 5.41 | 7.09 |
| Factory floor, normal release, authored paused startup | 176.11 | 5.68 | 8.20 |

The Site Layout has five sampled transform channels with 146,520 authored
keys. OpenUSD's native `XformQuery` contains per-attribute `AttributeQuery`
source memos; resolving a timed source copies its composed sample map. The
canonical stage retains those queries across animation samples, invalidates
on committed edits, and retires them with the stage. A single native matrix
provides both the render pose and optional f64 parent-local grid translation.
No track is downsampled, and no render-quality or physics setting is changed.

The adjacent Tracy 0.14.1 collector and native CSV exporter report
`sample_usd_animation` mean self time 5.297 ms before, 0.077 ms after (68.4×).
Both captures used 1 kHz sampling and the same native scene/camera. The first
nested-X11 diagnostic measured 49 ms of `surface_present` wait and is excluded
from the native FPS comparison. The clean result is not a before/after FPS
ratio because there is no matched uninstrumented baseline. Its mean exceeds
150 FPS; p95 remains just outside 6.67 ms. The Site Layout retains 501 native
colliders and completed 1,207 fixed ticks during the 20-second clean window,
with achieved transport 1.012× and no registered runtime diagnostic findings.
The factory-floor scene intentionally pauses its transport at startup; it is
not evidence of advancing physics in that scene. The default telemetry channel
cap warning appeared in Site Layout runs and was not altered.

The focused stage test `retained_animation_query_tracks_committed_stack_and_sample_edits`
passed (native interpolation, Z-up centimetre convention, f64 grid position,
and committed sample/order edits before projection-notice drain).
The production `native_xform_animation.rhai` scenario passed all ten authored
checks (native suffixed operation, inert unordered operation, live order
retirement/restoration, stable entity and geometry, terminal rejected malformed
order). The native before/after screenshots were inspected. Captures, CSVs,
frames, executable hard links and receipts are under ignored
`target/perf/factory-20261125/`.
