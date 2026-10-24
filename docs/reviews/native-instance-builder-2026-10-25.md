# Native mesh-instance preparation

The desktop composition selects Bevy's native CPU mesh-instance builder through
`PbrPlugin::use_gpu_instance_buffer_builder = false`. Both windowed and offscreen
hosts use this configuration. Native GPU drawing, material shaders, shadows and
quality settings retain their contracts. The existing render adapter derives
effective preprocessing and culling from the actual native builder. No instance
builder, GPU-data ABI, solver, cache, runtime switch or dependency patch is added.

The choice favors the measured frame budget across the three target scenes.
CPU preparation materially improves Summer and modestly improves Griffin;
Tesla's GPU median is lower, while both modes stay within its measured frame
budget. This is a host workload choice, not a universal CPU/GPU ranking.

## Normal production comparison

All runs use regular `target/` builds, high quality, no VSync or throttling,
isolated settings, complete terrain readiness, five seconds of soak and at
least ten measured seconds with 240 fresh coupled ticks. Each CPU/GPU pair has
exactly equal admitted Twin source hashes. Registered diagnostics are clear,
the images are inspected, and every owned app closes. Griffin completes
touchdown and four-pad handoff; Summer and Tesla are initial nominal windows,
not complete driving or mission acceptance.

| Twin | GPU frame median/p95 ms | CPU frame median/p95 ms | CPU coupled median/p95 ms | CPU first physics ready s |
|---|---:|---:|---:|---:|
| Griffin | 10.962 / 13.837 | 9.816 / 12.720 | 3.670 / 4.496 | 14.360 |
| Summer | 8.472 / 10.094 | 5.173 / 9.717 | 3.326 / 4.112 | 1.508 |
| Tesla | 4.469 / 6.819 | 5.781 / 6.872 | 1.597 / 2.038 | 2.728 |

Griffin's CPU/GPU/CPU sequence reports frame medians 10.416, 10.962 and 9.816 ms.
Its physical counts remain 638 bodies, 20 dynamic, 21 sleeping, 777 colliders,
31 joints and 315 contact pairs. An earlier GPU run during concurrent Tesla
video rendering reports 80.348 ms; it is excluded from the quieter comparison.
The separate Tesla process remains resident. These are bounded sequential host
observations, not isolated hardware throughput or cross-platform acceptance.
The GPU monitor started after the GPU-after-CPU app closed; it covers startup
of the following CPU run, not that preceding GPU measurement window.

Source revisions are Griffin `78d308e4e4fa97ce3d0b07d3b65437f3c95a4de5`, Summer
`9859e30efd0950a9735e9eb7feadee9aa5320b18` and Tesla
`a8e7b105f86a9fdf65cbf287501943aefd14fefc`. Existing authored Tesla edits are
preserved; its frame/datum-unavailable HUD remains visible. No HUD repair is
claimed. Full loading, coupled-tick and graphics targets remain open, including
Griffin's frame tail above 10 ms.

## Native attribution and visual gates

Separate Tracy builds use the adjacent 0.14.1 collector, started before the app,
with 1 kHz sampling and the same executor-filtered diagnostic settings. Native
framing and CSV export pass. CPU PID 2088183/API 5253 connects to collector
2088116 on 8436; GPU PID 2147255/API 5254 connects to collector 2147220 on 8437.
Both retain equal Summer sources, complete readiness, clear diagnostics and
inspected images, then close with their collectors. The captures contain
222,520,339 and 229,311,247 bytes respectively. Exact measured-window selection
excludes eleven CPU and fifteen GPU screenshot/shutdown ticks; every retained
API timing sample passes its native-counter duration check.

Within the selected 240 complete fixed ticks, native GPU preprocessing bind-group
preparation is 2.507 ms median per invocation and shadow instance-buffer writing
is 1.837 ms. Native CPU shadow batching is 0.366 ms. Inclusive render-schedule
medians are 14.224 ms GPU and 5.080 ms CPU. Parent and child costs overlap; these
are attribution values with profiler overhead, not product frame times.

The normal CPU build passes both existing pixel-gate contracts through the
production runner: `shared_shadow_allocation` on owned PID 2015648/API 5244 and
`vertex_shadow_alignment` on PID 2087121/API 5252. Each drains three GPU frames,
passes its authored setup and both positive probes, rejects red pixels and exits
zero. This covers local-light and Sun shadows, uniform-driven vertex deformation
and all camera prepasses. The point-light image is also inspected directly.

Regular CPU construction passes in 1m38s; CPU and GPU Tracy builds pass in 2m08s
and 1m50s. The pre-integration normal CPU build passes in 27.80s, SHA-256
`00f8ade58106244a9adcfaa4fa85ddf169ec786fb207c3c5cb4f6e7c8f10f902`.
The plot UI changes on newer local `main` are preserved through a fast-forward
to `37f2e4172635d6dd7b76da90743488756cb111ee`. The integrated normal build passes
in 1m21s, SHA-256
`a1742f4dfdb99142b9f9defcb40e7dd3bd457c8fb56a0ab368fbcf2f36ec9b8c`.
Owned Summer PID 2177375/API 5255 repeats the readiness/terrain/240-tick window,
with unchanged sources, clear diagnostics, an inspected image and normal exit.
It reports 2.407 s first physics readiness, 3.784/7.608 ms coupled service and
10.177/18.269 ms frames. A concurrent four-thread video export uses about 337
percent CPU. This integrated window proves operation under that load, not the
earlier timing ratio or a clean frame-budget acceptance. No behavior tests are
repeated solely for the explanatory comment, formatting or docs.

The same captures identify physical solar visibility as a remaining coupled-tick
cost: 0.961 ms CPU and 1.135 ms GPU, with 240 calls each. The native sample reader
verifies both owned PIDs and 1,000,000 ns periods, selecting 237 and 282 samples
inside those exact zones with zero missing stacks. Enclosure construction/cache
work remains visible and needs its own source-level attribution before editing.

Raw traces, symbols, maps, images and receipts remain ignored under
`.cache/codex-evidence/performance-20261023/`, `scripts/perf/captures/` and
`target/scene-tests/`. The two temporary baseline executable hard links are
removed. Normal Cargo outputs and shared caches are retained.
