# Large typed maps at the Rhai hook boundary

Runtime exposure policies receive complete typed facts, including participant
output maps. `lunco-hooks-rhai` remains the single adapter from `HookValue` to
Rhai values. It now orders and deduplicates large maps before building the
native B-tree; repeated keys retain the last value. Small maps retain direct
insertion. No fact, hook, policy, clock or execution boundary is removed.

Eight alternating baseline/candidate pairs of the existing complete nested-map
conversion diagnostic give median elapsed times, in milliseconds per 262,144
entries:

| Map entries | Key order | Baseline | Candidate |
| ---: | --- | ---: | ---: |
| 8 | sorted / reverse / permuted | 45.52 / 42.73 / 44.59 | 44.77 / 43.10 / 44.51 |
| 128 | sorted / reverse / permuted | 69.82 / 51.47 / 62.05 | 50.77 / 53.49 / 59.02 |
| 2,048 | sorted / reverse / permuted | 98.95 / 59.26 / 87.11 | 54.08 / 55.62 / 68.59 |

This diagnostic uses the test allocator and nested synthetic values. It does not
establish a production-frame or simulation speedup. The raw paired data and
executable fingerprints are `target/perf/fps-20261106/hook-map-paired.json`.
The low-level conversion test passes nested values, signed zero, unsigned values,
Unicode, empty maps and both small and large duplicate-key cases. Production
Rhai and Griffin validation are separate from this result.

The regular production executable `cc75e8c237adbb0104ab29e3311e485d46a584c84a6dad8126425cc8bbcdf946`
passes the 47-check authored hook-policy gate, including wide-map value and
negative-case checks, and the separate 13-check UI preparation plus 20-check
scene gate. Owned API sessions 6266 and 6268 exited and released their ports.
The build's three existing `lunco-spatial` dead-code warnings are unrelated to
this adapter.

Uninstrumented High/2560×1440 Griffin cohorts `hook-map-normal-pairs/` and
`hook-map-normal-confirm/` measure baseline / candidate / candidate / baseline
/ baseline / candidate mean FPS of 36.16 / 56.37 / 57.58 / 48.72 / 56.96 /
51.94, respectively. Their p95 frame times are 42.51 / 21.43 / 20.22 / 25.03 /
20.62 / 23.61 ms. The first baseline overlaps another windowed simulator and
is not comparable. GPU clocks vary across the remaining runs; the confirmation
baseline matches earlier candidates, so these data do not establish a causal
end-to-end FPS gain. Each session reports zero runtime diagnostics, preserves
all 548 Twin source hashes, and exits through its own API. The 150 FPS goal
remains unmet.

The live `hook-map-sizes/` read confirms that the landing scene uses large
participant maps: MainPropulsion has 3,035 output values, AttitudePropulsion
2,317, and several others hundreds. This is the same `SimComponent.outputs`
map consumed by runtime UI facts, not an inference from model variable counts.

The matching Tracy baseline / candidate / baseline captures use the same fixed
landing pose and playing phase. After excluding the first two and last half
second of each phase, `exposure_runtime_surface_projection` averages 1.518 /
1.329 / 2.185 ms per call (380 / 380 / 378 calls); p95 is 3.539 / 2.939 /
4.524 ms. The paused phases have no calls after their unchanged inputs settle.
A separate windowed simulator and a headless simulator were active throughout,
and the baseline drift makes the size of a production improvement uncertain.
These nested worker spans are not frame times, and Tracy FPS is not acceptance.
All three captures retain the 60 Hz clock and eight substeps, preserve 548 Twin
source hashes, report zero diagnostic findings and close their API and Tracy
ports. Evidence is `hook-map-tracy-{baseline,candidate,baseline2}/` under
`target/perf/fps-20261106/`; `hook-map-sizes/` records the values inspection.
