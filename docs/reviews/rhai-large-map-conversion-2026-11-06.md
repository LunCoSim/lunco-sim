# Large typed maps at the Rhai hook boundary

Runtime exposure policies receive complete typed facts, including participant
output maps. `lunco-hooks-rhai` remains the single `HookValue`-to-Rhai adapter.
Small maps use direct insertion. Large maps collect directly into Rhai's native
`BTreeMap`. The conversion test covers the last-duplicate-wins
contract, nested values, signed zero, unsigned values, Unicode and empty maps.

## Paired Tracy comparison — 2026-11-08

The baseline and candidate ran the same production Griffin `OpenFile` workload:
Twin `astrobotic-griffin-1`, `scenes/griffin_1_surface_ops.usda`, High quality,
2560×1440, pointer passthrough, and authored `landing_handoff`. The baseline
binary SHA256 was
`8157789e817ca0294e726662747dd172197856f0135192d640c39c5477f657a5`; the
candidate SHA256 was
`0f1948c81ff0023aa42aef4ee9af7296d4d8ed453a13068508b0450bbcce4d37`. The
matching Tracy 0.14.1 collector sampled at 1,000 Hz. The ABBA runs used API / Tracy
ports 4257 / 8207, 4258 / 8208, 4259 / 8209 and 4260 / 8210, respectively.

The last ten seconds of each capture contained 398 complete
`exposure_runtime_surface_projection` zones. Their mean and p95 durations were:

| Run | Mean | p95 | Selected CPU samples |
| --- | ---: | ---: | ---: |
| Baseline A | 1.129 ms | 2.366 ms | 453 |
| Direct-map A | 1.058 ms | 2.187 ms | 416 |
| Direct-map B | 1.035 ms | 2.199 ms | 412 |
| Baseline B | 1.242 ms | 2.529 ms | 490 |

Across both runs, the direct-map candidate reduced the mean and p95 projection
time by about 12% and 10%, and selected CPU samples by about 12%. Native sample
resolution against each run's exact ELF showed fewer adapter samples in
`hook_to_dynamic`; `_mi_page_malloc_zero` samples also fell by about 7%. Map
construction remained visible in both candidates.

All four runs passed the authored landing handoff, reported zero runtime
diagnostics, retained the same Twin source snapshot, and exited with their API
and Tracy ports released. The exact captures, filtered zone events, selected
samples, native maps and receipts are under `target/perf/tracy-20261108/` in the
`repeat-abba-*` directories.

This is Tracy-only CPU-zone evidence. It does not establish an FPS result or the
150 FPS objective, which remains open.
